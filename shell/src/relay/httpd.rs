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
            let code = query_param(&query, "c");
            let valid = code.as_deref() == Some(hub.pair_code().as_str());
            let base = stream
                .local_addr()
                .map(|a| format!("http://{}", a))
                .unwrap_or_default();
            let apk_available = chrome::config_dir().join("browser-relay.apk").exists();
            let html = pair_page(valid.then_some(code.as_deref()).flatten(), &base, apk_available);
            respond(&mut stream, "200 OK", "text/html; charset=utf-8", html.as_bytes()).await;
        }
        "/app.apk" => {
            let p = chrome::config_dir().join("browser-relay.apk");
            match tokio::fs::read(&p).await {
                Ok(bytes) => {
                    respond(&mut stream, "200 OK", "application/vnd.android.package-archive", &bytes)
                        .await;
                }
                Err(_) => {
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

async fn respond(stream: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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

/// The QR landing page. With a valid code it auto-jumps to the
/// Android app via an intent:// URL whose browser fallback is the
/// pre-paired web client; without the app (or without a valid code)
/// the user lands on the web client.
fn pair_page(code: Option<&str>, base: &str, apk_available: bool) -> String {
    let (intent_url, direct_url, web_url, code_html) = match code {
        Some(c) => {
            let web = format!("{base}/?code={c}");
            // `host` carries the phone-visible base URL so the native
            // app can connect with zero manual entry after a QR scan.
            let intent = format!(
                "intent://pair?c={c}&host={}#Intent;scheme=browserrelay;package=com.hafidz.browserrelay;S.browser_fallback_url={};end",
                percent_encode(base),
                percent_encode(&web)
            );
            let direct = format!(
                "browserrelay://pair?c={c}&host={}",
                percent_encode(base)
            );
            let code_h = format!(
                "<div class=\"code\">{}</div><p class=\"codehint\">atau ketik manual kode ini di aplikasi</p>",
                c
            );
            (intent, direct, web, code_h)
        }
        None => (
            String::new(),
            String::new(),
            format!("{base}/"),
            String::new(),
        ),
    };
    let apk_btn = if apk_available {
        r#"<a class="btn ghost" href="/app.apk">Download aplikasi Android</a>"#
    } else {
        ""
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="id">
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
  .dot {{ width:44px; height:44px; border-radius:50%; background:#8b72f0; margin:0 auto 16px; }}
  .code {{ font-size:34px; font-weight:700; letter-spacing:12px; text-indent:12px; color:#fff;
           background:#141414; border:1px solid #303030; border-radius:10px; padding:12px; margin:0 0 6px; }}
  .codehint {{ font-size:12px !important; margin:0 0 18px !important; }}
</style>
</head>
<body>
<div class="card">
  <div class="dot"></div>
  <h1>browser-relay</h1>
  <p>Pairing berhasil discan. Ketuk tombol buat buka aplikasi — kalau nggak kebuka, ketik kodenya manual.</p>
  {code_html}
  <a class="btn" href="{intent_url}">Buka di aplikasi</a>
  <a class="btn ghost" href="{direct_url}">Coba cara lain</a>
  <a class="btn ghost" href="{web_url}">Pakai versi web</a>
  {apk_btn}
</div>
</body>
</html>"#
    )
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}
