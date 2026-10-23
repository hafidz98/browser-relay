#![windows_subsystem = "windows"]

//! browser-relay (egui, native — no WebView): the panel, the tray,
//! and the relay itself (mod relay) in a single full-Rust binary
//! since v0.8.0. See docs/plan/FULL-RUST.md.

mod chrome;
mod config;
mod firewall;
pub mod relay;

use std::fs;
use std::net::TcpListener;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

use relay::{CoreState, RelayHandle, RelayOpts};

pub const VERSION: &str = "0.9.69";
const ICON_RGBA: &[u8] = include_bytes!("../assets/icon.rgba");
const ICON_SIZE: u32 = 64;
/// Loopback port used as a single-instance lock (held for lifetime).
const INSTANCE_LOCK_PORT: u16 = 18769;

// Obsidian-style palette (same values as the Wails panel CSS).
fn light() -> bool {
    crate::config::get().theme == "light"
}
fn BG() -> egui::Color32 {
    if light() { egui::Color32::from_rgb(0xf4, 0xf4, 0xf4) } else { egui::Color32::from_rgb(0x16, 0x16, 0x16) }
}
fn CARD() -> egui::Color32 {
    if light() { egui::Color32::WHITE } else { egui::Color32::from_rgb(0x1e, 0x1e, 0x1e) }
}
fn RAISED() -> egui::Color32 {
    if light() { egui::Color32::from_rgb(0xec, 0xec, 0xec) } else { egui::Color32::from_rgb(0x26, 0x26, 0x26) }
}
fn ACTIVE() -> egui::Color32 {
    if light() { egui::Color32::from_rgb(0xdd, 0xdd, 0xdd) } else { egui::Color32::from_rgb(0x2f, 0x2f, 0x2f) }
}
fn BORDER() -> egui::Color32 {
    if light() { egui::Color32::from_rgb(0xd0, 0xd0, 0xd0) } else { egui::Color32::from_rgb(0x30, 0x30, 0x30) }
}
fn TEXT() -> egui::Color32 {
    if light() { egui::Color32::from_rgb(0x1a, 0x1a, 0x1a) } else { egui::Color32::from_rgb(0xda, 0xda, 0xda) }
}
fn MUTED() -> egui::Color32 {
    if light() { egui::Color32::from_rgb(0x66, 0x66, 0x66) } else { egui::Color32::from_rgb(0x9a, 0x9a, 0x9a) }
}
fn FAINT() -> egui::Color32 {
    if light() { egui::Color32::from_rgb(0x99, 0x99, 0x99) } else { egui::Color32::from_rgb(0x6b, 0x6b, 0x6b) }
}
fn ACCENT() -> egui::Color32 {
    egui::Color32::from_rgb(0x8b, 0x72, 0xf0)
}
fn GREEN() -> egui::Color32 {
    if light() { egui::Color32::from_rgb(0x2d, 0x8a, 0x3e) } else { egui::Color32::from_rgb(0x3f, 0xb9, 0x50) }
}
/// Warning color (firewall notice).
fn AMBER() -> egui::Color32 {
    if light() {
        egui::Color32::from_rgb(0xb7, 0x79, 0x1f)
    } else {
        egui::Color32::from_rgb(0xe3, 0xb3, 0x41)
    }
}

fn RED() -> egui::Color32 {
    if light() { egui::Color32::from_rgb(0xd3, 0x2f, 0x2f) } else { egui::Color32::from_rgb(0xe5, 0x53, 0x4b) }
}

/// Apply the configured theme to egui's visuals (call on start + on save).
fn apply_theme(ctx: &egui::Context) {
    if light() {
        ctx.set_visuals(egui::Visuals::light());
    } else {
        ctx.set_visuals(egui::Visuals::dark());
    }
}

/// "3m ago" / "2h ago" for the client list.
fn ago_str(unix_secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(unix_secs);
    let d = now.saturating_sub(unix_secs);
    if d < 60 {
        format!("{d}s ago")
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else {
        format!("{}h ago", d / 3600)
    }
}

fn styled_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.window_fill = BG();
    v.panel_fill = BG();
    v.extreme_bg_color = egui::Color32::from_rgb(0x10, 0x10, 0x10);
    v.override_text_color = Some(TEXT());
    v.widgets.inactive.bg_fill = RAISED();
    v.widgets.inactive.weak_bg_fill = RAISED();
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER());
    v.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
    v.widgets.hovered.bg_fill = ACTIVE();
    v.widgets.hovered.weak_bg_fill = ACTIVE();
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, BORDER());
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
    v.widgets.active.bg_fill = ACTIVE();
    v.widgets.active.weak_bg_fill = ACTIVE();
    v.widgets.active.corner_radius = egui::CornerRadius::same(6);
    v.selection.bg_fill = egui::Color32::from_rgba_premultiplied(0x8b, 0x72, 0xf0, 0x40);
    v
}

fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD())
        .stroke(egui::Stroke::new(1.0, BORDER()))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(14))
}

fn field_box() -> egui::Frame {
    egui::Frame::new()
        .fill(BG())
        .stroke(egui::Stroke::new(1.0, BORDER()))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(10, 7))
}

/// Force the current Ui (e.g. a card body) to span the full
/// available width: a zero-height allocation widens min_rect.
fn force_full_width(ui: &mut egui::Ui) {
    let w = ui.available_width();
    if w > 0.0 {
        ui.allocate_space(egui::vec2(w, 0.0));
    }
}

enum UiAction {
    LaunchChrome,
    CloseChrome,
}

#[cfg(windows)]
#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(
        hwnd: *mut std::ffi::c_void,
        dwattribute: u32,
        pvattribute: *const std::ffi::c_void,
        cbattribute: u32,
    ) -> i32;
}

// Window-region rounded corners: the approach that works on every
// Windows version (Win10 has no DWM corner rounding, and per-pixel
// alpha windows are disqualified from it anyway). Clips the window
// shape itself via a rounded-rect region; reapplied on resize.
//
// A plain comment, not a doc comment: rustdoc does not document
// extern blocks, so a doc comment here is an unused-doc-comment
// warning.
#[cfg(windows)]
#[link(name = "gdi32")]
extern "system" {
    fn CreateRoundRectRgn(
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        ellipse_w: i32,
        ellipse_h: i32,
    ) -> *mut std::ffi::c_void;
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn SetWindowRgn(
        hwnd: *mut std::ffi::c_void,
        hrgn: *mut std::ffi::c_void,
        redraw: i32,
    ) -> i32;
}

/// Ask DWM (Windows 11) to round this window's corners natively,
/// keeping the proper window shadow. Ignored on Windows 10; the
/// SetWindowRgn region below covers that case.
#[cfg(windows)]
fn dwm_round_corners(hwnd: *mut std::ffi::c_void) {
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUNDSMALL: i32 = 3;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &DWMWCP_ROUNDSMALL as *const i32 as *const std::ffi::c_void,
            4,
        );
    }
}

/// Result of a firewall probe or rule change, delivered from the
/// background thread that runs PowerShell.
enum FwUpdate {
    Verdict(firewall::Verdict),
    RuleAdded(bool),
}

struct App {
    relay: Option<RelayHandle>,
    relay_err: Option<String>,
    rt: std::sync::Arc<tokio::runtime::Runtime>,
    opts: RelayOpts,
    qr_cache: Option<(String, Vec<bool>, usize)>,
    qr_popup: bool,
    /// Device list popup (from "Connected device" row).
    devices_popup: bool,
    _tray: Option<TrayIcon>,
    tray_ids: TrayIds,
    /// App icon texture for the title bar (loaded lazily from ICON_RGBA).
    title_icon: Option<egui::TextureHandle>,
    /// Raw HWND for Win32 calls (DWM rounding, window region).
    /// 0 = unavailable (non-Windows or handle lookup failed).
    #[cfg(windows)]
    hwnd: usize,
    /// Physical pixel size the window region was last built for.
    #[cfg(windows)]
    region_applied: Option<(i32, i32)>,
    headless: bool,
    toast: Option<(String, Instant)>,
    action_tx: Sender<Result<String, String>>,
    action_rx: Receiver<Result<String, String>>,
    action_pending: bool,
    last_poll: Instant,
    quitting: bool,
    _instance_lock: Option<TcpListener>,
    /// Settings view toggle.
    show_settings: bool,
    /// Windows Firewall verdict for this binary (None = not checked).
    fw_verdict: Option<firewall::Verdict>,
    /// Firewall probe results (PowerShell runs on a worker thread).
    fw_rx: Receiver<FwUpdate>,
    fw_tx: Sender<FwUpdate>,
    /// Last theme applied to egui visuals (re-apply on change).
    applied_theme: String,    /// Settings edit buffers (loaded from config when opening).
    set_chrome_path: String,
    set_theme: String,
    set_chrome_port: String,
    set_relay_addr: String,
    set_relay_port: String,
    set_hide_to_tray: bool,
    set_auto_start_server: bool,
    set_auto_launch_browser: bool,
}

#[derive(Clone)]
struct TrayIds {
    open: MenuId,
    launch: MenuId,
    close: MenuId,
    quit: MenuId,
}

/// Panel HWND (set once at startup) so tray/sentinel threads can
/// restore the window WITHOUT going through egui's viewport queue.
#[cfg(windows)]
static PANEL_HWND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn ShowWindow(hwnd: *mut std::ffi::c_void, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: *mut std::ffi::c_void) -> i32;
    fn BringWindowToTop(hwnd: *mut std::ffi::c_void) -> i32;
}

/// Restore the panel via Win32 directly. egui's ViewportCommand queue
/// is only flushed on a redraw, and a hidden window receives none —
/// so Visible(true) sent from tray/sentinel threads was a silent
/// no-op and the panel could never be reopened after closing (bug
/// verified live: sentinel consumed, IsWindowVisible still false).
#[cfg(windows)]
fn show_panel_native() {
    let hwnd = PANEL_HWND.load(std::sync::atomic::Ordering::Relaxed);
    if hwnd == 0 {
        return;
    }
    const SW_RESTORE: i32 = 9; // show hidden / restore minimized
    unsafe {
        ShowWindow(hwnd as *mut std::ffi::c_void, SW_RESTORE);
        SetForegroundWindow(hwnd as *mut std::ffi::c_void);
        BringWindowToTop(hwnd as *mut std::ffi::c_void);
    }
}

#[cfg(not(windows))]
fn show_panel_native() {}

/// Re-show the panel window. Safe to call from any thread.
fn show_panel(ctx: &egui::Context) {
    // Win32 first: deterministic, independent of the render loop.
    show_panel_native();
    // egui commands as a complement (they also fix internal state
    // like focus/minimize flags once the loop wakes).
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
    // Second attempt after a beat, in case the first was swallowed
    // while the event loop was idle.
    let ctx2 = ctx.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        show_panel_native();
        ctx2.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx2.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx2.request_repaint();
    });
}

/// Quit the whole app from any thread: close panel-launched
/// Chrome (never a hand-started one), then exit. process::exit
/// reclaims the relay's sockets and threads; Chrome is the only
/// external process that needs explicit cleanup.
fn quit_app() {
    if chrome::chrome_pid().is_some() {
        let _ = chrome::close_chrome();
    }
    // Unconditional exit: the tray-icon + tokio teardown has hung
    // the event loop before (Quit did nothing and the process had
    // to be killed from Task Manager).
    std::process::exit(0);
}

impl App {
    /// Clip the window to a rounded rect via SetWindowRgn (the
    /// Win10-compatible way to get rounded corners; DWM rounding
    /// doesn't exist there). Rebuilt only when the physical window
    /// size changes (the auto-fit height resizes the window).
    #[cfg(windows)]
    fn apply_window_region(&mut self, ctx: &egui::Context) {
        if self.hwnd == 0 {
            return;
        }
        let ppp = ctx.pixels_per_point();
        let (lw, lh) = ctx.input(|i| {
            i.viewport()
                .inner_rect
                .map(|r| (r.width(), r.height()))
                .unwrap_or((344.0, 540.0))
        });
        let pw = (lw * ppp).round() as i32;
        let ph = (lh * ppp).round() as i32;
        if pw <= 0 || ph <= 0 || self.region_applied == Some((pw, ph)) {
            return;
        }
        // 8px corner radius (ellipse diameters in physical px).
        let d = (16.0 * ppp).round() as i32;
        unsafe {
            let hrgn = CreateRoundRectRgn(0, 0, pw + 1, ph + 1, d, d);
            if !hrgn.is_null() {
                // The system takes ownership of the region.
                SetWindowRgn(self.hwnd as *mut std::ffi::c_void, hrgn, 1);
                self.region_applied = Some((pw, ph));
            }
        }
    }
}

/// Dedicated tray event threads. egui's update() is NOT called
/// while the window is hidden (Windows delivers no WM_PAINT to a
/// hidden window, so winit never emits RedrawRequested) — polling
/// tray events from update() left the tray completely dead after
/// hiding the panel. These threads block on the tray-icon
/// channels directly and act through the thread-safe Context.
fn spawn_tray_watcher(ctx: egui::Context, ids: TrayIds) {
    let ctx_click = ctx.clone();
    let ctx_menu = ctx.clone();
    let ctx_sentinel = ctx.clone();
    let _ = std::thread::Builder::new()
        .name("tray-click".into())
        .spawn(move || loop {
            match TrayIconEvent::receiver().recv() {
                Ok(TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    ..
                }) => show_panel(&ctx_click),
                Ok(_) => {}
                Err(_) => return, // channel closed during shutdown
            }
        });
    let _ = std::thread::Builder::new()
        .name("tray-menu".into())
        .spawn(move || loop {
            match MenuEvent::receiver().recv() {
                Ok(ev) => {
                    if ev.id == ids.open {
                        show_panel(&ctx_menu);
                    } else if ev.id == ids.launch {
                        let _ = chrome::launch_chrome();
                    } else if ev.id == ids.close {
                        let _ = chrome::close_chrome();
                    } else if ev.id == ids.quit {
                        quit_app();
                    }
                }
                Err(_) => return,
            }
        });
    // Second-instance handoff: a new exe writes the sentinel file
    // and exits; we surface the panel. Polled here (not in update)
    // so it also works while the window is hidden. The thread uses
    // ctx_sentinel (cloned above), so there is nothing else to copy.
    let _ = std::thread::Builder::new()
        .name("tray-sentinel".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let sentinel = chrome::config_dir().join("show-panel");
            if sentinel.exists() {
                let _ = fs::remove_file(&sentinel);
                show_panel(&ctx_sentinel);
            }
        });
}

impl App {
    fn new(
        lock: Option<TcpListener>,
        rt: std::sync::Arc<tokio::runtime::Runtime>,
        opts: RelayOpts,
    ) -> Self {
        let mut toast = None;
        // Manual server start by default (avoids the Windows Firewall
        // prompt on first open); auto-start only when configured.
        let (relay, relay_err) = if crate::config::get().auto_start_server {
            match RelayHandle::start(rt.clone(), opts.clone()) {
                Ok(h) => (Some(h), None),
                Err(e) => {
                    toast = Some((format!("Failed to start relay: {e}"), Instant::now()));
                    (None, Some(e))
                }
            }
        } else {
            (None, None)
        };
        let (tray, tray_ids) = build_tray();
        let (action_tx, action_rx) = channel();
        let (fw_tx, fw_rx) = channel();
        let mut app = App {
            relay,
            relay_err,
            rt,
            opts,
            qr_cache: None,
            qr_popup: false,
            devices_popup: false,
            _tray: tray,
            tray_ids,
            title_icon: None,
            headless: chrome::load_headless(),
            toast,
            action_tx,
            action_rx,
            action_pending: false,
            last_poll: Instant::now() - Duration::from_secs(2),
            quitting: false,
            _instance_lock: lock,
            #[cfg(windows)]
            hwnd: 0,
            #[cfg(windows)]
            region_applied: None,
            show_settings: false,
            fw_verdict: None,
            fw_rx,
            fw_tx,
            set_chrome_path: String::new(),
            set_theme: String::new(),
            set_chrome_port: String::new(),
            set_relay_addr: String::new(),
            set_relay_port: String::new(),
            set_hide_to_tray: false,
            set_auto_start_server: false,
            set_auto_launch_browser: false,
            applied_theme: String::new(),
        };
        // Check the firewall once at startup so the banner is there
        // before the user hits Start (and after a manual start).
        app.probe_firewall();
        app
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn run_action(&mut self, action: UiAction) {
        if self.action_pending {
            return;
        }
        self.action_pending = true;
        let tx = self.action_tx.clone();
        std::thread::spawn(move || {
            let r = match action {
                UiAction::LaunchChrome => chrome::launch_chrome().map(|_| "Chrome launched".to_string()),
                UiAction::CloseChrome => chrome::close_chrome().map(|_| "Chrome closed".to_string()),
            };
            let _ = tx.send(r);
        });
    }

    fn relay_state(&self) -> CoreState {
        match &self.relay {
            Some(r) => r.state(),
            None => CoreState::Down(
                self.relay_err
                    .clone()
                    // Default state of manual mode (auto-start off) is
                    // NOT a failure — the old copy ("relay failed to
                    // start") blamed the user before any start attempt.
                    .unwrap_or_else(|| "relay is off — press Start to run it".into()),
            ),
        }
    }

    fn restart_relay(&mut self) {
        match &mut self.relay {
            Some(r) => r.restart(),
            None => self.start_relay(),
        }
    }

    /// Manual server start (also used by restart when down).
    fn start_relay(&mut self) {
        match RelayHandle::start(self.rt.clone(), self.opts.clone()) {
            Ok(h) => {
                self.relay = Some(h);
                self.relay_err = None;
                self.toast("Server running".to_string());
                // Check whether the phone can actually reach us.
                self.probe_firewall();
                // Auto-launch Chrome when configured.
                if crate::config::get().auto_launch_browser {
                    self.run_action(UiAction::LaunchChrome);
                }
            }
            Err(e) => {
                self.relay_err = Some(e.clone());
                self.toast(format!("Failed to start relay: {e}"));
            }
        }
    }

    /// Probe the Windows Firewall in the background: the check shells
    /// out to PowerShell, which takes a second or two and must not
    /// block the UI thread.
    fn probe_firewall(&mut self) {
        let tx = self.fw_tx.clone();
        std::thread::spawn(move || {
            let exe = std::env::current_exe().unwrap_or_default();
            let port = crate::config::get().relay_port;
            let _ = tx.send(FwUpdate::Verdict(firewall::probe(&exe, port)));
        });
    }

    /// Ask Windows for an inbound allow rule (shows the elevation
    /// prompt), then re-probe so the banner disappears if it worked.
    fn request_firewall_rule(&mut self) {
        let tx = self.fw_tx.clone();
        std::thread::spawn(move || {
            let exe = std::env::current_exe().unwrap_or_default();
            let port = crate::config::get().relay_port;
            let added = firewall::add_allow_rule(&exe, port);
            let _ = tx.send(FwUpdate::RuleAdded(added));
            let _ = tx.send(FwUpdate::Verdict(firewall::probe(&exe, port)));
        });
    }

    fn stop_relay(&mut self) {
        if let Some(r) = &mut self.relay {
            r.stop();
        }
        self.relay = None;
        self.toast("Server stopped".to_string());
    }

    fn poll_tick(&mut self, _ctx: &egui::Context) {
        if self.last_poll.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_poll = Instant::now();
        // NOTE: the second-instance "show-panel" sentinel is watched
        // by a spawn_tray_watcher thread, not here: update() is not
        // called while the window is hidden.
        while let Ok(r) = self.action_rx.try_recv() {
            self.action_pending = false;
            match r {
                Ok(msg) => self.toast(msg),
                Err(e) => self.toast(e),
            }
        }
        while let Ok(msg) = self.fw_rx.try_recv() {
            match msg {
                FwUpdate::Verdict(v) => self.fw_verdict = Some(v),
                FwUpdate::RuleAdded(ok) => {
                    self.toast(if ok {
                        "Firewall rule added - allow the app if Windows asks.".to_string()
                    } else {
                        "Could not add the firewall rule.".to_string()
                    });
                }
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // NOTE: tray events are handled by spawn_tray_watcher threads,
        // not here: update() is not called while the window is hidden.
        #[cfg(windows)]
        self.apply_window_region(ctx);
        self.poll_tick(ctx);
        ctx.request_repaint_after(Duration::from_millis(250));

        // Apply theme when it changes (startup + settings save).
        let theme = crate::config::get().theme;
        if theme != self.applied_theme {
            self.applied_theme = theme;
            apply_theme(ctx);
        }

        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if crate::config::get().hide_to_tray {
                // Hide to the tray; Quit lives in the tray menu.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                // X really quits (default).
                self.quitting = true;
                quit_app();
            }
        }

        let state = self.relay_state();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::TRANSPARENT)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(ctx, |ui| {
                // Borderless opaque window; DWM rounds the corners
                // natively on Windows 11 (see dwm_round_corners). The
                // painted rounding is the fallback for older Windows.
                let full = ui.max_rect();
                ui.painter().rect_filled(full, 8.0, BG());

                // Custom title bar, part of the measured flow.
                let bar_h = 38.0;
                let (bar, bar_resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), bar_h),
                    egui::Sense::click_and_drag(),
                );
                if bar_resp.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                self.ui_title_bar_contents(ui, bar);
                ui.add_space(10.0);

                // Content takes its natural height with 14px side
                // margins; cards are forced to the full width.
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 14,
                        right: 14,
                        top: 0,
                        bottom: 0,
                    })
                    .show(ui, |ui| {
                        if self.show_settings {
                            // Sticky header: back (left) + save (right).
                            self.ui_settings_header(ui, ctx);
                            ui.add_space(4.0);
                        }
                        // Scrollable content: the window auto-fits up to
                        // 624px; taller content scrolls instead of clipping.
                        let scroll = egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                if self.show_settings {
                                    self.ui_settings_content(ui, ctx);
                                } else {
                                    self.ui_server_card(ui, &state);
                                    self.ui_chrome_card(ui, &state);
                                    self.ui_connection(ui, &state);
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new(
                                            "Keep the relay running while you use your phone. Tray icon reopens this panel.",
                                        )
                                        .size(11.5)
                                        .color(FAINT()),
                                    );
                                }
                            });
                        // Remember content height for the auto-fit below.
                        ui.data_mut(|d| {
                            d.insert_temp(
                                egui::Id::new("content_h"),
                                scroll.content_size.y,
                            )
                        });
                    });
                ui.add_space(10.0);

                // Fit the window to the content: this removes the
                // empty space at the bottom. Measured before the
                // overlay below allocates anything. Content taller
                // than the cap scrolls (see ScrollArea above).
                let content_h: f32 = ui.data(|d| {
                    d.get_temp(egui::Id::new("content_h")).unwrap_or(400.0)
                });
                // Title bar (38) + spacings/margins (~48).
                let used_h = content_h + 86.0;
                let target_h = used_h.clamp(320.0, 624.0);
                if (target_h - full.height()).abs() > 1.5 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                        egui::vec2(344.0, target_h),
                    ));
                }

                // QR popup as a direct overlay in this pass (no
                // egui::Area): dim scrim + centered card.
                self.ui_qr_overlay(ui, full, &state);
                // Pairing-request popup REMOVED (v0.9.16): the panel field
                // already shows the pending code (v0.9.14), so the popup
                // was redundant and confusing.
                // Device list popup.
                self.ui_devices_popup(ui, full);
            });
        self.ui_toast(ctx);
    }
}

impl App {
    /// Custom title bar for the borderless window: drag region,
    /// title + version, minimize and close (hide-to-tray) buttons.
    /// No maximize button. The bar rect itself is allocated (and
    /// drag-handled) by the caller so it stays inside the measured
    /// flow; buttons allocate on top of the drag rect.
    fn ui_title_bar_contents(&mut self, ui: &mut egui::Ui, bar: egui::Rect) {
        let painter = ui.painter().clone();
        // App icon left of the title (lazy texture from the embedded icon).
        if self.title_icon.is_none() {
            let img = egui::ColorImage::from_rgba_unmultiplied([64, 64], ICON_RGBA);
            self.title_icon = Some(ui.ctx().load_texture(
                "title-icon",
                img,
                egui::TextureOptions::LINEAR,
            ));
        }
        let icon_s = 18.0;
        if let Some(tex) = &self.title_icon {
            painter.image(
                tex.id(),
                egui::Rect::from_min_size(
                    egui::pos2(bar.min.x + 12.0, bar.center().y - icon_s / 2.0),
                    egui::vec2(icon_s, icon_s),
                ),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        let title_font = egui::FontId::proportional(13.0);
        painter.text(
            egui::pos2(bar.min.x + 38.0, bar.center().y),
            egui::Align2::LEFT_CENTER,
            "Browser Relay",
            title_font.clone(),
            TEXT(),
        );
        let title_w = painter
            .layout_no_wrap(
                "Browser Relay".to_string(),
                title_font,
                TEXT(),
            )
            .size()
            .x;
        painter.text(
            egui::pos2(bar.min.x + 38.0 + title_w + 8.0, bar.center().y),
            egui::Align2::LEFT_CENTER,
            format!("v{VERSION}"),
            egui::FontId::proportional(11.0),
            FAINT(),
        );
        // Window buttons, right aligned: minimize, then close.
        let btn_s = 30.0;
        let btn_y = bar.min.y + (bar.height() - btn_s) / 2.0;
        let mut bx = bar.max.x - 8.0 - btn_s;
        // close
        let close_r = egui::Rect::from_min_size(
            egui::pos2(bx, btn_y),
            egui::vec2(btn_s, btn_s),
        );
        let close_resp = ui.allocate_rect(close_r, egui::Sense::click());
        if close_resp.hovered() {
            painter.rect_filled(
                close_r,
                6.0,
                egui::Color32::from_rgb(0x5a, 0x28, 0x26),
            );
        }
        let c = close_r.center();
        let s = 5.5;
        let xc = if close_resp.hovered() {
            egui::Color32::WHITE
        } else {
            MUTED()
        };
        painter.line_segment(
            [egui::pos2(c.x - s, c.y - s), egui::pos2(c.x + s, c.y + s)],
            egui::Stroke::new(1.6, xc),
        );
        painter.line_segment(
            [egui::pos2(c.x - s, c.y + s), egui::pos2(c.x + s, c.y - s)],
            egui::Stroke::new(1.6, xc),
        );
        if close_resp.clicked() {
            // Same behavior as the native close path: hide to the tray
            // when configured, otherwise quit. The custom X always
            // hid, so the hide_to_tray setting was a lie.
            if crate::config::get().hide_to_tray {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                self.quitting = true;
                quit_app();
            }
        }
        // minimize
        bx -= btn_s + 2.0;
        let min_r =
            egui::Rect::from_min_size(egui::pos2(bx, btn_y), egui::vec2(btn_s, btn_s));
        let min_resp = ui.allocate_rect(min_r, egui::Sense::click());
        if min_resp.hovered() {
            painter.rect_filled(min_r, 6.0, ACTIVE());
        }
        let mc = min_r.center();
        painter.line_segment(
            [egui::pos2(mc.x - 5.5, mc.y), egui::pos2(mc.x + 5.5, mc.y)],
            egui::Stroke::new(1.6, MUTED()),
        );
        if min_resp.clicked() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        // settings gear
        bx -= btn_s + 2.0;
        let gear_r =
            egui::Rect::from_min_size(egui::pos2(bx, btn_y), egui::vec2(btn_s, btn_s));
        let gear_resp = ui.allocate_rect(gear_r, egui::Sense::click());
        if gear_resp.hovered() || self.show_settings {
            painter.rect_filled(gear_r, 6.0, ACTIVE());
        }
        let gc = gear_r.center();
        let gcol = if self.show_settings { ACCENT() } else { MUTED() };
        // simple gear: circle + spokes
        painter.circle_stroke(gc, 6.0, egui::Stroke::new(1.6, gcol));
        painter.circle_filled(gc, 2.0, gcol);
        for i in 0..8 {
            let a = i as f32 * std::f32::consts::PI / 4.0;
            let p1 = egui::pos2(gc.x + a.cos() * 6.0, gc.y + a.sin() * 6.0);
            let p2 = egui::pos2(gc.x + a.cos() * 8.5, gc.y + a.sin() * 8.5);
            painter.line_segment([p1, p2], egui::Stroke::new(1.6, gcol));
        }
        if gear_resp.clicked() {
            self.show_settings = !self.show_settings;
            if self.show_settings {
                let cfg = crate::config::get();
                self.set_chrome_path = cfg.chrome_path;
                self.set_theme = cfg.theme;
                self.set_chrome_port = cfg.chrome_port.to_string();
                self.set_relay_addr = cfg.relay_addr;
                self.set_relay_port = cfg.relay_port.to_string();
                self.set_hide_to_tray = cfg.hide_to_tray;
                self.set_auto_start_server = cfg.auto_start_server;
                self.set_auto_launch_browser = cfg.auto_launch_browser;
            }
        }
    }

    /// Server card: status + slim Start/Stop button inline on the right.
    fn ui_server_card(&mut self, ui: &mut egui::Ui, state: &CoreState) {
        Self::section_label(ui, "SERVER");
        let running = !matches!(state, CoreState::Down(_));
        card().show(ui, |ui| {
            force_full_width(ui);
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter().circle_filled(
                    rect.center(),
                    5.0,
                    if running { GREEN() } else { RED() },
                );
                ui.add_space(4.0);
                // Status text takes remaining space.
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(if running {
                            "Server running"
                        } else {
                            "Server stopped"
                        })
                        .strong(),
                    );
                    if let CoreState::Running(st) = state {
                        // Show all reachable URLs (one per network interface).
                        for url in &st.phone_urls {
                            ui.label(
                                egui::RichText::new(url)
                                    .size(11.5)
                                    .monospace()
                                    .color(MUTED()),
                            );
                        }
                    } else if let CoreState::Down(reason) = state {
                        ui.label(
                            egui::RichText::new(reason.clone())
                                .size(12.0)
                                .color(MUTED()),
                        );
                    }
                });
                // Slim button on the right, vertically centered.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, fill) = if running {
                        ("Stop", RED())
                    } else {
                        ("Start", ACCENT())
                    };
                    let btn = egui::Button::new(
                        egui::RichText::new(label).size(12.5).color(egui::Color32::WHITE),
                    )
                    .fill(fill)
                    .corner_radius(7.0)
                    .min_size(egui::vec2(64.0, 30.0));
                    if ui.add(btn).clicked() {
                        if running {
                            self.stop_relay();
                        } else {
                            self.start_relay();
                        }
                    }
                });
            });
            // A3: the server binds fine even when Windows Firewall
            // blocks inbound traffic, so "Server running" used to be
            // a lie when the user declined the prompt.
            if running && self.fw_verdict == Some(firewall::Verdict::Blocked) {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Windows Firewall is blocking incoming connections")
                        .size(12.0)
                        .color(AMBER()),
                );
                ui.label(
                    egui::RichText::new(
                        "Your phone cannot reach this PC until the relay is allowed through the firewall.",
                    )
                    .size(11.0)
                    .color(FAINT()),
                );
                ui.horizontal(|ui| {
                    if ui
                        .small_button(egui::RichText::new("Allow relay").size(11.5).color(AMBER()))
                        .clicked()
                    {
                        self.request_firewall_rule();
                    }
                    if ui
                        .small_button(egui::RichText::new("Firewall settings").size(11.5).color(MUTED()))
                        .clicked()
                    {
                        firewall::open_settings();
                    }
                    if ui
                        .small_button(egui::RichText::new("Re-check").size(11.5).color(MUTED()))
                        .clicked()
                    {
                        self.probe_firewall();
                    }
                });
            }
        });
        ui.add_space(4.0);
    }

    /// Chrome card: status + slim Launch/Close button inline on the right.
    fn ui_chrome_card(&mut self, ui: &mut egui::Ui, state: &CoreState) {
        Self::section_label(ui, "CHROME");
        let (dot_color, title, sub): (egui::Color32, String, String) = match state {
            CoreState::Starting => (FAINT(), "Starting\u{2026}".into(), "starting the relay".into()),
            CoreState::Running(st) if st.connected => (
                GREEN(),
                "Chrome connected".into(),
                if st.clients == 1 {
                    "1 device driving".into()
                } else {
                    format!("{} devices connected", st.clients)
                },
            ),
            CoreState::Running(_) => (
                RED(),
                "Chrome not running".into(),
                "launch it to start driving".into(),
            ),
            CoreState::Down(_) => (
                FAINT(),
                "Server stopped".into(),
                "start the server first".into(),
            ),
        };
        let server_up = !matches!(state, CoreState::Down(_));
        let chrome_running = matches!(state, CoreState::Running(st) if st.connected);
        card().show(ui, |ui| {
            force_full_width(ui);
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                if dot_color == GREEN() {
                    ui.painter().circle_filled(
                        rect.center(),
                        6.0,
                        egui::Color32::from_rgba_premultiplied(0x3f, 0xb9, 0x50, 0x45),
                    );
                }
                ui.painter().circle_filled(rect.center(), 5.0, dot_color);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(title).strong());
                    ui.label(egui::RichText::new(sub).size(12.0).color(MUTED()));
                });
                // Slim Launch/Close button on the right.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, fill) = if chrome_running {
                        ("Close", RED())
                    } else {
                        ("Launch", ACCENT())
                    };
                    let btn = egui::Button::new(
                        egui::RichText::new(label).size(12.5).color(egui::Color32::WHITE),
                    )
                    .fill(if server_up { fill } else { FAINT() })
                    .corner_radius(7.0)
                    .min_size(egui::vec2(64.0, 30.0));
                    if ui.add_enabled(server_up, btn).clicked() {
                        self.run_action(if chrome_running {
                            UiAction::CloseChrome
                        } else {
                            UiAction::LaunchChrome
                        });
                    }
                });
            });
            ui.add_space(8.0);
            if ui
                .checkbox(&mut self.headless, "Headless Chrome (no window)")
                .changed()
            {
                chrome::save_headless(self.headless);
            }
        });
        ui.add_space(4.0);
    }

    fn section_label(ui: &mut egui::Ui, text: &str) {
        ui.add_space(10.0);
        ui.label(egui::RichText::new(text).size(11.0).color(FAINT()).strong());
        ui.add_space(6.0);
    }


    fn ui_connection(&mut self, ui: &mut egui::Ui, state: &CoreState) {
        Self::section_label(ui, "CONNECTION");
        let (url, code, clients) = match state {
            CoreState::Running(st) => (
                st.phone_url.clone(),
                st.code.clone(),
                st.clients.to_string(),
            ),
            _ => ("—".to_string(), "—".to_string(), "0".to_string()),
        };
        card().show(ui, |ui| {
            force_full_width(ui);
            ui.label(egui::RichText::new("Relay URL").size(12.0).color(MUTED()));
            ui.add_space(4.0);
            self.copy_field(ui, &url, 12.0, false);
            ui.add_space(10.0);
            ui.label(egui::RichText::new("Pairing code").size(12.0).color(MUTED()));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let spaced: String = code
                    .chars()
                    .map(|c| c.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                // Field takes remaining space; QR button matches field height.
                let show_qr = matches!(state, CoreState::Running(_));
                let qr_w = if show_qr { 42.0 } else { 0.0 };
                let field_w = (ui.available_width() - qr_w).max(80.0);
                let field_rect = ui
                    .allocate_ui(egui::vec2(field_w, 0.0), |ui| {
                        self.copy_field(ui, &spaced, 20.0, true);
                    })
                    .response
                    .rect;
                if show_qr {
                    // Same top and same height as the field. Nesting it
                    // in a layout kept landing the button a few pixels
                    // high (measured 6 px on screen), so it is placed on
                    // the field's own rect instead of being aligned.
                    let side = field_rect.height();
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(ui.cursor().left(), field_rect.top()),
                        egui::vec2(side, side),
                    );
                    self.ui_qr_button_on(ui, rect);
                }
            });
            // Pending pairing request: the code shown above IS this
            // requester's code (the dedicated popup was removed in
            // v0.9.16, leaving no trace of who is asking).
            if let Some(req) = self.relay.as_ref().and_then(|r| r.pair_request()) {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "Pairing request: {} ({}) — that code is for them",
                            req.name, req.ip
                        ))
                        .size(11.5)
                        .color(ACCENT()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button(egui::RichText::new("Cancel").size(11.5).color(RED()))
                            .clicked()
                        {
                            if let Some(r) = &self.relay {
                                r.cancel_pair_request();
                            }
                        }
                    });
                });
            }
            // Code expiry: the pairing code rotates every 5 minutes -
            // without this the digits changed silently under the user.
            if let CoreState::Running(st) = state {
                let secs = st.code_expires_in;
                let txt = if secs == 0 {
                    "expired - a fresh code is generated automatically".to_string()
                } else {
                    format!("expires in {}m {:02}s", secs / 60, secs % 60)
                };
                ui.label(egui::RichText::new(txt).size(10.5).color(FAINT()));
            }
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(4.0);
            // Connected device row: click -> device list popup.
            let (cdr, cdresp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 28.0),
                egui::Sense::click(),
            );
            if cdresp.hovered() {
                ui.painter().rect_filled(cdr, 6.0, CARD());
            }
            ui.painter().text(
                egui::pos2(cdr.min.x + 4.0, cdr.center().y),
                egui::Align2::LEFT_CENTER,
                "Connected devices",
                egui::FontId::proportional(12.5),
                MUTED(),
            );
            ui.painter().text(
                egui::pos2(cdr.max.x - 4.0, cdr.center().y),
                egui::Align2::RIGHT_CENTER,
                format!("{clients} ›"),
                egui::FontId::proportional(12.5),
                TEXT(),
            );
            if cdresp.clicked() {
                self.devices_popup = true;
            }

            // Live sessions, inline: this used to live at the bottom
            // of the Settings page, where a 624px window put it past
            // the fold. Device state is live information, not a
            // preference.
            let sessions: Vec<_> = self
                .relay
                .as_ref()
                .and_then(|r| r.hub())
                .map(|h| h.client_list())
                .unwrap_or_default();
            if !sessions.is_empty() {
                // Names of paired devices, for a friendlier label.
                let names: std::collections::HashMap<String, String> = self
                    .relay
                    .as_ref()
                    .map(|r| {
                        r.paired_devices()
                            .into_iter()
                            .map(|(fp, d)| (fp, d.name))
                            .collect()
                    })
                    .unwrap_or_default();
                ui.add_space(6.0);
                for c in &sessions {
                    let short_id: String = c.fingerprint.chars().take(6).collect();
                    let title = if c.fingerprint.is_empty() {
                        format!("New device · {}", c.peer)
                    } else {
                        names
                            .get(&c.fingerprint)
                            .cloned()
                            .unwrap_or_else(|| format!("#{} · {}", short_id, c.peer))
                    };
                    let role = if c.driver {
                        "driver"
                    } else if c.authed {
                        "viewer"
                    } else {
                        "pairing…"
                    };
                    let ago = ago_str(c.connected_at);
                    ui.horizontal(|ui| {
                        let (dot, _) =
                            ui.allocate_exact_size(egui::vec2(10.0, 16.0), egui::Sense::hover());
                        ui.painter()
                            .circle_filled(dot.center(), 4.0, if c.driver { ACCENT() } else { MUTED() });
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(title).size(12.0).color(TEXT()));
                            ui.label(
                                egui::RichText::new(format!("{role} · {ago}"))
                                    .size(10.5)
                                    .color(FAINT()),
                            );
                        });
                    });
                }
            }
        });
    }

    /// Full-width field with an embedded copy icon on the left.
    /// Clicking the icon copies `text` (unspaced copy for codes).
    fn copy_field(&mut self, ui: &mut egui::Ui, text: &str, font_size: f32, strong: bool) {
        let copy_text = text.replace(' ', "");
        field_box().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // Copy icon (two overlapping rects), clickable.
                let (ir, iresp) =
                    ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
                let p = ui.painter();
                let ic = if iresp.hovered() { ACCENT() } else { MUTED() };
                let r = egui::Rect::from_center_size(ir.center(), egui::vec2(13.0, 13.0));
                // Back rect.
                p.rect_stroke(
                    egui::Rect::from_min_size(
                        egui::pos2(r.min.x + 3.0, r.min.y),
                        egui::vec2(8.0, 10.0),
                    ),
                    2.0,
                    egui::Stroke::new(1.5, ic),
                    egui::StrokeKind::Inside,
                );
                // Front rect.
                p.rect_filled(
                    egui::Rect::from_min_size(
                        egui::pos2(r.min.x, r.min.y + 3.0),
                        egui::vec2(8.0, 10.0),
                    ),
                    2.0,
                    CARD(),
                );
                p.rect_stroke(
                    egui::Rect::from_min_size(
                        egui::pos2(r.min.x, r.min.y + 3.0),
                        egui::vec2(8.0, 10.0),
                    ),
                    2.0,
                    egui::Stroke::new(1.5, ic),
                    egui::StrokeKind::Inside,
                );
                if iresp.clicked() {
                    ui.ctx().copy_text(copy_text.clone());
                    self.toast("Copied");
                }
                let rt = egui::RichText::new(text).monospace().size(font_size);
                ui.label(if strong { rt.strong() } else { rt });
            });
        });
    }

    /// Pairing URL for the QR: {phone_url}/pair?c={code}. The phone
    /// scans it with any QR scanner; the landing page sends it to the
    /// app (intent://, if installed) or the pre-paired web client.
    fn pair_url(url: &str, code: &str) -> Option<String> {
        if code.len() != 6
            || !code.bytes().all(|b| b.is_ascii_digit())
            || !url.starts_with("http")
        {
            return None;
        }
        Some(format!("{url}/pair?c={code}"))
    }

    /// Pairing URL for the QR: {phone_url}/pair?c={code}. The phone
    /// Cached QR module grid for the current pairing URL.
    fn qr_modules(&mut self, pair: &str) -> Option<(&[bool], usize)> {
        let stale = self
            .qr_cache
            .as_ref()
            .map(|(u, _, _)| u != pair)
            .unwrap_or(true);
        if stale {
            self.qr_cache = qrcode::QrCode::new(pair.as_bytes()).ok().map(|qc| {
                let width = qc.width();
                let modules: Vec<bool> = qc
                    .to_colors()
                    .into_iter()
                    .map(|c| c == qrcode::Color::Dark)
                    .collect();
                (pair.to_string(), modules, width)
            });
        }
        self.qr_cache
            .as_ref()
            .map(|(_, m, w)| (m.as_slice(), *w))
    }

    /// Draw the QR (dark modules on white, 4-module quiet zone per
    /// the QR spec — 2 modules failed to scan reliably at low light)
    /// into rect.
    fn draw_qr(painter: &egui::Painter, rect: egui::Rect, modules: &[bool], width: usize) {
        let total = width + 8;
        let cell = (rect.width() / total as f32).floor().max(1.0);
        let side = cell * total as f32;
        let origin = egui::pos2(
            rect.center().x - side / 2.0,
            rect.center().y - side / 2.0,
        );
        painter.rect_filled(
            egui::Rect::from_min_size(origin, egui::vec2(side, side)),
            3.0,
            egui::Color32::WHITE,
        );
        let dark = egui::Color32::from_rgb(0x14, 0x14, 0x14);
        for row in 0..width {
            for col in 0..width {
                if modules[row * width + col] {
                    painter.rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(
                                origin.x + (col + 4) as f32 * cell,
                                origin.y + (row + 4) as f32 * cell,
                            ),
                            egui::vec2(cell, cell),
                        ),
                        0.0,
                        dark,
                    );
                }
            }
        }
    }

    /// Settings view: Chrome path, theme, Chrome DevTools port,
    /// relay listen address/port, and the paired-clients list.
    /// Sticky settings header: back arrow (left), save icon (right).
    /// Drawn outside the scroll area so it never scrolls away.
    fn ui_settings_header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            // Back: arrow icon, left.
            let (br, bresp) =
                ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::click());
            if bresp.hovered() {
                ui.painter().rect_filled(br, 8.0, CARD());
            }
            // Arrow-left glyph.
            let c = br.center();
            ui.painter().line_segment(
                [egui::pos2(c.x + 6.0, c.y), egui::pos2(c.x - 6.0, c.y)],
                egui::Stroke::new(2.0, TEXT()),
            );
            ui.painter().line_segment(
                [egui::pos2(c.x - 6.0, c.y), egui::pos2(c.x - 1.0, c.y - 5.0)],
                egui::Stroke::new(2.0, TEXT()),
            );
            ui.painter().line_segment(
                [egui::pos2(c.x - 6.0, c.y), egui::pos2(c.x - 1.0, c.y + 5.0)],
                egui::Stroke::new(2.0, TEXT()),
            );
            if bresp.clicked() {
                self.show_settings = false;
            }
            // Title centered.
            ui.with_layout(
                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                |ui| {
                    ui.label(
                        egui::RichText::new("Settings")
                            .size(15.0)
                            .strong()
                            .color(TEXT()),
                    );
                },
            );
            // Save: disk icon, right.
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    let (sr, sresp) =
                        ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::click());
                    if sresp.hovered() {
                        ui.painter().rect_filled(sr, 8.0, CARD());
                    }
                    // Floppy-disk glyph.
                    let sc = sr.center();
                    let s = 7.0;
                    ui.painter().rect_stroke(
                        egui::Rect::from_center_size(sc, egui::vec2(s * 2.0, s * 2.0)),
                        2.0,
                        egui::Stroke::new(1.8, ACCENT()),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(sc.x - s + 3.0, sc.y - s),
                            egui::vec2(s * 2.0 - 6.0, 6.0),
                        ),
                        1.0,
                        ACCENT(),
                    );
                    if sresp.clicked() {
                        self.save_settings(ctx);
                    }
                },
            );
        });
        ui.add_space(2.0);
        ui.separator();
    }

    /// Settings view, in the app's design language: section labels,
    /// cards, field boxes, purple accent for the primary action.
    fn ui_settings_content(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {

        // ---- CHROME ----
        Self::section_label(ui, "CHROME");
        card().show(ui, |ui| {
            force_full_width(ui);
            ui.label(egui::RichText::new("Executable").size(12.0).color(MUTED()));
            ui.add_space(4.0);
            field_box().show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.set_chrome_path)
                        .hint_text("auto-detect")
                        .desired_width(f32::INFINITY)
                        .frame(false),
                );
            });
            ui.label(
                egui::RichText::new("Empty = auto-detect.")
                    .size(11.0)
                    .color(FAINT()),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("DevTools port").size(12.0).color(MUTED()),
                );
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        field_box().show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.set_chrome_port)
                                    .desired_width(64.0)
                                    .frame(false),
                            );
                        });
                    },
                );
            });
            ui.label(
                egui::RichText::new("Applies on next Chrome launch.")
                    .size(11.0)
                    .color(FAINT()),
            );
        });

        // ---- APPEARANCE ----
        Self::section_label(ui, "APPEARANCE");
        card().show(ui, |ui| {
            force_full_width(ui);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Theme").size(12.0).color(MUTED()));
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        egui::ComboBox::from_id_salt("theme_combo")
                            .selected_text(if self.set_theme == "light" {
                                "Light"
                            } else {
                                "Dark"
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.set_theme,
                                    "dark".to_string(),
                                    "Dark",
                                );
                                ui.selectable_value(
                                    &mut self.set_theme,
                                    "light".to_string(),
                                    "Light",
                                );
                            });
                    },
                );
            });
            ui.add_space(8.0);
            ui.checkbox(
                &mut self.set_hide_to_tray,
                egui::RichText::new("Hide to tray when clicking X").size(12.5).color(TEXT()),
            );
            ui.label(
                egui::RichText::new("Default: X quits the app.")
                    .size(11.0)
                    .color(FAINT()),
            );
        });

        // ---- RELAY SERVER ----
        Self::section_label(ui, "RELAY SERVER");
        card().show(ui, |ui| {
            force_full_width(ui);
            ui.label(
                egui::RichText::new("Listen address").size(12.0).color(MUTED()),
            );
            ui.add_space(4.0);
            field_box().show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.set_relay_addr)
                        .hint_text("auto (all interfaces: WiFi + hotspot)")
                        .desired_width(f32::INFINITY)
                        .frame(false),
                );
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Listen port").size(12.0).color(MUTED()));
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        field_box().show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.set_relay_port)
                                    .desired_width(64.0)
                                    .frame(false),
                            );
                        });
                    },
                );
            });
            ui.label(
                egui::RichText::new("Restart the relay to apply address/port changes.")
                    .size(11.0)
                    .color(FAINT()),
            );
            ui.add_space(10.0);
            ui.checkbox(
                &mut self.set_auto_start_server,
                egui::RichText::new("Auto-start server when the panel opens").size(12.5).color(TEXT()),
            );
            ui.label(
                egui::RichText::new("Default off: avoids the Windows Firewall prompt on every launch.")
                    .size(11.0)
                    .color(FAINT()),
            );
            ui.add_space(6.0);
            ui.checkbox(
                &mut self.set_auto_launch_browser,
                egui::RichText::new("Auto-launch Chrome when the server starts").size(12.5).color(TEXT()),
            );
        });

    }

    /// Validate + persist settings, apply theme live.
    fn save_settings(&mut self, ctx: &egui::Context) {
        let mut port_errs: Vec<String> = Vec::new();
        let chrome_port: u16 = match self.set_chrome_port.trim().parse() {
            Ok(p) if p > 0 => p,
            _ => {
                port_errs.push("DevTools port invalid — kept 9222".to_string());
                9222
            }
        };
        let relay_port: u16 = match self.set_relay_port.trim().parse() {
            Ok(p) if p > 0 => p,
            _ => {
                port_errs.push("Listen port invalid — kept 8765".to_string());
                8765
            }
        };
        let chrome_path = self.set_chrome_path.trim().to_string();
        let theme = if self.set_theme == "light" {
            "light".to_string()
        } else {
            "dark".to_string()
        };
        let relay_addr = self.set_relay_addr.trim().to_string();
        if !chrome_path.is_empty() && !std::path::Path::new(&chrome_path).exists() {
            self.toast("Chrome path does not exist — not saved.".to_string());
            return;
        }
        crate::config::update(|c| {
            c.chrome_path = chrome_path;
            c.theme = theme;
            c.chrome_port = chrome_port;
            c.relay_addr = relay_addr;
            c.relay_port = relay_port;
            c.hide_to_tray = self.set_hide_to_tray;
            c.auto_start_server = self.set_auto_start_server;
            c.auto_launch_browser = self.set_auto_launch_browser;
        });
        apply_theme(ctx);
        // Surface invalid inputs instead of silently swapping in the
        // default (the old code kept "Settings saved." while quietly
        // reverting a typo'd port).
        if port_errs.is_empty() {
            self.toast("Settings saved. Restart the server to apply network changes.");
        } else {
            self.toast(format!("{} — other settings saved.", port_errs.join("; ")));
        }
    }

    fn ui_qr_button_sized(&mut self, ui: &mut egui::Ui, size: f32) {
        let s = size.max(28.0);
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(s, s), egui::Sense::click());
        self.paint_qr_button(ui, rect, &resp);
    }

    /// The QR button drawn on an exact rect, for a caller that needs it
    /// on a line of its own choosing (the pairing code row does: the
    /// button must share the field's top and height).
    fn ui_qr_button_on(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let resp = ui.allocate_rect(rect, egui::Sense::click());
        self.paint_qr_button(ui, rect, &resp);
    }

    fn paint_qr_button(&mut self, ui: &egui::Ui, rect: egui::Rect, resp: &egui::Response) {
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            if resp.hovered() {
                painter.rect_filled(rect, 6.0, ACTIVE());
            }
            painter.rect_stroke(
                rect,
                6.0,
                egui::Stroke::new(1.0, BORDER()),
                egui::StrokeKind::Inside,
            );
            // Mini QR glyph on a 7x7 module grid: three finder
            // patterns (ring + filled center) + data modules.
            let q = egui::Color32::from_rgb(0xda, 0xda, 0xda);
            let m = 2.6; // module size
            let ox = rect.center().x - 3.5 * m;
            let oy = rect.center().y - 3.5 * m;
            let cell = |c: i32, r: i32| {
                egui::Rect::from_min_size(
                    egui::pos2(ox + c as f32 * m, oy + r as f32 * m),
                    egui::vec2(m, m),
                )
            };
            // Finder = 3x3 ring with a filled center module.
            for (fc, fr) in [(0, 0), (4, 0), (0, 4)] {
                for dc in 0..3 {
                    for dr in 0..3 {
                        let edge = dc == 0 || dc == 2 || dr == 0 || dr == 2;
                        let center = dc == 1 && dr == 1;
                        if edge || center {
                            painter.rect_filled(
                                cell(fc + dc, fr + dr),
                                0.0,
                                q,
                            );
                        }
                    }
                }
            }
            // Data modules.
            for (c, r) in [(4, 4), (5, 5), (6, 4), (5, 3), (3, 5), (6, 6)] {
                painter.rect_filled(cell(c, r), 0.0, q);
            }
        }
        if resp.clicked() {
            self.qr_popup = true;
        }
    }

    /// In-panel QR popup drawn as a direct overlay in the main UI
    /// pass (deliberately no egui::Area): dim scrim + centered
    /// card. Click outside, the X, or Esc to close.
    fn ui_qr_overlay(&mut self, ui: &mut egui::Ui, full: egui::Rect, state: &CoreState) {
        if !self.qr_popup {
            return;
        }
        let (url, code) = match state {
            CoreState::Running(st) => (st.phone_url.clone(), st.code.clone()),
            _ => {
                self.qr_popup = false;
                return;
            }
        };
        let Some(pair) = Self::pair_url(&url, &code) else {
            self.qr_popup = false;
            return;
        };
        if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
            self.qr_popup = false;
            return;
        }
        let qr = self.qr_modules(&pair).map(|(m, w)| (m.to_vec(), w));
        let Some((modules, width)) = qr else {
            self.qr_popup = false;
            return;
        };
        let painter = ui.painter().clone();
        // Scrim, allocated first so it sits below the card chrome.
        let scrim = ui.allocate_rect(full, egui::Sense::click());
        painter.rect_filled(full, 8.0, egui::Color32::from_black_alpha(150));
        // Card.
        let card =
            egui::Rect::from_center_size(full.center(), egui::vec2(252.0, 316.0));
        painter.rect_filled(card, 8.0, CARD());
        painter.rect_stroke(
            card,
            8.0,
            egui::Stroke::new(1.0, BORDER()),
            egui::StrokeKind::Inside,
        );
        // X button, allocated after the scrim so it wins hit-testing.
        let xr = egui::Rect::from_min_size(
            egui::pos2(card.max.x - 34.0, card.min.y + 6.0),
            egui::vec2(26.0, 26.0),
        );
        let xresp = ui.allocate_rect(xr, egui::Sense::click());
        if xresp.hovered() {
            painter.rect_filled(xr, 6.0, ACTIVE());
        }
        let xc = xr.center();
        let s = 5.0;
        painter.line_segment(
            [
                egui::pos2(xc.x - s, xc.y - s),
                egui::pos2(xc.x + s, xc.y + s),
            ],
            egui::Stroke::new(1.6, MUTED()),
        );
        painter.line_segment(
            [
                egui::pos2(xc.x - s, xc.y + s),
                egui::pos2(xc.x + s, xc.y - s),
            ],
            egui::Stroke::new(1.6, MUTED()),
        );
        // QR + caption (paint only).
        let qr_r = egui::Rect::from_center_size(
            egui::pos2(card.center().x, card.min.y + 40.0 + 104.0),
            egui::vec2(208.0, 208.0),
        );
        Self::draw_qr(&painter, qr_r, &modules, width);
        painter.text(
            egui::pos2(card.center().x, card.max.y - 26.0),
            egui::Align2::CENTER_CENTER,
            "Scan to pair",
            egui::FontId::proportional(12.5),
            MUTED(),
        );
        // Clicks are checked last, after every allocation, so the X
        // button (allocated later = on top) wins hit-testing.
        if xresp.clicked() || scrim.clicked() {
            self.qr_popup = false;
        }
    }

    /// Device list popup: paired devices with online status + revoke.
    /// Opened from the "Connected device" row.
    fn ui_devices_popup(&mut self, ui: &mut egui::Ui, full: egui::Rect) {
        if !self.devices_popup {
            return;
        }
        if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
            self.devices_popup = false;
            return;
        }
        let paired = self
            .relay
            .as_ref()
            .map(|r| r.paired_devices())
            .unwrap_or_default();
        let online: std::collections::HashSet<String> = self
            .relay
            .as_ref()
            .and_then(|r| r.hub())
            .map(|h| {
                h.client_list()
                    .into_iter()
                    .filter(|c| c.authed)
                    .map(|c| c.fingerprint)
                    .collect()
            })
            .unwrap_or_default();
        let painter = ui.painter().clone();
        painter.rect_filled(full, 0.0, egui::Color32::from_black_alpha(140));
        let scrim = ui.allocate_rect(full, egui::Sense::click());
        let card_w = 300.0;
        let card_h = 320.0;
        let card = egui::Rect::from_center_size(full.center(), egui::vec2(card_w, card_h));
        painter.rect_filled(card, 14.0, CARD());
        // Title.
        painter.text(
            egui::pos2(card.center().x, card.min.y + 16.0),
            egui::Align2::CENTER_TOP,
            "Devices",
            egui::FontId::proportional(15.0),
            TEXT(),
        );
        // Close X (drawn manually — font glyphs like ✕ may not render).
        let xr = egui::Rect::from_center_size(
            egui::pos2(card.max.x - 20.0, card.min.y + 20.0),
            egui::vec2(28.0, 28.0),
        );
        let xresp = ui.allocate_rect(xr, egui::Sense::click());
        if xresp.hovered() {
            painter.circle_filled(xr.center(), 13.0, ACTIVE());
        }
        let xw = 6.0;
        let xc = xr.center();
        painter.line_segment(
            [egui::pos2(xc.x - xw, xc.y - xw), egui::pos2(xc.x + xw, xc.y + xw)],
            egui::Stroke::new(2.0, MUTED()),
        );
        painter.line_segment(
            [egui::pos2(xc.x + xw, xc.y - xw), egui::pos2(xc.x - xw, xc.y + xw)],
            egui::Stroke::new(2.0, MUTED()),
        );
        if xresp.clicked() {
            self.devices_popup = false;
            return;
        }
        // List area (manual layout inside the card).
        let list_top = card.min.y + 52.0;
        let list = egui::Rect::from_min_max(
            egui::pos2(card.min.x + 14.0, list_top),
            egui::pos2(card.max.x - 14.0, card.max.y - 14.0),
        );
        if paired.is_empty() {
            painter.text(
                list.center(),
                egui::Align2::CENTER_CENTER,
                "No paired devices yet.",
                egui::FontId::proportional(12.0),
                FAINT(),
            );
        } else {
            let mut y = list.min.y;
            let mut revoke: Option<String> = None;
            // Staged confirmation: one mis-click used to kill a paired
            // device's live session immediately.
            let mut revoke_stage: Option<String> = None;
            for (fp, dev) in &paired {
                if y + 44.0 > list.max.y {
                    break;
                }
                let row = egui::Rect::from_min_size(
                    egui::pos2(list.min.x, y),
                    egui::vec2(list.width(), 40.0),
                );
                // Online dot.
                let dot_col = if online.contains(fp) { GREEN() } else { FAINT() };
                painter.circle_filled(
                    egui::pos2(row.min.x + 6.0, row.center().y),
                    4.0,
                    dot_col,
                );
                painter.text(
                    egui::pos2(row.min.x + 18.0, row.min.y + 4.0),
                    egui::Align2::LEFT_TOP,
                    dev.name.clone(),
                    egui::FontId::proportional(13.0),
                    TEXT(),
                );
                painter.text(
                    egui::pos2(row.min.x + 18.0, row.min.y + 22.0),
                    egui::Align2::LEFT_TOP,
                    format!(
                        "#{} · {}",
                        fp.chars().take(6).collect::<String>(),
                        ago_str(dev.last_seen)
                    ),
                    egui::FontId::proportional(11.0),
                    FAINT(),
                );
                // Revoke button.
                let br = egui::Rect::from_min_size(
                    egui::pos2(row.max.x - 52.0, row.center().y - 12.0),
                    egui::vec2(52.0, 24.0),
                );
                let bresp = ui.allocate_rect(br, egui::Sense::click());
                let staged = revoke_stage.as_deref() == Some(fp.as_str());
                painter.rect_filled(br, 6.0, if staged { ACTIVE() } else { RAISED() });
                painter.text(
                    br.center(),
                    egui::Align2::CENTER_CENTER,
                    if staged { "Sure?" } else { "Revoke" },
                    egui::FontId::proportional(11.0),
                    if staged { RED() } else { MUTED() },
                );
                if bresp.clicked() {
                    if staged {
                        revoke = Some(fp.clone());
                    } else {
                        revoke_stage = Some(fp.clone());
                    }
                }
                y += 44.0;
            }
            if let Some(fp) = revoke {
                if let Some(r) = &self.relay {
                    if r.revoke_device(&fp) {
                        self.toast("Device revoked.".to_string());
                    }
                }
            }
        }
        if scrim.clicked() {
            self.devices_popup = false;
        }
    }

    fn ui_toast(&mut self, ctx: &egui::Context) {
        let Some((msg, at)) = &self.toast else { return };
        if at.elapsed() > Duration::from_millis(2000) {
            self.toast = None;
            return;
        }
        let msg = msg.clone();
        egui::Area::new(egui::Id::new("toast"))
            // Top, below the title bar: the old bottom anchor sat ON
            // the footer hint text (overlapping layers, both visible).
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 46.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(RAISED())
                    .stroke(egui::Stroke::new(1.0, BORDER()))
                    .corner_radius(20)
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(msg).size(12.0));
                    });
            });
    }
}

fn build_tray() -> (Option<TrayIcon>, TrayIds) {
    let ids = TrayIds {
        open: MenuId::new("open-panel"),
        launch: MenuId::new("launch-chrome"),
        close: MenuId::new("close-chrome"),
        quit: MenuId::new("quit"),
    };
    let menu = Menu::new();
    let open_i = MenuItem::with_id(ids.open.clone(), "Open panel", true, None);
    let launch_i = MenuItem::with_id(ids.launch.clone(), "Launch Chrome", true, None);
    let close_i = MenuItem::with_id(ids.close.clone(), "Close Chrome", true, None);
    let quit_i = MenuItem::with_id(ids.quit.clone(), "Quit", true, None);
    let _ = menu.append(&open_i);
    let _ = menu.append(&launch_i);
    let _ = menu.append(&close_i);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit_i);
    let icon = match tray_icon::Icon::from_rgba(ICON_RGBA.to_vec(), ICON_SIZE, ICON_SIZE) {
        Ok(i) => i,
        Err(_) => return (None, ids),
    };
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon)
        .with_tooltip(format!("Browser Relay v{VERSION}"))
        .build()
        .ok();
    (tray, ids)
}

fn main() -> eframe::Result<()> {
    config::init();
    relay::init_log();
    let (nogui, opts) = relay::parse_args();
    let rt = std::sync::Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime"),
    );

    if nogui {
        // Console mode (test surface + headless use): relay only,
        // no window. Blocks until Ctrl-C / SIGTERM.
        return match RelayHandle::start(rt.clone(), opts) {
            Ok(h) => {
                println!("browser-relay service v{VERSION} (rust)");
                println!();
                println!("  open on your phone (same Wi-Fi):  {}", h.phone_url());
                println!("  pairing code:                      {}", h.code());
                println!();
                rt.block_on(async {
                    let _ = tokio::signal::ctrl_c().await;
                });
                Ok(())
            }
            Err(e) => {
                eprintln!("fatal: {e}");
                std::process::exit(1);
            }
        };
    }

    // Single instance: hold a loopback listener for our lifetime. A
    // second launch asks the running instance to show its panel via a
    // sentinel file, then exits.
    let lock = TcpListener::bind(("127.0.0.1", INSTANCE_LOCK_PORT)).ok();
    if lock.is_none() {
        chrome::ensure_config_dir();
        let _ = fs::write(chrome::config_dir().join("show-panel"), "1");
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Browser Relay")
            // Borderless modern panel: custom title bar, DWM-rounded
            // corners (Win11), no maximize, no user resizing. Height
            // auto-fits the content at runtime (344 wide, 320..624).
            // NOTE: the window must stay OPAQUE: per Microsoft's docs,
            // per-pixel alpha (transparent) windows can NEVER be
            // DWM-rounded, even via the opt-in API — which is why the
            // transparent attempt showed square corners.
            .with_decorations(false)
            .with_transparent(false)
            .with_resizable(false)
            .with_inner_size([344.0, 540.0])
            .with_min_inner_size([344.0, 320.0])
            .with_max_inner_size([344.0, 624.0])
            .with_icon(egui::IconData {
                rgba: ICON_RGBA.to_vec(),
                width: ICON_SIZE,
                height: ICON_SIZE,
            }),
        ..Default::default()
    };
    eframe::run_native(
        "browser-relay",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(styled_visuals());
            let mut app = App::new(lock, rt, opts);
            #[cfg(windows)]
            {
                use raw_window_handle::HasWindowHandle as _;
                if let Ok(h) = cc.window_handle() {
                    if let raw_window_handle::RawWindowHandle::Win32(w) = h.as_raw()
                    {
                        let hwnd = w.hwnd.get() as usize;
                        app.hwnd = hwnd;
                        #[cfg(windows)]
                        PANEL_HWND.store(hwnd, std::sync::atomic::Ordering::Relaxed);
                        dwm_round_corners(hwnd as *mut std::ffi::c_void);
                    }
                }
            }
            // Tray watcher threads: they must outlive update(),
            // which stops being called while the window is hidden.
            spawn_tray_watcher(cc.egui_ctx.clone(), app.tray_ids.clone());
            Ok(Box::new(app))
        }),
    )
}
