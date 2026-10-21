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

#[allow(unused_mut)] // mut is only used by the Windows-only cfg below
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

/// Host name of this PC (shown to clients so the phone can tell
/// which machine it drives).
pub fn host_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this PC".to_string())
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
        // chrome://newtab/ does not create a page target in the new
        // headless mode, so the relay had nothing to attach to and the
        // panel kept saying "Chrome not running".
        args.push("about:blank".into());
    } else {
        args.push("chrome://newtab/".into());
    }
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
    // The recorded PID is a hint, not a handle: Chrome re-execs, so the
    // PID we wrote at launch may already be gone while the browser
    // keeps running. The DevTools endpoint is the reliable handle.
    let pid = chrome_pid();
    let _ = fs::remove_file(chrome_pid_path());
    // 1. Graceful via WM_CLOSE (taskkill without /F): the Windows-native
    //    equivalent of clicking the window X. Chrome saves its session,
    //    so relaunch does not show the "restore pages" prompt.
    if let Some(pid) = pid {
        let _ = taskkill(pid, false);
        let _ = wait_pid_gone(pid, Duration::from_secs(2));
    }
    // 2. CDP Browser.close: closes the whole browser, tabs included, and
    //    works for headless Chrome too.
    let _ = graceful_browser_close();
    // 3. Wait for the endpoint to go away - that is the proof that the
    //    browser is really gone.
    for _ in 0..16 {
        if !cdp_up() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    // 4. Still answering: kill the process that serves our debug port.
    if let Some(pid) = chrome_pid_by_debug_port() {
        if pid_is_chrome(pid) {
            let _ = taskkill(pid, true);
            for _ in 0..16 {
                if !cdp_up() {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
    Err("Chrome is still running".into())
}

/// Find the Chrome process that serves our DevTools port.
fn chrome_pid_by_debug_port() -> Option<u32> {
    let port = cdp_port();
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='chrome.exe'\" | \
         Where-Object {{ $_.CommandLine -like '*--remote-debugging-port={}*' }} | \
         Select-Object -First 1 -ExpandProperty ProcessId",
        port
    );
    let out = sys_cmd("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.trim().parse::<u32>().ok())
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn EnumWindows(
        cb: extern "system" fn(*mut std::ffi::c_void, isize) -> i32,
        param: isize,
    ) -> i32;
    fn GetWindowThreadProcessId(hwnd: *mut std::ffi::c_void, pid: *mut u32) -> u32;
    fn ShowWindow(hwnd: *mut std::ffi::c_void, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: *mut std::ffi::c_void) -> i32;
    fn BringWindowToTop(hwnd: *mut std::ffi::c_void) -> i32;
}

#[cfg(windows)]
static WAKE_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
#[cfg(windows)]
static WAKE_HWND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(windows)]
extern "system" fn collect_chrome_window(hwnd: *mut std::ffi::c_void, _l: isize) -> i32 {
    use std::sync::atomic::Ordering;
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == WAKE_PID.load(Ordering::Relaxed) {
            WAKE_HWND.store(hwnd as usize, Ordering::Relaxed);
            return 0; // stop
        }
    }
    1
}

/// Bring the Chrome window to the front and restore it when it was
/// minimized.
///
/// A minimized or covered window stops painting, so the screencast
/// freezes and the phone keeps looking at an old screen. Restoring the
/// window makes it paint again.
pub fn wake_chrome() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::sync::atomic::Ordering;
        let pid = chrome_pid_by_debug_port()
            .or_else(chrome_pid)
            .ok_or("Chrome is not running")?;
        WAKE_PID.store(pid, Ordering::Relaxed);
        WAKE_HWND.store(0, Ordering::Relaxed);
        unsafe {
            EnumWindows(collect_chrome_window, 0);
        }
        let hwnd = WAKE_HWND.load(Ordering::Relaxed);
        if hwnd == 0 {
            return Err("no Chrome window found".into());
        }
        const SW_RESTORE: i32 = 9;
        unsafe {
            ShowWindow(hwnd as *mut std::ffi::c_void, SW_RESTORE);
            BringWindowToTop(hwnd as *mut std::ffi::c_void);
            SetForegroundWindow(hwnd as *mut std::ffi::c_void);
        }
        return Ok(());
    }
    #[cfg(not(windows))]
    Err("not supported on this platform".into())
}

/// Do the flags that keep the screen alive exist on the running
/// Chrome?
///
/// `--disable-backgrounding-occluded-windows` and
/// `--disable-renderer-backgrounding` are what keep Chrome painting
/// while another window covers it. They only work when Chrome is
/// started with them, so a Chrome that was already running (started
/// by an older build, or by hand) can freeze the screencast. None =
/// no Chrome to look at.
pub fn chrome_has_relay_flags() -> Option<bool> {
    let port = cdp_port();
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='chrome.exe'\" | \
         Where-Object {{ $_.CommandLine -like '*--remote-debugging-port={}*' }} | \
         Select-Object -First 1 -ExpandProperty CommandLine",
        port
    );
    let out = sys_cmd("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    if text.trim().is_empty() {
        return None;
    }
    Some(
        text.contains("--disable-backgrounding-occluded-windows")
            && text.contains("--disable-renderer-backgrounding"),
    )
}

/// Close Chrome and start it again - this time with the flags that
/// keep the screen alive.
pub fn restart_chrome() -> Result<(), String> {
    let _ = close_chrome();
    std::thread::sleep(Duration::from_millis(800));
    launch_chrome()
}
