//! The relay, in-process (v0.8.0 full-Rust): a tokio runtime on
//! background threads serves the phone client and owns the CDP
//! session; the egui panel reads shared status. Replaces the v0.7.0
//! Go sidecar (see docs/plan/FULL-RUST.md).

pub mod browser;
pub mod cdp;
pub mod clipboard;
pub mod hub;
pub mod httpd;
pub mod pairing;

use std::fs;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::Rng;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::chrome;
use crate::VERSION;

// ---------- logging (relay.log in the config dir, like the Go core) ----------

static LOG_FILE: Mutex<Option<fs::File>> = Mutex::new(None);

pub fn init_log() {
    chrome::ensure_config_dir();
    if let Ok(f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(chrome::config_dir().join("relay.log"))
    {
        *LOG_FILE.lock().unwrap() = Some(f);
    }
}

pub fn log_line(msg: &str) {
    let line = format!("[{}] {}", ts_now(), msg);
    eprintln!("{line}");
    if let Some(f) = LOG_FILE.lock().unwrap().as_mut() {
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }
}

#[macro_export]
macro_rules! rlog {
    ($($t:tt)*) => {
        $crate::relay::log_line(&format!($($t)*))
    };
}

/// UTC timestamp "HH:MM:SS" from the system clock (log lines only).
fn ts_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

// ---------- shared status (panel reads this) ----------

#[derive(Clone, Default)]
pub struct SharedStatus {
    pub server_up: bool,
    pub server_err: Option<String>,
    pub connected: bool,
    pub clients: i64,
    pub phone_url: String,
    pub phone_urls: Vec<String>,
    pub code: String,
}

#[derive(Clone, Debug)]
pub struct CoreStatus {
    pub connected: bool,
    pub clients: i64,
    pub phone_url: String,
    pub phone_urls: Vec<String>,
    pub code: String,
    pub version: String,
}

#[derive(Clone, Debug)]
pub enum CoreState {
    Starting,
    Running(CoreStatus),
    Down(String),
}

#[derive(Clone)]
pub struct RelayOpts {
    pub cdp_base: String,
    pub listen: String, // "" = auto-detect
    pub port: u16,
    pub token_flag: String,
}

impl Default for RelayOpts {
    fn default() -> Self {
        let cfg = crate::config::get();
        RelayOpts {
            cdp_base: format!("http://127.0.0.1:{}", cfg.chrome_port),
            listen: cfg.relay_addr.clone(),
            port: cfg.relay_port,
            token_flag: String::new(),
        }
    }
}

/// Parse CLI args shared by the GUI binary and relay-nogui:
/// --nogui/--service, --cdp, --listen, --port, --token.
pub fn parse_args() -> (bool, RelayOpts) {
    let args: Vec<String> = std::env::args().collect();
    let mut opts = RelayOpts::default();
    let mut nogui = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--nogui" | "--service" => nogui = true,
            "--cdp" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    opts.cdp_base = v.clone();
                }
            }
            "--listen" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    opts.listen = v.clone();
                }
            }
            "--port" => {
                i += 1;
                if let Some(v) = args.get(i).and_then(|s| s.parse::<u16>().ok()) {
                    opts.port = v;
                }
            }
            "--token" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    opts.token_flag = v.clone();
                }
            }
            _ => {}
        }
        i += 1;
    }
    (nogui, opts)
}

pub struct RelayHandle {
    rt: Arc<tokio::runtime::Runtime>,
    opts: RelayOpts,
    shared: Arc<Mutex<SharedStatus>>,
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    phone_url: String,
    started: Instant,
    hub: Arc<Mutex<Option<Arc<hub::Hub>>>>,
}

impl RelayHandle {
    /// Resolve token + URL synchronously (so the panel/banner can show
    /// them immediately), then spawn the relay tasks on the runtime.
    pub fn start(
        rt: Arc<tokio::runtime::Runtime>,
        opts: RelayOpts,
    ) -> Result<RelayHandle, String> {
        let ip = if opts.listen.is_empty() {
            detect_lan_ip()
        } else {
            opts.listen.clone()
        };
        // Bind to 0.0.0.0 (all interfaces) when auto-detecting, so the server
        // is reachable via WiFi, hotspot, etc. simultaneously. The displayed
        // URL still uses the detected LAN IP for convenience.
        let bind_ip = if opts.listen.is_empty() {
            "0.0.0.0".to_string()
        } else {
            ip.clone()
        };
        let addr = format!("{bind_ip}:{}", opts.port);
        let phone_url = format!("http://{ip}:{}", opts.port);
        // All reachable URLs (one per interface) for the panel display.
        let phone_urls: Vec<String> = if opts.listen.is_empty() {
            detect_lan_ips()
                .into_iter()
                .map(|a| format!("http://{a}:{}", opts.port))
                .collect()
        } else {
            vec![phone_url.clone()]
        };
        let shared = Arc::new(Mutex::new(SharedStatus {
            phone_url: phone_url.clone(),
            phone_urls,
            ..Default::default()
        }));
        let tasks: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
        let hub_slot: Arc<Mutex<Option<Arc<hub::Hub>>>> = Arc::new(Mutex::new(None));
        let h = RelayHandle {
            rt,
            opts,
            shared,
            tasks,
            phone_url,
            started: Instant::now(),
            hub: hub_slot.clone(),
        };
        h.spawn_tasks(addr, hub_slot);
        Ok(h)
    }

    /// The live hub (None until the server task creates it).
    pub fn hub(&self) -> Option<Arc<hub::Hub>> {
        self.hub.lock().unwrap().clone()
    }

    fn spawn_tasks(&self, addr: String, hub_slot: Arc<Mutex<Option<Arc<hub::Hub>>>>) {
        let shared = self.shared.clone();
        let tasks = self.tasks.clone();
        let opts = self.opts.clone();
        let sup = self.rt.spawn(async move {
            let listener = match TcpListener::bind(&addr).await {
                Ok(l) => l,
                Err(e) => {
                    let mut sh = shared.lock().unwrap();
                    sh.server_err = Some(format!(
                        "could not listen on {addr}: {e} — is another relay already running?"
                    ));
                    rlog!("listen {addr}: {e}");
                    return;
                }
            };
            rlog!("phone server on {addr}");
            {
                let mut sh = shared.lock().unwrap();
                sh.server_up = true;
                sh.server_err = None;
            }
            let fixed = if opts.token_flag.is_empty() {
                None
            } else {
                Some(opts.token_flag.clone())
            };
            let hub = hub::Hub::new(&chrome::config_dir(), fixed);
            *hub_slot.lock().unwrap() = Some(hub.clone());
            let browser = browser::Browser::new(
                &opts.cdp_base,
                hub.clone(),
                browser::ScreencastOpts::default(),
            );
            hub.set_browser(browser.clone());

            let mut mine: Vec<JoinHandle<()>> = Vec::new();
            mine.push(tokio::spawn(httpd::serve(listener, hub.clone())));
            mine.push(tokio::spawn(browser.clone().watch()));
            mine.push(tokio::spawn(browser.clone().maintain()));
            // status writer (1 s): connected = DevTools answers AND a
            // tab is attached; clients from the hub.
            let sh2 = shared.clone();
            let b2 = browser.clone();
            let hub2 = hub.clone();
            mine.push(tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                loop {
                    tick.tick().await;
                    let connected =
                        !b2.current_target().is_empty() && b2.is_chrome_up().await;
                    {
                        let mut sh = sh2.lock().unwrap();
                        sh.connected = connected;
                        sh.clients = hub2.authed_client_count();
                    }
                    // Phones hear about browser up/down transitions
                    // so they can show an indicator instead of black.
                    if hub2.set_browser_up(connected) {
                        hub2.broadcast_status();
                    }
                }
            }));
            tasks.lock().unwrap().extend(mine);
        });
        self.tasks.lock().unwrap().push(sup);
    }

    pub fn state(&self) -> CoreState {
        let sh = self.shared.lock().unwrap();
        if let Some(err) = &sh.server_err {
            return CoreState::Down(err.clone());
        }
        if sh.server_up {
            // Current single-use pairing code comes from the hub (rotates).
            // Client count is read fresh from the hub (authed only) so the
            // panel never shows a stale number.
            let code = self.hub().map(|h| h.pair_code()).unwrap_or_default();
            let clients = self.hub().map(|h| h.authed_client_count()).unwrap_or(0);
            return CoreState::Running(CoreStatus {
                connected: sh.connected,
                clients,
                phone_url: sh.phone_url.clone(),
                phone_urls: sh.phone_urls.clone(),
                code,
                version: VERSION.to_string(),
            });
        }
        if self.started.elapsed() < Duration::from_secs(8) {
            CoreState::Starting
        } else {
            CoreState::Down(
                "relay did not start — see relay.log in the config folder".into(),
            )
        }
    }

    pub fn restart(&mut self) {
        self.stop();
        self.started = Instant::now();
        {
            let mut sh = self.shared.lock().unwrap();
            sh.server_up = false;
            sh.server_err = None;
            sh.connected = false;
            sh.clients = 0;
        }
        let ip = if self.opts.listen.is_empty() {
            detect_lan_ip()
        } else {
            self.opts.listen.clone()
        };
        // Bind to 0.0.0.0 when auto-detecting (see spawn path above).
        let bind_ip = if self.opts.listen.is_empty() {
            "0.0.0.0".to_string()
        } else {
            ip.clone()
        };
        let addr = format!("{bind_ip}:{}", self.opts.port);
        let hub_slot = self.hub.clone();
        self.spawn_tasks(addr, hub_slot);
    }

    pub fn stop(&mut self) {
        let mut tasks = self.tasks.lock().unwrap();
        for t in tasks.drain(..) {
            t.abort();
        }
    }

    pub fn phone_url(&self) -> &str {
        &self.phone_url
    }

    /// Current single-use pairing code (from the hub; rotates on use).
    pub fn code(&self) -> String {
        self.hub()
            .map(|h| h.pair_code())
            .unwrap_or_default()
    }

    /// Pending client-initiated pairing request, if any.
    pub fn pair_request(&self) -> Option<pairing::PairRequest> {
        self.hub().and_then(|h| h.pairing().pending_request())
    }

    /// Dismiss the pending pairing request.
    pub fn cancel_pair_request(&self) {
        if let Some(h) = self.hub() {
            h.pairing().cancel_request();
        }
    }

    /// Paired devices (fingerprint contracts).
    pub fn paired_devices(&self) -> Vec<(String, pairing::PairedDevice)> {
        self.hub()
            .map(|h| h.pairing().list())
            .unwrap_or_default()
    }

    /// Revoke a paired device; drops its live connections.
    pub fn revoke_device(&self, fingerprint: &str) -> bool {
        if let Some(h) = self.hub() {
            if h.pairing().revoke(fingerprint) {
                h.drop_revoked(fingerprint);
                return true;
            }
        }
        false
    }
}

// ---------- token + LAN IP (ports of main.go) ----------

fn is_pair_code(t: &str) -> bool {
    t.len() == 6 && t.bytes().all(|b| b.is_ascii_digit())
}

pub fn resolve_token(flag: &str) -> Result<String, String> {
    if !flag.is_empty() {
        return Ok(flag.to_string());
    }
    chrome::ensure_config_dir();
    let p = chrome::config_dir().join("token.txt");
    if let Ok(content) = fs::read_to_string(&p) {
        let t = content.trim();
        if is_pair_code(t) {
            return Ok(t.to_string());
        }
        // old long-format token: fall through and rotate to a pair code
    }
    let code = format!("{}", rand::rng().random_range(100_000..=999_999));
    fs::write(&p, format!("{code}\n")).map_err(|e| format!("write token: {e}"))?;
    Ok(code)
}

/// First usable non-loopback, non-link-local IPv4 (Go: detectLANIP).
pub fn detect_lan_ip() -> String {
    detect_lan_ips().into_iter().next().unwrap_or_else(|| "127.0.0.1".to_string())
}

/// All usable non-loopback, non-link-local IPv4 addresses (for display).
pub fn detect_lan_ips() -> Vec<String> {
    let mut ips = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if iface.is_loopback() {
                continue;
            }
            // Skip virtual adapters: WSL/Hyper-V vEthernet (172.x),
            // VMware, VirtualBox, Tailscale, ZeroTier. Their addresses
            // are unreachable from a phone on the LAN — and WSL's
            // vEthernet usually sorts FIRST, poisoning the primary
            // phone_url and the pairing QR host.
            let name = iface.name.to_lowercase();
            if name.starts_with("vethernet")
                || name.starts_with("wsl")
                || name.contains("vmware")
                || name.contains("virtualbox")
                || name.contains("tailscale")
                || name.contains("zerotier")
            {
                continue;
            }
            if let std::net::IpAddr::V4(v4) = iface.ip() {
                let o = v4.octets();
                if o[0] == 169 && o[1] == 254 {
                    continue; // link-local
                }
                let s = v4.to_string();
                if !ips.contains(&s) {
                    ips.push(s);
                }
            }
        }
    }
    if ips.is_empty() {
        ips.push("127.0.0.1".to_string());
    }
    ips
}
