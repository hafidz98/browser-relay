//! Browser owns the privileged CDP session to the laptop's Chrome:
//! one attached page target, screencast fan-out, input dispatch, tab
//! management and mobile emulation. A port of the Go core's browser.go.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;

use super::cdp::{Cdp, EventHandler};
use super::hub::Hub;
use crate::rlog;

const MOBILE_UA: &str = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36";

#[derive(Clone, Debug, PartialEq)]
pub struct PageTarget {
    pub id: String,
    pub title: String,
    pub url: String,
}

#[derive(Deserialize)]
struct TargetFull {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(rename = "type", default)]
    typ: String,
    #[serde(rename = "webSocketDebuggerUrl", default)]
    ws_url: String,
}

#[derive(Clone, Copy)]
pub struct ScreencastOpts {
    pub quality: i64,
    pub max_w: i64,
    pub max_h: i64,
    pub every_nth: i64,
}

impl Default for ScreencastOpts {
    fn default() -> Self {
        ScreencastOpts { quality: 85, max_w: 2048, max_h: 2048, every_nth: 1 }
    }
}

struct BState {
    cdp: Option<Arc<Cdp>>,
    target_id: String,
    mobile: bool,
    mobile_w: u32,
    mobile_h: u32,
    /// Device scale factor for coordinate mapping (2 when mobile emulation on).
    dsf: f64,
    /// The layout viewport of the page we drive, in CSS pixels. Read
    /// back from the page itself, because the emulation is not always
    /// what the browser ends up using.
    page_w: f64,
    page_h: f64,
    /// The size of the screencast frame we send, in CSS pixels (DIP).
    frame_w: f64,
    frame_h: f64,
    /// The viewport the client asked for (width, height, dpr). Kept so
    /// it can be re-applied when a tab is attached: a new tab starts
    /// with the plain window size again.
    viewport: Option<(u32, u32, f64)>,
    orig_ua: String,
    last_tabs: Vec<PageTarget>,
}

pub struct Browser {
    cdp_base: String, // http://127.0.0.1:9222
    cdp_host: String, // 127.0.0.1:9222
    hub: Arc<Hub>,
    sc: ScreencastOpts,
    state: Mutex<BState>,
    attach_lock: AsyncMutex<()>,
}

fn nop() -> Value {
    json!({})
}

impl Browser {
    pub fn new(cdp_base: &str, hub: Arc<Hub>, sc: ScreencastOpts) -> Arc<Browser> {
        let host = cdp_base
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_string();
        Arc::new(Browser {
            cdp_base: cdp_base.trim_end_matches('/').to_string(),
            cdp_host: host,
            hub,
            sc,
            state: Mutex::new(BState {
                cdp: None,
                target_id: String::new(),
                mobile: false,
                mobile_w: 390,
                mobile_h: 844,
                dsf: 1.0,
                page_w: 0.0,
                page_h: 0.0,
                frame_w: 0.0,
                frame_h: 0.0,
                viewport: None,
                orig_ua: String::new(),
                last_tabs: Vec::new(),
            }),
            attach_lock: AsyncMutex::new(()),
        })
    }

    // ---- DevTools HTTP ----

    async fn http_do(&self, path: &str, timeout_d: Duration) -> Result<Vec<u8>, String> {
        let host = self.cdp_host.clone();
        let path = path.to_string();
        let path_for_timeout = path.clone();
        let fut = async move {
            let mut stream = TcpStream::connect(&host)
                .await
                .map_err(|e| format!("cdp http connect: {e}"))?;
            let req = format!(
                "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
            );
            stream
                .write_all(req.as_bytes())
                .await
                .map_err(|e| format!("cdp http write: {e}"))?;
            // Read headers first, then exactly the body the headers
            // describe — the DevTools server keep-alives, so reading
            // to EOF would stall until the caller's timeout.
            let mut raw: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 8192];
            let header_end = loop {
                if let Some(p) = find_subslice(&raw, b"\r\n\r\n") {
                    break p + 4;
                }
                let n = stream
                    .read(&mut chunk)
                    .await
                    .map_err(|e| format!("cdp http read: {e}"))?;
                if n == 0 {
                    return Err("cdp http: closed during headers".into());
                }
                raw.extend_from_slice(&chunk[..n]);
            };
            let head = String::from_utf8_lossy(&raw[..header_end]).into_owned();
            let status_line = head.lines().next().unwrap_or("").to_string();
            if !status_line.contains(" 200") {
                return Err(format!("cdp http GET {path}: {status_line}"));
            }
            let head_lc = head.to_lowercase();
            let content_len = head_lc
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok());
            let body: Vec<u8> = if head_lc.contains("transfer-encoding: chunked") {
                // Read until the terminating 0-chunk arrives.
                let mut body = raw[header_end..].to_vec();
                while find_subslice(&body, b"\r\n0\r\n\r\n").is_none()
                    && !body.starts_with(b"0\r\n\r\n")
                {
                    let n = stream
                        .read(&mut chunk)
                        .await
                        .map_err(|e| format!("cdp http read: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..n]);
                }
                decode_chunked(&body)?
            } else if let Some(want) = content_len {
                let mut body = raw[header_end..].to_vec();
                while body.len() < want {
                    let n = stream
                        .read(&mut chunk)
                        .await
                        .map_err(|e| format!("cdp http read: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..n]);
                }
                body.truncate(want);
                body
            } else {
                // No length and not chunked: body ends at close.
                let mut body = raw[header_end..].to_vec();
                loop {
                    let n = stream
                        .read(&mut chunk)
                        .await
                        .map_err(|e| format!("cdp http read: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..n]);
                }
                body
            };
            Ok(body)
        };
        tokio::time::timeout(timeout_d, fut)
            .await
            .map_err(|_| format!("cdp http GET {path_for_timeout}: timeout"))?
    }

    async fn http_get(&self, path: &str) -> Result<Vec<u8>, String> {
        self.http_do(path, Duration::from_secs(10)).await
    }

    /// Whether the DevTools HTTP endpoint answers right now.
    pub async fn is_chrome_up(&self) -> bool {
        self.http_do("/json/version", Duration::from_secs(3))
            .await
            .is_ok()
    }

    /// Record the browser's original UA (for emulation revert). Called
    /// opportunistically by the status loop; failure is harmless.
    pub async fn refresh_orig_ua(&self) {
        if !self.state.lock().unwrap().orig_ua.is_empty() {
            return;
        }
        if let Ok(body) = self.http_get("/json/version").await {
            if let Ok(v) = serde_json::from_slice::<Value>(&body) {
                if let Some(ua) = v.get("User-Agent").and_then(|u| u.as_str()) {
                    if !ua.is_empty() {
                        self.state.lock().unwrap().orig_ua = ua.to_string();
                    }
                }
            }
        }
    }

    async fn list_targets_full(&self) -> Result<Vec<TargetFull>, String> {
        let body = self.http_get("/json/list").await?;
        serde_json::from_slice(&body).map_err(|e| format!("cdp /json/list parse: {e}"))
    }

    pub async fn list_targets(&self) -> Result<Vec<PageTarget>, String> {
        let all = self.list_targets_full().await?;
        Ok(all
            .into_iter()
            .filter(|t| t.typ == "page")
            // Chrome's omnibox popup surfaces are type "page" too but
            // are ephemeral UI, not tabs — keep them off the phone's
            // tab strip (and out of attach-following).
            .filter(|t| !t.url.starts_with("chrome://omnibox-popup"))
            .map(|t| PageTarget { id: t.id, title: t.title, url: t.url })
            .collect())
    }

    async fn target_ws_url(&self, id: &str) -> Result<String, String> {
        let all = self.list_targets_full().await?;
        for t in &all {
            if t.id == id && !t.ws_url.is_empty() {
                return Ok(t.ws_url.clone());
            }
        }
        Err(format!("target {id} not found"))
    }

    // ---- attach / screencast ----

    pub async fn attach(self: &Arc<Self>, target_id: &str) -> Result<(), String> {
        let mut last_err = String::new();
        for attempt in 0..2 {
            match self.target_ws_url(target_id).await {
                Ok(ws) => match self.attach_ws(target_id, &ws).await {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        last_err = e;
                        rlog!("attach to {} failed (attempt {}): {}", target_id, attempt + 1, last_err);
                    }
                },
                Err(e) => last_err = e,
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Err(last_err)
    }

    async fn attach_ws(self: &Arc<Self>, target_id: &str, ws_url: &str) -> Result<(), String> {
        let _guard = self.attach_lock.lock().await;
        let old = {
            let mut st = self.state.lock().unwrap();
            st.cdp.take()
        };
        if let Some(old) = old {
            // Stop the old screencast cleanly before dropping the
            // connection: a lingering session can wedge the next
            // Page.startScreencast (observed hang on re-attach).
            let _ = tokio::time::timeout(
                Duration::from_secs(3),
                old.call("Page.stopScreencast", nop()),
            )
            .await;
            old.close().await;
        }
        let weak = Arc::downgrade(self);
        let handler: EventHandler = Arc::new(move |method, params| {
            if let Some(b) = weak.upgrade() {
                b.on_event(method, params);
            }
        });
        let origin = self.cdp_base.clone();
        let cdp = Cdp::dial(ws_url, &origin, handler).await?;
        let mobile = {
            let mut st = self.state.lock().unwrap();
            st.cdp = Some(cdp.clone());
            st.target_id = target_id.to_string();
            st.mobile
        };
        if let Err(e) = cdp.call("Page.enable", nop()).await {
            rlog!("Page.enable: {e}");
        }
        // The page may already be laid out: read its real size.
        self.read_page_size(&cdp).await;
        // A new tab starts with the plain window size: put the client's
        // viewport back.
        let wanted = self.state.lock().unwrap().viewport;
        if let Some((w, h, dpr)) = wanted {
            let _ = self.set_viewport(w, h, dpr).await;
        }
        // Chrome without the anti-backgrounding flags stops painting
        // when another window covers it: the screen would freeze.
        let flags = crate::chrome::chrome_has_relay_flags();
        if self.hub.set_flags_ok(flags) {
            self.hub.broadcast_status();
        }
        if mobile {
            let (w, h) = {
                let st = self.state.lock().unwrap();
                (st.mobile_w, st.mobile_h)
            };
            let _ = self.apply_emulation(&cdp, true, w, h).await;
        }
        self.start_screencast(&cdp).await;
        // Screencast only emits on repaint; send one still immediately.
        let mut shot_params = json!({"format": "jpeg", "quality": self.sc.quality});
        if let Some(clip) = self.clip() {
            shot_params["clip"] = clip;
        }
        if let Ok(shot) = cdp.call("Page.captureScreenshot", shot_params).await {
            if let Some(data) = shot.get("data").and_then(|d| d.as_str()) {
                if !data.is_empty() {
                    self.hub.broadcast_frame(data);
                }
            }
        }
        rlog!("attached to tab {target_id}");
        Ok(())
    }

    fn on_event(self: &Arc<Self>, method: String, params: Value) {
        match method.as_str() {
            "Page.screencastFrame" => {
                let Some(data) = params.get("data").and_then(|d| d.as_str()) else {
                    return;
                };
                // The frame tells us how big the page it shows is.
                if let Some(meta) = params.get("metadata") {
                    let w = meta.get("deviceWidth").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let h = meta.get("deviceHeight").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    if w > 0.0 && h > 0.0 {
                        let mut st = self.state.lock().unwrap();
                        st.frame_w = w;
                        st.frame_h = h;
                    }
                }
                let data = data.to_string();
                let session = params.get("sessionId").cloned().unwrap_or(Value::Null);
                let cdp = self.state.lock().unwrap().cdp.clone();
                if let Some(cdp) = cdp {
                    tokio::spawn(async move {
                        cdp.notify(
                            "Page.screencastFrameAck",
                            json!({"sessionId": session}),
                        )
                        .await;
                    });
                }
                self.hub.broadcast_frame(&data);
            }
            "Page.loadEventFired" => {
                // A new page: its size can be different.
                if let Some(cdp) = self.state.lock().unwrap().cdp.clone() {
                    let b2 = self.clone();
                    tokio::spawn(async move { b2.read_page_size(&cdp).await; });
                }
                let b = self.clone();
                tokio::spawn(async move {
                    b.push_tabs().await;
                });
            }
            _ => {}
        }
    }

    pub fn current_target(&self) -> String {
        self.state.lock().unwrap().target_id.clone()
    }

    /// Broadcast one fresh still of the attached tab. Called when a
    /// phone finishes auth: the attach-time still may have gone out
    /// before anyone was listening, and a static page emits no
    /// screencast frames — a new client would stare at black.
    pub async fn send_still(&self) {
        let Some(cdp) = self.cdp() else { return };
        let mut shot_params = json!({"format": "jpeg", "quality": self.sc.quality});
        if let Some(clip) = self.clip() {
            shot_params["clip"] = clip;
        }
        if let Ok(shot) = cdp.call("Page.captureScreenshot", shot_params).await {
            if let Some(data) = shot.get("data").and_then(|d| d.as_str()) {
                if !data.is_empty() {
                    self.hub.broadcast_frame(data);
                }
            }
        }
    }

    fn cdp(&self) -> Option<Arc<Cdp>> {
        self.state.lock().unwrap().cdp.clone()
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let cdp = self.cdp().ok_or("no tab attached")?;
        cdp.call(method, params).await
    }

    // ---- input ----

    /// Read the real layout size of the page we drive.
    ///
    /// The emulation we ask for is not always what the browser ends
    /// up using (a window that cannot shrink, a page that overrides
    /// the viewport). The page itself is the only trustworthy source.
    async fn read_page_size(&self, cdp: &Arc<Cdp>) {
        let Ok(res) = cdp
            .call(
                "Runtime.evaluate",
                json!({"expression": "[window.innerWidth, window.innerHeight]", "returnByValue": true}),
            )
            .await
        else {
            return;
        };
        let Some(v) = res
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_array())
        else {
            return;
        };
        let w = v.first().and_then(|x| x.as_f64()).unwrap_or(0.0);
        let h = v.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0);
        if w > 0.0 && h > 0.0 {
            let mut st = self.state.lock().unwrap();
            st.page_w = w;
            st.page_h = h;
        }
    }

    /// The clip region for the screencast: the page's own viewport.
    ///
    /// `setDeviceMetricsOverride` does not resize the OS window, so the
    /// frame would be the size of the window with the page sitting in
    /// a corner of it: black bars, a scaled-down page, soft text.
    /// Clipping the capture to the page makes the frame the page.
    fn clip(&self) -> Option<Value> {
        let st = self.state.lock().unwrap();
        if st.page_w > 0.0 && st.page_h > 0.0 {
            Some(json!({
                "x": 0, "y": 0,
                "width": st.page_w, "height": st.page_h,
                "scale": 1,
            }))
        } else {
            None
        }
    }

    /// (Re)start the screencast, clipped to the page.
    async fn start_screencast(&self, cdp: &Arc<Cdp>) {
        let mut params = json!({
            "format": "jpeg",
            "quality": self.sc.quality,
            "maxWidth": self.sc.max_w,
            "maxHeight": self.sc.max_h,
            "everyNthFrame": self.sc.every_nth,
        });
        if let Some(clip) = self.clip() {
            params["clip"] = clip;
        }
        if let Err(e) = cdp.call("Page.startScreencast", params).await {
            rlog!("startScreencast: {e}");
        }
    }

    /// Make the attached tab the active one.
    ///
    /// A tab that is not the active one has
    /// `document.visibilityState == "hidden"`, and a hidden page does
    /// not paint: the screencast freezes and the client stares at an
    /// old frame. Activating the tab brings the screen back.
    pub async fn bring_to_front(&self) -> Result<(), String> {
        let Some(cdp) = self.state.lock().unwrap().cdp.clone() else {
            return Err("no tab attached".into());
        };
        cdp.call("Page.bringToFront", nop())
            .await
            .map(|_| ())
    }

    /// Is the page in the foreground?
    ///
    /// A minimized or covered window stops painting, so the screencast
    /// freezes and the phone keeps looking at an old screen. The page
    /// itself knows when it is hidden.
    pub async fn is_hidden(&self) -> bool {
        let Some(cdp) = self.state.lock().unwrap().cdp.clone() else {
            return false;
        };
        let Ok(res) = cdp
            .call(
                "Runtime.evaluate",
                json!({"expression": "document.visibilityState", "returnByValue": true}),
            )
            .await
        else {
            return false;
        };
        res.get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .map(|s| s == "hidden")
            .unwrap_or(false)
    }

    /// Frame pixels -> CSS pixels of the page.
    ///
    /// The client sends the position inside the image it shows, which
    /// is the screencast frame. That frame is not always the size of
    /// the page the browser laid out: a window bigger than the
    /// emulated viewport shows the page scaled up. Dividing by the
    /// device pixel factor alone then puts every click the wrong
    /// place, so the real page size and the real frame size are used.
    fn to_css(&self, x: f64, y: f64) -> (f64, f64) {
        let st = self.state.lock().unwrap();
        let (page_w, page_h) = (st.page_w, st.page_h);
        let (frame_w, frame_h) = (st.frame_w, st.frame_h);
        let dsf = st.dsf;
        // The frame is the page in device pixels, capped by the
        // screencast limits.
        let px_w = if frame_w > 0.0 { (frame_w * dsf).min(self.sc.max_w as f64) } else { 0.0 };
        let px_h = if frame_h > 0.0 { (frame_h * dsf).min(self.sc.max_h as f64) } else { 0.0 };
        if page_w > 0.0 && px_w > 0.0 {
            (x * page_w / px_w, y * page_h / px_h)
        } else if dsf > 0.0 {
            (x / dsf, y / dsf)
        } else {
            (x, y)
        }
    }

    pub async fn mouse(&self, action: &str, x: f64, y: f64, button: &str) -> Result<(), String> {
        let t = match action {
            "move" => "mouseMoved",
            "down" => "mousePressed",
            "up" => "mouseReleased",
            _ => return Err(format!("bad mouse action {action:?}")),
        };
        let button = if ["left", "middle", "right"].contains(&button) { button } else { "left" };
        // Client sends frame pixels; CDP expects the page's CSS pixels.
        let (cx, cy) = self.to_css(x, y);
        self.call(
            "Input.dispatchMouseEvent",
            json!({"type": t, "x": cx, "y": cy, "button": button, "clickCount": 1}),
        )
        .await?;
        Ok(())
    }

    pub async fn wheel(&self, x: f64, y: f64, dx: f64, dy: f64) -> Result<(), String> {
        let (cx, cy) = self.to_css(x, y);
        self.call(
            "Input.dispatchMouseEvent",
            json!({"type": "mouseWheel", "x": cx, "y": cy, "deltaX": dx, "deltaY": dy}),
        )
        .await?;
        Ok(())
    }

    pub async fn insert_text(&self, text: &str) -> Result<(), String> {
        self.call("Input.insertText", json!({"text": text})).await?;
        Ok(())
    }

    pub async fn key_code(&self, vk: i64, down: bool) -> Result<(), String> {
        let t = if down { "rawKeyDown" } else { "keyUp" };
        self.call(
            "Input.dispatchKeyEvent",
            json!({"type": t, "windowsVirtualKeyCode": vk}),
        )
        .await?;
        Ok(())
    }

    // ---- tabs ----

    pub async fn activate_tab(self: &Arc<Self>, id: &str) -> Result<(), String> {
        self.http_get(&format!("/json/activate/{id}")).await?;
        self.attach(id).await
    }

    pub async fn new_tab(self: &Arc<Self>, raw_url: &str) -> Result<(), String> {
        let url = normalize_url(raw_url);
        let res = self.call("Target.createTarget", json!({"url": url})).await?;
        let tid = res
            .get("targetId")
            .and_then(|t| t.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("createTarget returned no targetId")?;
        let tid = tid.to_string();
        self.push_tabs().await;
        self.attach(&tid).await
    }

    pub async fn close_tab(self: &Arc<Self>, id: &str) -> Result<(), String> {
        self.http_get(&format!("/json/close/{id}")).await?;
        let mine = self.state.lock().unwrap().target_id == id;
        if mine {
            // Chrome removes the target asynchronously: /json/list
            // can still contain the just-closed tab for a moment.
            // Never pick it as the replacement.
            let pages: Vec<PageTarget> = self
                .list_targets()
                .await?
                .into_iter()
                .filter(|p| p.id != id)
                .collect();
            if pages.is_empty() {
                return self.new_tab("about:blank").await;
            }
            return self.attach(&pages[0].id).await;
        }
        self.push_tabs().await;
        Ok(())
    }

    /// History navigation (Back/Forward tools): move `delta` steps
    /// in the current tab's navigation history via
    /// Page.getNavigationHistory + Page.navigateToHistoryEntry.
    pub async fn history_go(&self, delta: i32) -> Result<(), String> {
        if delta == 0 {
            return Ok(());
        }
        let res = self.call("Page.getNavigationHistory", nop()).await?;
        let entries = res
            .get("entries")
            .and_then(|e| e.as_array())
            .ok_or("history: no entries")?;
        let cur: i64 = res
            .get("currentIndex")
            .and_then(|i| i.as_i64())
            .ok_or("history: no currentIndex")?;
        let want = cur + delta as i64;
        let entry = entries
            .get(want as usize)
            .and_then(|e| e.get("id"))
            .and_then(|i| i.as_i64())
            .ok_or("history: out of range")?;
        self.call("Page.navigateToHistoryEntry", json!({"entryId": entry}))
            .await?;
        Ok(())
    }

    /// Reload the current tab (tools-sheet reload button).
    pub async fn reload_page(&self) -> Result<(), String> {
        self.call("Page.reload", nop()).await?;
        Ok(())
    }

    /// Find-in-page: highlight all matches of `query`
    /// (DOM.performSearch). Cleared by the next search/navigation.
    pub async fn find_in_page(&self, query: &str) -> Result<i64, String> {
        // performSearch needs the DOM agent enabled on the session.
        let _ = self.call("DOM.enable", nop()).await;
        let res = self
            .call(
                "DOM.performSearch",
                json!({"query": query, "includeUserAgentShadowDOM": true}),
            )
            .await?;
        res.get("resultCount")
            .and_then(|n| n.as_i64())
            .ok_or("find: no resultCount".into())
    }

    pub async fn push_tabs(self: &Arc<Self>) {
        let pages = match self.list_targets().await {
            Ok(p) => p,
            Err(_) => return,
        };
        let cur = {
            let mut st = self.state.lock().unwrap();
            st.last_tabs = pages.clone();
            st.target_id.clone()
        };
        self.hub.broadcast_tabs(&pages, &cur);
    }

    // ---- background loops ----

    /// Follow tab changes made on the laptop itself; re-attach when
    /// our tab vanishes. (Go: Browser.Watch, 3 s.)
    pub async fn watch(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(3));
        loop {
            tick.tick().await;
            let pages = match self.list_targets().await {
                Ok(p) => p,
                Err(_) => continue,
            };
            let (cur, changed, alive, new_tab) = {
                let mut st = self.state.lock().unwrap();
                let cur = st.target_id.clone();
                let mut changed = pages.len() != st.last_tabs.len();
                if !changed {
                    for (a, b) in pages.iter().zip(st.last_tabs.iter()) {
                        if a.id != b.id || a.title != b.title || a.url != b.url {
                            changed = true;
                            break;
                        }
                    }
                }
                let alive = pages.iter().any(|p| p.id == cur);
                // Detect a newly opened tab (not in the previous list).
                // Follow it — the user likely opened it to view it.
                let new_tab = pages
                    .iter()
                    .find(|p| !st.last_tabs.iter().any(|q| q.id == p.id))
                    .map(|p| p.id.clone());
                st.last_tabs = pages.clone();
                (cur, changed, alive, new_tab)
            };
            if changed {
                self.hub.broadcast_tabs(&pages, &cur);
            }
            if !alive {
                if !pages.is_empty() {
                    rlog!("attached tab closed on laptop, following to {}", pages[0].id);
                    let _ = self.attach(&pages[0].id).await;
                } else {
                    let _ = self.new_tab("about:blank").await;
                }
            } else if let Some(id) = new_tab {
                // A new tab was opened (e.g., user opened YouTube on the PC).
                // Follow it so the phone shows what the user is looking at.
                if id != cur {
                    rlog!("new tab opened, following to {}", id);
                    let _ = self.attach(&id).await;
                }
            }
        }
    }

    /// Attach the first tab whenever Chrome shows up and nothing is
    /// attached yet. (Go: maintainBrowser, 2 s.)
    pub async fn maintain(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            self.refresh_orig_ua().await;
            if self.current_target().is_empty() && self.is_chrome_up().await {
                if let Ok(pages) = self.list_targets().await {
                    if let Some(first) = pages.first() {
                        let _ = self.attach(&first.id).await;
                    }
                }
            }
            // Chrome in the background: the screen is frozen, so say so
            // instead of letting the phone look at a stale frame.
            if !self.current_target().is_empty() {
                let hidden = self.is_hidden().await;
                if self.hub.set_paused(hidden) {
                    self.hub.broadcast_paused();
                }
            }
        }
    }

    // ---- mobile emulation ----

    pub fn is_mobile(&self) -> bool {
        self.state.lock().unwrap().mobile
    }

    pub async fn set_mobile(&self, on: bool, width: u32, height: u32) -> Result<(), String> {
        let cdp = {
            let mut st = self.state.lock().unwrap();
            st.mobile = on;
            st.dsf = if on { 2.0 } else { 1.0 };
            if on {
                st.mobile_w = width;
                st.mobile_h = height;
            }
            st.cdp.clone()
        };
        let cdp = cdp.ok_or("no tab attached")?;
        self.apply_emulation(&cdp, on, width, height).await
    }

    /// Render the page at the client's viewport size.
    ///
    /// The screencast then matches the client 1:1: the remote page
    /// fills the view without letterbox bars and the text stays as
    /// sharp as the screen it is shown on.
    pub async fn set_viewport(&self, width: u32, height: u32, dpr: f64) -> Result<(), String> {
        let (cdp, mobile) = {
            let mut st = self.state.lock().unwrap();
            // Remember the request even when no tab is attached yet:
            // otherwise it is lost and the page keeps the size of the
            // window until the client asks again.
            st.viewport = Some((width, height, dpr));
            (st.cdp.clone(), st.mobile)
        };
        let cdp = cdp.ok_or("no tab attached")?;
        if mobile {
            // The phone view keeps its own metrics (touch, mobile UA).
            return self.apply_emulation(&cdp, true, width, height).await;
        }
        let w = width.clamp(320, 1920);
        let h = height.clamp(320, 1600);
        // Render at 2x only when the client really has a HiDPI screen,
        // and keep the frame inside the screencast bounds.
        let dsf = if dpr >= 1.5 { 2.0 } else { 1.0 };
        // The mouse mapping divides the frame pixels by this, so it has
        // to match what we actually render with - otherwise every click
        // lands twice as far from where it was aimed.
        {
            let mut st = self.state.lock().unwrap();
            st.dsf = dsf;
            st.viewport = Some((w, h, dpr));
        }
        cdp.call(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": w, "height": h, "deviceScaleFactor": dsf, "mobile": false}),
        )
        .await?;
        self.read_page_size(&cdp).await;
        // The clip follows the page size: restart the capture.
        let cdp2 = self.state.lock().unwrap().cdp.clone();
        if let Some(cdp) = cdp2 {
            let _ = cdp.call("Page.stopScreencast", nop()).await;
            self.start_screencast(&cdp).await;
        }
        Ok(())
    }

    async fn apply_emulation(
        &self,
        cdp: &Arc<Cdp>,
        on: bool,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if on {
            // Use the client's actual viewport size (clamped to sane bounds).
            // deviceScaleFactor=2 for sharper rendering on high-DPI phones.
            // The screencast max (1280x1700) accommodates the 2x pixels.
            let w = width.clamp(320, 1920);
            let h = height.clamp(480, 1080);
            cdp.call("Network.setUserAgentOverride", json!({"userAgent": MOBILE_UA}))
                .await?;
            cdp.call(
                "Emulation.setDeviceMetricsOverride",
                json!({"width": w, "height": h, "deviceScaleFactor": 2, "mobile": true}),
            )
            .await?;
            cdp.call("Emulation.setTouchEmulationEnabled", json!({"enabled": true}))
                .await?;
            return Ok(());
        }
        // Desktop: render the page at the size of the client view, so
        // the screencast is shown 1:1. Clearing the override would
        // fall back to the real window and letterbox the page again -
        // and it also wiped the viewport the client had asked for.
        let w = width.clamp(320, 1920);
        let h = height.clamp(320, 1600);
        self.state.lock().unwrap().dsf = 1.0;
        cdp.call(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": w, "height": h, "deviceScaleFactor": 1, "mobile": false}),
        )
        .await?;
        self.read_page_size(&cdp).await;
        cdp.call("Emulation.setTouchEmulationEnabled", json!({"enabled": false}))
            .await?;
        let ua = self.state.lock().unwrap().orig_ua.clone();
        if ua.is_empty() {
            return Ok(());
        }
        cdp.call("Network.setUserAgentOverride", json!({"userAgent": ua}))
            .await?;
        Ok(())
    }
}

/// Best-effort absolute URL out of user input (port of normalizeURL).
pub fn normalize_url(raw: &str) -> String {
    let u = raw.trim();
    if u.is_empty() {
        return "about:blank".to_string();
    }
    if !u.contains("://") {
        return format!("https://{u}");
    }
    u.to_string()
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn decode_chunked(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut rest = data;
    loop {
        let eol = find_subslice(rest, b"\r\n").ok_or("chunked: bad size line")?;
        let size_str = String::from_utf8_lossy(&rest[..eol]).into_owned();
        let size = usize::from_str_radix(size_str.trim().split(';').next().unwrap_or("0"), 16)
            .map_err(|_| "chunked: bad size")?;
        rest = &rest[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if rest.len() < size {
            return Err("chunked: truncated".into());
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size..];
        if rest.starts_with(b"\r\n") {
            rest = &rest[2..];
        }
    }
}
