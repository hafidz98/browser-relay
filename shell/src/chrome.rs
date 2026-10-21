//! Local config-dir paths, preferences, and management of the
//! dedicated remote-profile Chrome — a Rust mirror of the Go core's
//! prefs.go + chrome_mgmt_windows.go (same files, same launch args,
//! same chrome.pid contract).

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Don't flash a console window when spawning console programs
/// (taskkill/tasklist) from the GUI panel.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn sys_cmd(program: &str) -> Command {
    let mut c = Command::new(program);
    #[cfg(target_os = "windows")]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

pub fn cdp_port() -> u16 {
    crate::config::get().chrome_port
}

pub fn cdp_base() -> String {
    format!("http://127.0.0.1:{}", cdp_port())
}

/// %APPDATA%\browser-relay (the Go core's configDir()).
pub fn config_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return PathBuf::from(appdata).join("browser-relay");
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        return PathBuf::from(home)
            .join("AppData")
            .join("Roaming")
            .join("browser-relay");
    }
    PathBuf::from("browser-relay")
}

pub fn ensure_config_dir() {
    let _ = fs::create_dir_all(config_dir());
}

fn headless_path() -> PathBuf {
    config_dir().join("headless.txt")
}

pub fn load_headless() -> bool {
    fs::read_to_string(headless_path())
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

pub fn save_headless(on: bool) {
    ensure_config_dir();
    let _ = fs::write(headless_path(), if on { "1\n" } else { "0\n" });
}

pub fn chrome_pid_path() -> PathBuf {
    config_dir().join("chrome.pid")
}

pub fn chrome_pid() -> Option<u32> {
    fs::read_to_string(chrome_pid_path())
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn find_chrome() -> Option<PathBuf> {
    // Manual override from settings wins.
    let custom = crate::config::get().chrome_path;
    if !custom.is_empty() {
        let p = PathBuf::from(&custom);
        if p.exists() {
            return Some(p);
        }
    }
    let pf = std::env::var("ProgramFiles").unwrap_or_default();
    let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_default();
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    for base in [pf, pf86, local] {
        if base.is_empty() {
            continue;
        }
        let p = PathBuf::from(base)
            .join("Google")
            .join("Chrome")
            .join("Application")
            .join("chrome.exe");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// True when something answers on the DevTools HTTP endpoint.
pub fn cdp_up() -> bool {
    let addr = format!("127.0.0.1:{}", cdp_port());
    let mut stream = match TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_millis(1200)) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1200)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(1200)));
    if stream
        .write_all(format!("GET /json/version HTTP/1.0\r\nHost: {addr}\r\n\r\n").as_bytes())
        .is_err()
    {
        return false;
    }
    let mut buf = [0u8; 64];
    match stream.read(&mut buf) {
        Ok(n) if n > 0 => {
            let head = String::from_utf8_lossy(&buf[..n]);
            head.starts_with("HTTP/1.0 200") || head.starts_with("HTTP/1.1 200")
        }
        _ => false,
    }
}

/// Launch the dedicated remote-profile Chrome; mirrors
/// launchRemoteChrome() in the Go core, argument for argument.
pub fn launch_chrome() -> Result<(), String> {
    if cdp_up() {
        return Err("Chrome is already running with the debug port".into());
    }
    let chrome = find_chrome()
        .ok_or("chrome.exe was not found in the usual install locations")?;
    let profile = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default())
        .join("chrome-remote");
    let mut args: Vec<String> = vec![
        format!("--user-data-dir={}", profile.display()),
        format!("--remote-debugging-port={}", cdp_port()),
        "--remote-allow-origins=*".into(),
        "--no-first-run".into(),
        // Keep rendering while covered/minimized, else the screencast freezes.
        "--disable-backgrounding-occluded-windows".into(),
        "--disable-renderer-backgrounding".into(),
    ];
    if load_headless() {
        args.push("--headless=new".into());
    }
    args.push("chrome://newtab/".into());
    let child = Command::new(&chrome)
        .args(&args)
        .spawn()
        .map_err(|e| format!("failed to start Chrome: {e}"))?;
    ensure_config_dir();
    let _ = fs::write(chrome_pid_path(), child.id().to_string());
    Ok(())
}

/// Close the Chrome instance launched from the panel (by recorded PID).
/// Async GET of a DevTools HTTP path, parsed as JSON.
async fn cdp_http_get_json(path: &str) -> Result<serde_json::Value, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let fut = async {
        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", cdp_port()))
            .await
            .map_err(|e| format!("cdp http: {e}"))?;
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            cdp_port()
        );
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| format!("cdp http write: {e}"))?;
        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .await
            .map_err(|e| format!("cdp http read: {e}"))?;
        let text = String::from_utf8_lossy(&raw);
        let body = text
            .split("\r\n\r\n")
            .nth(1)
            .ok_or("cdp http: no body")?;
        serde_json::from_str(body).map_err(|e| format!("cdp http json: {e}"))
    };
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .map_err(|_| "cdp http: timeout".to_string())?
}

/// Whether the DevTools endpoint answers (async variant).
async fn cdp_up_async() -> bool {
    tokio::time::timeout(
        Duration::from_millis(800),
        tokio::net::TcpStream::connect(format!("127.0.0.1:{}", cdp_port())),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

/// Ask Chrome to quit gracefully via CDP `Browser.close` — the
/// equivalent of clicking the window X. Chrome saves its session,
/// so the next launch doesn't show the "restore pages" prompt.
/// Works for headed and headless alike. Returns true when Chrome
/// actually went away afterwards.
fn graceful_browser_close() -> bool {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return false,
    };
    rt.block_on(async {
        // Browser-level debugger URL (not a page target).
        let version = match cdp_http_get_json("/json/version").await {
            Ok(v) => v,
            Err(_) => return false,
        };
        let ws_url = match version
            .get("webSocketDebuggerUrl")
            .and_then(|u| u.as_str())
        {
            Some(u) => u,
            None => return false,
        };
        let nop: crate::relay::cdp::EventHandler = std::sync::Arc::new(|_, _| {});
        let cdp = match crate::relay::cdp::Cdp::dial(ws_url, &cdp_base(), nop)
            .await
        {
            Ok(c) => c,
            Err(_) => return false,
        };
        // Browser.close may never answer (the browser exits, dropping
        // the connection), so cap the wait and then poll the endpoint.
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            cdp.call("Browser.close", serde_json::json!({})),
        )
        .await;
        cdp.close().await;
        for _ in 0..30 {
            if !cdp_up_async().await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        !cdp_up_async().await
    })
}

/// True when `pid` is a live chrome.exe process (guards against
/// PID reuse before a force kill).
fn pid_is_chrome(pid: u32) -> bool {
    let out = sys_cmd("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output();
    match out {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout).to_lowercase();
            text.contains(&format!("\"chrome.exe\",\"{pid}\","))
        }
        Err(_) => false,
    }
}

/// taskkill wrapper. `force=false` sends WM_CLOSE (graceful, the
/// equivalent of clicking the window X — Chrome saves its session);
/// `force=true` is TerminateProcess.
fn taskkill(pid: u32, force: bool) -> Result<(), String> {
    let pid_s = pid.to_string();
    let mut args: Vec<&str> = vec!["/PID", &pid_s];
    if force {
        args.push("/F");
    }
    let status = sys_cmd("taskkill").args(&args).output();
    match status {
        Ok(out) if out.status.success() => Ok(()),
        // "not found" means the process is already gone: that is success.
        Ok(out) => {
            let msg = String::from_utf8_lossy(&out.stderr).to_lowercase();
            if msg.contains("not found") || msg.contains("no running instance") {
                Ok(())
            } else {
                Err(format!("taskkill failed: {}", msg.trim()))
            }
        }
        Err(e) => Err(format!("taskkill unavailable: {e}")),
    }
}

/// Poll until `pid` is gone (or timeout). True when the process exited.
fn wait_pid_gone(pid: u32, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !pid_is_chrome(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    !pid_is_chrome(pid)
}

pub fn close_chrome() -> Result<(), String> {
    let pid = chrome_pid().ok_or(
        "no panel-launched Chrome to close (Chrome started outside the panel must be closed from its own window)",
    )?;
    let _ = fs::remove_file(chrome_pid_path());
    // 1. Graceful via WM_CLOSE (taskkill without /F): the Windows-native
    //    equivalent of clicking the window X. Chrome saves its session,
    //    so relaunch doesn't show the "restore pages" prompt. For headless
    //    Chrome (no windows) this errors immediately — that's fine.
    let _ = taskkill(pid, false);
    if wait_pid_gone(pid, Duration::from_secs(4)) {
        return Ok(());
    }
    // 2. CDP Browser.close: covers headless and any headed case where
    //    WM_CLOSE didn't land (tight timeouts — no 13 s hang).
    if graceful_browser_close() {
        return Ok(());
    }
    // 3. Still there? Force kill — but only if the PID is still
    //    our chrome.exe (never kill a reused PID).
    if !pid_is_chrome(pid) {
        return Ok(());
    }
    taskkill(pid, true)
}
