//! Phone-facing HTTP server: the web client (two static files), the
//! WebSocket endpoint, the QR pairing landing page (/pair), and the
//! Android app download (/app.apk, served when an APK file is present
//! in the config dir). Routing peeks at the request line without
//! consuming it, so /ws can hand the untouched stream to the
//! WebSocket handshake.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::hub::Hub;
use crate::chrome;
use crate::rlog;

const INDEX_HTML: &[u8] = include_bytes!("../../assets/client/index.html");
const APP_JS: &[u8] = include_bytes!("../../assets/client/app.js");

pub async fn serve(listener: TcpListener, hub: Arc<Hub>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let hub = hub.clone();
                tokio::spawn(async move {
                    handle(stream, hub).await;
                });
            }
            Err(e) => {
                rlog!("accept: {e}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn handle(mut stream: TcpStream, hub: Arc<Hub>) {
    // Peek until the end of the first request line (never consume:
    // the WS handshake must read the request itself).
    let mut buf = vec![0u8; 8192];
    let mut filled = 0usize;
    for _ in 0..16 {
        match stream.peek(&mut buf).await {
            Ok(n) if n > 0 => {
                filled = n;
                if find_subslice(&buf[..n], b"\r\n").is_some() {
                    break;
                }
            }
            _ => break,
        }
        if filled >= buf.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    if filled == 0 {
        return;
    }
    let head = String::from_utf8_lossy(&buf[..filled]).into_owned();
    let first_line = head.lines().next().unwrap_or("").to_string();
    let mut parts = first_line.split_whitespace();
    let _method = parts.next().unwrap_or("");
    let raw_path = parts.next().unwrap_or("/").to_string();
    let (path, query) = match raw_path.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (raw_path, String::new()),
    };

    // Drain the HTTP request headers before responding (non-WS paths).
    // We used peek() above, so the request bytes are still in the socket's
    // receive buffer. If we respond and close with unread data pending,
    // the OS sends TCP RST instead of FIN, and the client may discard our
    // response — observed as blank pages on Android Chrome for the larger
    // static files (22KB HTML, 27KB JS), while small pages worked.
    if path != "/ws" {
        let mut tmp = [0u8; 4096];
        let mut acc: Vec<u8> = Vec::with_capacity(8192);
        loop {
            match tokio::time::timeout(
                Duration::from_millis(500),
                stream.read(&mut tmp),
            )
            .await
            {
                Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                Ok(Ok(n)) => {
                    acc.extend_from_slice(&tmp[..n]);
                    if find_subslice(&acc, b"\r\n\r\n").is_some() || acc.len() > 16384 {
                        break;
                    }
                }
            }
        }
    }

    match path.as_str() {
        "/" => {
            // The client opens on its beranda (home) view - the remote
            // screen is one tap away from there.
            respond(&mut stream, "200 OK", "text/html; charset=utf-8", INDEX_HTML).await;
        }
        "/app.js" => {
            respond(&mut stream, "200 OK", "text/javascript; charset=utf-8", APP_JS).await;
        }
        "/ws" => {
            match tokio_tungstenite::accept_async(stream).await {
                Ok(ws) => hub.add_client(ws),
                Err(e) => rlog!("ws upgrade: {e}"),
            }
        }
        "/pair" => {
            // The landing page says NOTHING about whether the code is
            // the current one: it renders the same page for every
            // code, so a script cannot tell a live code from a dead
            // one with a single request. The phone app (or the web
            // client) reports whether the code worked.
            let code = query_param(&query, "c");
            let base = stream
                .local_addr()
                .map(|a| format!("http://{}", a))
                .unwrap_or_default();
            let apk_available = chrome::config_dir().join("browser-relay.apk").exists();
            let html = pair_page(code.as_deref(), &base, apk_available);
            respond(&mut stream, "200 OK", "text/html; charset=utf-8", html.as_bytes()).await;
        }
        "/app.apk" => {
            // Any browser-relay*.apk in the config folder works, so the
            // file can carry its version (browser-relay-2.6.apk). The
            // phone then saves it under that name.
            match newest_apk().await {
                Some((name, bytes)) => {
                    respond_download(
                        &mut stream,
                        "200 OK",
                        "application/vnd.android.package-archive",
                        &bytes,
                        &name,
                    )
                    .await;
                }
                None => {
                    respond(&mut stream, "404 Not Found", "text/plain; charset=utf-8", b"app not available yet")
                        .await;
                }
            }
        }
        _ => {
            respond(&mut stream, "404 Not Found", "text/plain; charset=utf-8", b"not found").await;
        }
    }
}

/// The newest browser-relay*.apk in the config folder, with its name.
async fn newest_apk() -> Option<(String, Vec<u8>)> {
    let dir = chrome::config_dir();
    let mut entries = tokio::fs::read_dir(&dir).await.ok()?;
    let mut best: Option<(std::time::SystemTime, String)> = None;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("browser-relay") || !name.ends_with(".apk") {
            continue;
        }
        let modified = entry
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
            best = Some((modified, name));
        }
    }
    let (_, name) = best?;
    let bytes = tokio::fs::read(dir.join(&name)).await.ok()?;
    Some((name, bytes))
}

/// Answer with a file the browser saves under the given name.
async fn respond_download(
    stream: &mut TcpStream,
    status: &str,
    ctype: &str,
    body: &[u8],
    filename: &str,
) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Disposition: attachment; filename=\"{filename}\"\r\nContent-Length: {}\r\nCache-Control: no-store, no-cache, must-revalidate\r\nPragma: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    let _ = stream.write_all(body).await;
    let _ = stream.shutdown().await;
}

async fn respond(stream: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store, no-cache, must-revalidate\r\nPragma: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    // Single write_all is fine now that the request headers are drained
    // before responding (no more RST race).
    let _ = stream.write_all(body).await;
    let _ = stream.shutdown().await;
}

fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The QR landing page. It renders the SAME page for every code — the
/// code goes into the deep link, and the phone app is the thing that
/// tells the user whether it still works.
///
/// The page used to render a different layout for a wrong code, which
/// turned it into a free oracle: one request per guess, and the
/// answer in the HTML. Now the response is byte-identical whatever
/// the code is, so guessing over HTTP buys the attacker nothing.
///
/// `code` is the code from the URL query — attacker-controlled, so it
/// is sanitised to six digits (or a six-character placeholder) before
/// it reaches the HTML: a stray `<` or `&` must not become markup, and
/// the rendered page must stay the same length whatever was asked for.
fn pair_page(code: Option<&str>, base: &str, apk_available: bool) -> String {
    // Six digits, or a fixed-width placeholder: the rendered page must
    // have the same length for every input, so a malformed code cannot
    // be told apart from a wrong one by its size.
    let c = match code {
        Some(c) if c.len() == 6 && c.bytes().all(|b| b.is_ascii_digit()) => c,
        _ => "------",
    };
    let web = format!("{base}/?code={c}");
    // `host` carries the phone-visible base URL so the native app can
    // connect with zero manual entry after a QR scan.
    let intent_url = format!(
        "intent://pair?c={c}&host={}#Intent;scheme=browserrelay;package=com.hafidz.browserrelay;S.browser_fallback_url={};end",
        percent_encode(base),
        percent_encode(&web)
    );
    let direct_url = format!(
        "browserrelay://pair?c={c}&host={}",
        percent_encode(base)
    );
    let code_html = format!(
        "<div class=\"coderow\"><div class=\"code\">{c}</div><button class=\"copybtn\" onclick=\"navigator.clipboard.writeText('{c}').then(function(){{this.textContent='Copied';}}).catch(function(){{}})\">Copy</button></div><p class=\"codehint\">or type this code manually in the app</p>"
    );
    let blurb = "Pairing scanned. Tap the button to open the app — if it doesn't open, type the code manually.";
    let buttons = r#"<a class="btn" href="{intent_url}">Open in app</a>
  <a class="btn ghost" href="{direct_url}">Open without the app</a>
  <a class="btn ghost" href="{web_url}">Use the web version</a>"#
        .replace("{intent_url}", &intent_url)
        .replace("{direct_url}", &direct_url)
        .replace("{web_url}", &web_url);
    let apk_btn = if apk_available {
        r#"<a class="btn ghost" href="/app.apk">Download the Android app</a>"#
    } else {
        ""
    };
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>browser-relay — pairing</title>
<style>
  body {{ margin:0; background:#161616; color:#dadada; font:15px/1.55 system-ui,-apple-system,"Segoe UI",sans-serif;
         display:flex; min-height:100vh; align-items:center; justify-content:center; }}
  .card {{ width:min(92vw,380px); background:#1e1e1e; border:1px solid #303030; border-radius:12px; padding:26px 22px; text-align:center; }}
  h1 {{ font-size:18px; margin:0 0 6px; }}
  p {{ color:#9a9a9a; margin:0 0 20px; font-size:13.5px; }}
  .btn {{ display:block; padding:13px 16px; border-radius:8px; background:#8b72f0; color:#fff;
          text-decoration:none; font-weight:600; margin-bottom:10px; }}
  .btn.ghost {{ background:#262626; color:#dadada; border:1px solid #303030; font-weight:500; }}
  .logo-wrap {{ width:56px; height:56px; border-radius:16px; background:#8b72f0;
    display:flex; align-items:center; justify-content:center; margin:0 auto 16px; }}
  .logo {{ width:38px; height:38px; }}
  .coderow {{ display:flex; gap:8px; align-items:stretch; }}
  .coderow .code {{ flex:1; }}
  .copybtn {{ align-self:stretch; padding:0 14px; border-radius:10px; cursor:pointer;
    background:#262626; color:#dadada; border:1px solid #303030; font:inherit; font-size:13px; }}
  .copybtn:hover {{ background:#2f2f2f; }}
  .code {{ font-size:34px; font-weight:700; letter-spacing:12px; text-indent:12px; color:#fff;
           background:#141414; border:1px solid #303030; border-radius:10px; padding:12px; margin:0 0 6px; }}
  .codehint {{ font-size:12px !important; margin:0 0 18px !important; }}
</style>
</head>
<body>
<div class="card">
  <div class="logo-wrap"><svg class="logo" viewBox="0 0 64 64" aria-hidden="true">
    <rect x="8" y="12" width="32" height="28" rx="9" fill="none" stroke="#fff" stroke-width="4" opacity=".85"/>
    <rect x="20" y="20" width="32" height="28" rx="9" fill="none" stroke="#fff" stroke-width="4"/>
    <rect x="37" y="36" width="18" height="18" rx="5" fill="#1e1e1e"/>
    <path d="M43 40.5l7 5.5-7 5.5z" fill="#fff"/>
  </svg></div>
  <h1>browser-relay</h1>
  <p>{blurb}</p>
  {code_html}
  {buttons}
  {apk_btn}
</div>
</body>
</html>"##
    )
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}
