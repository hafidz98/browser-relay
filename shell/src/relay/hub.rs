//! Hub multiplexes phone clients: token auth, one driver at a time
//! (others are view-only), message routing to the Browser.
//! A port of the Go core's hub.go — same message schema, same error
//! strings, same promotion and slow-client drop policies.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::browser::{Browser, PageTarget};
use super::clipboard;
use super::pairing::PairingManager;
use crate::chrome;
use crate::rlog;

enum OutMsg {
    Text(String),
    /// Flush everything queued so far, then close (bad-token path:
    /// the error must land before the connection drops).
    CloseAfterFlush,
}

struct ClientState {
    send: mpsc::Sender<OutMsg>,
    /// Queued-but-unsent messages, tracked so frame broadcasts can
    /// leave headroom for control messages (tabs/role/errors) —
    /// a queue stuffed with JPEG frames would otherwise starve them.
    queued: Arc<AtomicUsize>,
    authed: bool,
    seq: u64,
    peer: String,
    connected_at: u64,
    /// Paired-device fingerprint (set on auth).
    fingerprint: String,
    /// True after a successful pair_request — the client is in the
    /// pairing flow, so the auth deadline must not drop them.
    pairing_active: bool,
}

/// Snapshot of one connected phone for the panel's client list.
#[derive(Clone)]
pub struct ClientInfo {
    pub id: u64,
    pub peer: String,
    pub authed: bool,
    pub driver: bool,
    pub connected_at: u64,
    pub fingerprint: String,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Frames never occupy more than this many of the 8 queue slots.
const FRAME_QUEUE_CAP: usize = 4;

struct HubState {
    clients: HashMap<u64, ClientState>,
    /// The page is not in the foreground: the screencast is frozen.
    paused: bool,
    /// Whether the running Chrome was started with the flags that keep
    /// the screen alive (None: no Chrome to judge).
    flags_ok: Option<bool>,
    driver: Option<u64>,
    seq: u64,
}

pub struct Hub {
    pairing: PairingManager,
    state: Mutex<HubState>,
    browser: RwLock<Option<Arc<Browser>>>,
    /// Whether the laptop browser is up + attached (panel semantics).
    /// Phones get it in the welcome message and via status broadcasts
    /// so they can say "Chrome isn't running" instead of showing a
    /// black screen.
    browser_up: std::sync::atomic::AtomicBool,
}

impl Hub {
    pub fn new(config_dir: &std::path::Path, fixed_code: Option<String>) -> Arc<Hub> {
        Arc::new(Hub {
            pairing: PairingManager::new(config_dir, fixed_code),
            state: Mutex::new(HubState {
                clients: HashMap::new(),
                paused: false,
                flags_ok: None,
                driver: None,
                seq: 0,
            }),
            browser: RwLock::new(None),
            browser_up: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Current single-use pairing code (panel QR / display).
    pub fn pair_code(&self) -> String {
        self.pairing.code()
    }

    pub fn pairing(&self) -> &PairingManager {
        &self.pairing
    }

    pub fn browser_up(&self) -> bool {
        self.browser_up.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Update the browser-up flag; returns true when it changed.
    pub fn set_browser_up(&self, up: bool) -> bool {
        self.browser_up
            .swap(up, std::sync::atomic::Ordering::Relaxed)
            != up
    }

    /// Remember whether the running Chrome has the flags that keep the
    /// screen alive.
    pub fn set_flags_ok(&self, ok: Option<bool>) -> bool {
        let mut st = self.state.lock().unwrap();
        if st.flags_ok == ok {
            return false;
        }
        st.flags_ok = ok;
        true
    }

    /// The page is not in the foreground (minimized or covered): the
    /// screencast is frozen, so the client shows what it has.
    pub fn set_paused(&self, paused: bool) -> bool {
        let mut st = self.state.lock().unwrap();
        if st.paused == paused {
            return false;
        }
        st.paused = paused;
        true
    }

    /// Tell every authed client that the screen is frozen.
    pub fn broadcast_paused(&self) {
        let msg = json!({"type": "paused", "paused": self.state.lock().unwrap().paused}).to_string();
        let st = self.state.lock().unwrap();
        for c in st.clients.values() {
            if c.authed && c.send.try_send(OutMsg::Text(msg.clone())).is_ok() {
                c.queued.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Tell every authed client the browser went up or down.
    pub fn broadcast_status(&self) {
        let msg = json!({
            "type": "status",
            "browserUp": self.browser_up(),
            "paused": self.state.lock().unwrap().paused,
            "flagsOk": self.state.lock().unwrap().flags_ok,
        })
        .to_string();
        let st = self.state.lock().unwrap();
        for c in st.clients.values() {
            if c.authed && c.send.try_send(OutMsg::Text(msg.clone())).is_ok() {
                c.queued.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub fn set_browser(&self, b: Arc<Browser>) {
        *self.browser.write().unwrap() = Some(b);
    }

    fn browser(&self) -> Option<Arc<Browser>> {
        self.browser.read().unwrap().clone()
    }

    /// Count of AUTHENTICATED clients only (paired devices online).
    /// Unauthenticated WS connections (mid-pairing, probing) are excluded.
    pub fn authed_client_count(&self) -> i64 {
        self.state
            .lock()
            .unwrap()
            .clients
            .values()
            .filter(|c| c.authed)
            .count() as i64
    }

    pub fn client_count(&self) -> i64 {
        self.state.lock().unwrap().clients.len() as i64
    }

    /// Snapshot of connected clients for the panel's "paired clients" list.
    pub fn client_list(&self) -> Vec<ClientInfo> {
        let st = self.state.lock().unwrap();
        st.clients
            .values()
            .map(|c| ClientInfo {
                id: c.seq,
                peer: c.peer.clone(),
                authed: c.authed,
                driver: st.driver == Some(c.seq),
                connected_at: c.connected_at,
                fingerprint: c.fingerprint.clone(),
            })
            .collect()
    }

    // ---- connection lifecycle ----

    pub fn add_client(self: &Arc<Self>, ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) {
        let (tx, mut rx) = mpsc::channel::<OutMsg>(8);
        let queued = Arc::new(AtomicUsize::new(0));
        let peer = ws
            .get_ref()
            .peer_addr()
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|_| "?".to_string());
        let now = unix_now();
        let id = {
            let mut st = self.state.lock().unwrap();
            st.seq += 1;
            let id = st.seq;
            st.clients.insert(
                id,
                ClientState {
                    send: tx,
                    queued: queued.clone(),
                    authed: false,
                    seq: id,
                    peer,
                    connected_at: now,
                    fingerprint: String::new(),
                    pairing_active: false,
                },
            );
            id
        };
        let (mut sink, mut source) = ws.split();

        // writer pump
        let hub = self.clone();
        let queued_w = queued.clone();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                queued_w.fetch_sub(1, Ordering::Relaxed);
                match msg {
                    OutMsg::Text(t) => {
                        if sink.send(Message::Text(t.into())).await.is_err() {
                            break;
                        }
                    }
                    OutMsg::CloseAfterFlush => {
                        let _ = sink.flush().await;
                        let _ = sink.close().await;
                        break;
                    }
                }
            }
            let _ = sink.close().await;
            hub.remove(id);
        });

        // reader pump
        let hub = self.clone();
        tokio::spawn(async move {
            while let Some(item) = source.next().await {
                match item {
                    Ok(Message::Text(t)) => {
                        hub.handle_message(id, &t).await;
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => continue,
                }
            }
            hub.remove(id);
        });

        // auth deadline: 15 s to send the auth message (unless the client
        // is in the pairing flow — they need time to read+type the code).
        let hub = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let drop = hub
                .state
                .lock()
                .unwrap()
                .clients
                .get(&id)
                .map(|c| !c.authed && !c.pairing_active)
                .unwrap_or(false);
            if drop {
                hub.remove(id);
            }
        });
    }

    fn remove(&self, id: u64) {
        let (promoted, was_present) = {
            let mut st = self.state.lock().unwrap();
            if st.clients.remove(&id).is_none() {
                return;
            }
            let mut promoted = None;
            if st.driver == Some(id) {
                st.driver = None;
                let mut best: Option<(u64, u64)> = None; // (seq, id)
                for (cid, c) in st.clients.iter() {
                    if c.authed && best.map(|(s, _)| c.seq < s).unwrap_or(true) {
                        best = Some((c.seq, *cid));
                    }
                }
                if let Some((_, nid)) = best {
                    st.driver = Some(nid);
                    promoted = Some(nid);
                }
            }
            (promoted, true)
        };
        let _ = was_present;
        if let Some(nid) = promoted {
            rlog!("driver disconnected, promoted client #{nid}");
            self.send_role(nid);
            self.broadcast_roles();
        }
    }

    // ---- sending helpers (Go: sendJSON/sendErr, drop-on-full) ----

    fn send_text(&self, id: u64, text: String) {
        let st = self.state.lock().unwrap();
        if let Some(c) = st.clients.get(&id) {
            if c.send.try_send(OutMsg::Text(text)).is_ok() {
                c.queued.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn send_json(&self, id: u64, v: Value) {
        self.send_text(id, v.to_string());
    }

    fn send_err(&self, id: u64, msg: &str) {
        self.send_json(id, json!({"type": "error", "message": msg}));
    }

    /// Auth failure: queue the error + close-after-flush so it lands
    /// before the connection drops. NOTE: "unknown device" does NOT
    /// close — the client needs the connection alive to request
    /// a pairing code.
    fn send_auth_error(&self, id: u64, msg: &str) {
        let st = self.state.lock().unwrap();
        if let Some(c) = st.clients.get(&id) {
            // Counted sends: the writer pump decrements `queued` for
            // every message it drains, so anything try_sent here must
            // increment too — otherwise the counter underflows to
            // usize::MAX and broadcast_frame permanently skips this
            // client (black screen after the "unknown device" flow).
            if c
                .send
                .try_send(OutMsg::Text(
                    json!({"type": "error", "message": msg}).to_string(),
                ))
                .is_ok()
            {
                c.queued.fetch_add(1, Ordering::Relaxed);
            }
            if msg != "unknown device" && c.send.try_send(OutMsg::CloseAfterFlush).is_ok() {
                c.queued.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Peer IP without the port (for pairing-request rate limiting).
    fn client_ip(&self, id: u64) -> String {
        let st = self.state.lock().unwrap();
        st.clients
            .get(&id)
            .map(|c| c.peer.split(':').next().unwrap_or("").to_string())
            .unwrap_or_default()
    }

    /// Drop every connection whose fingerprint was revoked.
    pub fn drop_revoked(&self, fingerprint: &str) {
        let ids: Vec<u64> = {
            let st = self.state.lock().unwrap();
            st.clients
                .iter()
                .filter(|(_, c)| c.fingerprint == fingerprint)
                .map(|(id, _)| *id)
                .collect()
        };
        for id in ids {
            self.send_auth_error(id, "device revoked");
        }
    }

    fn send_role(&self, id: u64) {
        let is_driver = {
            let st = self.state.lock().unwrap();
            st.driver == Some(id)
        };
        self.send_json(id, json!({"type": "role", "driver": is_driver}));
    }

    fn broadcast_roles(&self) {
        let ids: Vec<u64> = {
            let st = self.state.lock().unwrap();
            st.clients
                .iter()
                .filter(|(_, c)| c.authed)
                .map(|(id, _)| *id)
                .collect()
        };
        for id in ids {
            self.send_role(id);
        }
    }

    /// JPEG frame (base64) to every authed client; slow clients drop.
    /// Frames yield queue space to control messages (FRAME_QUEUE_CAP).
    pub fn broadcast_frame(&self, b64: &str) {
        let msg = json!({"type": "frame", "data": b64}).to_string();
        let st = self.state.lock().unwrap();
        for c in st.clients.values() {
            if c.authed && c.queued.load(Ordering::Relaxed) < FRAME_QUEUE_CAP {
                if c.send.try_send(OutMsg::Text(msg.clone())).is_ok() {
                    c.queued.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    pub fn broadcast_tabs(&self, pages: &[PageTarget], current: &str) {
        let tabs: Vec<Value> = pages
            .iter()
            .map(|p| {
                json!({
                    "id": p.id, "title": p.title, "url": p.url,
                    "active": p.id == current,
                })
            })
            .collect();
        let msg = json!({"type": "tabs", "tabs": tabs}).to_string();
        let st = self.state.lock().unwrap();
        for c in st.clients.values() {
            if c.authed && c.send.try_send(OutMsg::Text(msg.clone())).is_ok() {
                c.queued.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    // ---- message handling ----

    fn is_authed(&self, id: u64) -> bool {
        self.state
            .lock()
            .unwrap()
            .clients
            .get(&id)
            .map(|c| c.authed)
            .unwrap_or(false)
    }

    fn is_driver(&self, id: u64) -> bool {
        self.state.lock().unwrap().driver == Some(id)
    }

    async fn handle_message(self: &Arc<Self>, id: u64, payload: &str) {
        let m: ClientMsg = match serde_json::from_str(payload) {
            Ok(m) => m,
            Err(_) => return,
        };

        if !self.is_authed(id) {
            // Pre-auth pairing request (client-initiated flow): the code
            // is revealed on the PC panel, never sent to the requester.
            if m.typ == "pair_request" {
                let ip = self.client_ip(id);
                match self.pairing.request_code(&m.fingerprint, &m.name, &ip) {
                    Ok(_) => {
                        rlog!("pairing requested by {} ({ip})", m.name);
                        // Mark pairing active so the auth deadline doesn't
                        // drop the client while they're typing the code.
                        if let Some(c) = self.state.lock().unwrap().clients.get_mut(&id) {
                            c.pairing_active = true;
                        }
                        // No code in the reply — it appears on the panel.
                        self.send_json(id, json!({"type": "pair_wait"}));
                    }
                    Err(e) => {
                        self.send_json(id, json!({"type": "error", "message": e}));
                    }
                }
                return;
            }
            if m.typ != "auth" {
                self.send_err(id, "auth required");
                return;
            }
            // Auth: either a single-use pairing code (+ fingerprint), or a
            // fingerprint-only reconnect for an already-paired device.
            let fp = m.fingerprint.clone();
            let auth_ok = if !m.token.is_empty() {
                match self.pairing.try_pair(&m.token, &fp, &m.name) {
                    Ok(()) => true,
                    Err(e) => {
                        self.send_auth_error(id, e);
                        return;
                    }
                }
            } else if !fp.is_empty() && self.pairing.is_paired(&fp) {
                true
            } else if fp.is_empty() {
                // Legacy client without a fingerprint: fall back to the old
                // static-token check is gone; require pairing instead.
                self.send_auth_error(id, "unknown device");
                return;
            } else {
                self.send_auth_error(id, "unknown device");
                return;
            };
            if !auth_ok {
                return;
            }
            // Remember the fingerprint on the client for the device list.
            let is_driver = {
                let mut st = self.state.lock().unwrap();
                if let Some(c) = st.clients.get_mut(&id) {
                    c.authed = true;
                    c.fingerprint = fp.clone();
                    c.pairing_active = false;
                }
                if st.driver.is_none() {
                    st.driver = Some(id);
                }
                st.driver == Some(id)
            };
            rlog!("client #{id} authed (driver={is_driver})");
            let mobile = self.browser().map(|b| b.is_mobile()).unwrap_or(false);
            self.send_json(id, json!({
                "type": "welcome", "driver": is_driver, "mobile": mobile,
                "browserUp": self.browser_up(),
                "paused": self.state.lock().unwrap().paused,
                "flagsOk": self.state.lock().unwrap().flags_ok, "paired": true,
                // Which PC this is (the client shows it, so the phone
                // can tell what it is driving).
                "host": crate::chrome::host_name(),
            }));
            self.send_role(id);
            if let Some(b) = self.browser() {
                b.push_tabs().await;
                b.send_still().await;
            }
            return;
        }

        // Control messages require the driver role.
        let needs_driver = matches!(
            m.typ.as_str(),
            "mouse" | "wheel" | "key" | "tab" | "clipboard" | "emulation" | "history" | "find"
                | "chrome" | "viewport"
        );
        if needs_driver && !self.is_driver(id) {
            self.send_err(id, "view-only: another client is driving");
            return;
        }
        let Some(b) = self.browser() else {
            self.send_err(id, "browser unavailable");
            return;
        };

        match m.typ.as_str() {
            "mouse" => {
                if b.mouse(&m.action, m.x, m.y, &m.button).await.is_err() {
                    self.send_err(id, "mouse failed");
                }
            }
            "wheel" => {
                if b.wheel(m.x, m.y, m.dx, m.dy).await.is_err() {
                    self.send_err(id, "wheel failed");
                }
            }
            "key" => {
                let err = if m.kind == "text" && !m.text.is_empty() {
                    b.insert_text(&m.text).await.err()
                } else if m.kind == "code" && m.vk != 0 {
                    b.key_code(m.vk, m.down).await.err()
                } else {
                    self.send_err(id, "bad key message");
                    return;
                };
                if err.is_some() {
                    self.send_err(id, "key failed");
                }
            }
            "tab" => {
                let err = match m.action.as_str() {
                    "activate" => b.activate_tab(&m.target_id).await.err(),
                    "new" => b.new_tab(&m.url).await.err(),
                    "close" => b.close_tab(&m.target_id).await.err(),
                    "reload" => b.reload_page().await.err(),
                    _ => {
                        self.send_err(id, "bad tab action");
                        return;
                    }
                };
                if let Some(e) = err {
                    rlog!("tab action {} failed: {e}", m.action);
                    self.send_err(id, &format!("tab action failed: {e}"));
                }
            }
            "find" => match b.find_in_page(&m.text).await {
                Ok(n) => self.send_json(id, json!({"type": "find", "count": n})),
                Err(e) => self.send_err(id, &format!("find failed: {e}")),
            },
            // Remote Chrome lifecycle: launch/close the panel's
            // dedicated Chrome from the phone. Driver-only.
            "chrome" => {
                let action = m.action.as_str();
                if action == "restart" {
                    // Closing and starting Chrome takes seconds: do it
                    // off the async runtime.
                    let res = tokio::task::spawn_blocking(chrome::restart_chrome).await;
                    match res {
                        Ok(Ok(())) => {
                            rlog!("client #{id} chrome restart");
                            self.send_json(id, json!({"type": "chrome", "action": action}));
                        }
                        Ok(Err(e)) => self.send_err(id, &format!("chrome {action} failed: {e}")),
                        Err(e) => self.send_err(id, &format!("chrome {action} failed: {e}")),
                    }
                    return;
                }
                let res = match action {
                    "launch" => chrome::launch_chrome(),
                    "close" => chrome::close_chrome(),
                    "wake" => {
                        // The window may be minimized (restore it) and the
                        // tab may not be the active one (activate it).
                        let _ = chrome::wake_chrome();
                        if let Some(b) = self.browser() {
                            let _ = b.bring_to_front().await;
                        }
                        Ok(())
                    }
                    "restart" => chrome::restart_chrome(),
                    _ => {
                        self.send_err(id, "bad chrome action");
                        return;
                    }
                };
                match res {
                    Ok(()) => {
                        rlog!("client #{id} chrome {action}");
                        // Stopping Chrome has to show up everywhere: the
                        // panel said "Chrome connected" while the browser
                        // was already gone.
                        if action == "close" {
                            self.set_browser_up(false);
                            self.broadcast_status();
                        }
                        if action == "wake" {
                            // The screen was frozen: send a fresh frame
                            // so the client is not looking at the old one.
                            if let Some(b) = self.browser() {
                                b.bring_to_front().await.ok();
                                b.send_still().await;
                            }
                        }
                        self.send_json(id, json!({"type": "chrome", "action": action}))
                    }
                    Err(e) => self.send_err(id, &format!("chrome {action} failed: {e}")),
                }
            }
            "clipboard" => match m.action.as_str() {
                "get" => match clipboard::get() {
                    Ok(t) => self.send_json(id, json!({"type": "clipboard", "text": t})),
                    Err(e) => self.send_err(id, &format!("clipboard get failed: {e}")),
                },
                "set" => {
                    if let Err(e) = clipboard::set(&m.text) {
                        self.send_err(id, &format!("clipboard set failed: {e}"));
                    }
                }
                _ => self.send_err(id, "bad clipboard action"),
            },
            "emulation" => {
                let w = m.width.unwrap_or(390) as u32;
                let h = m.height.unwrap_or(844) as u32;
                if let Err(e) = b.set_mobile(m.mobile, w, h).await {
                    // No tab attached yet: the preference is already
                    // stored in Browser state and applies on the next
                    // attach — echoing the raw error at every fresh
                    // connect (Chrome down) leaked dev text to users.
                    if !e.contains("no tab attached") {
                        self.send_err(id, &format!("emulation failed: {e}"));
                        return;
                    }
                }
                self.send_json(id, json!({"type": "emulation", "mobile": m.mobile}));
            }
            "history" => {
                let delta = match m.action.as_str() {
                    "back" => -1,
                    "forward" => 1,
                    _ => {
                        self.send_err(id, "bad history action");
                        return;
                    }
                };
                if let Err(e) = b.history_go(delta).await {
                    self.send_err(id, &format!("history failed: {e}"));
                }
            }
            "viewport" => {
                // The client tells us how big its view is. Rendering the
                // page at that size makes the screencast match 1:1: no
                // letterbox, no rescaling blur, and folding the tool
                // rail only reflows the page - it does not zoom it.
                let w = m.width.unwrap_or(1280) as u32;
                let h = m.height.unwrap_or(800) as u32;
                let dpr = m.dpr.unwrap_or(1.0);
                if let Err(e) = b.set_viewport(w, h, dpr).await {
                    // No tab attached yet: the size is applied on the
                    // next attach, so this is not an error the user
                    // should see.
                    if !e.contains("no tab attached") {
                        self.send_err(id, &format!("viewport failed: {e}"));
                        return;
                    }
                }
            }
            "ping" => {
                self.send_json(id, json!({"type": "pong", "t": m.t}));
            }
            _ => self.send_err(id, "unknown message type"),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ClientMsg {
    #[serde(rename = "type")]
    typ: String,
    token: String,
    /// Stable device identity (client-generated UUID). Required for pairing,
    /// and the reconnect credential for paired devices.
    fingerprint: String,
    /// Human-readable device name (client-supplied, shown on the panel).
    name: String,
    action: String,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    button: String,
    kind: String,
    text: String,
    vk: i64,
    down: bool,
    #[serde(rename = "targetId")]
    target_id: String,
    url: String,
    mobile: bool,
    width: Option<i64>,
    height: Option<i64>,
    /// Device pixel ratio of the client view.
    dpr: Option<f64>,
    t: i64,
}
