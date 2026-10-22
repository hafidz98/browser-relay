//! Console-only relay binary (no GUI): the E2E test surface on
//! Linux and a headless fallback. The product binary is
//! browser-relay-shell (src/main.rs); this one mounts the same
//! chrome + relay modules without eframe/tray-icon so it builds
//! and runs anywhere.

#[path = "../chrome.rs"]
pub mod chrome;
#[path = "../config.rs"]
pub mod config;
#[path = "../relay/mod.rs"]
pub mod relay;

pub const VERSION: &str = "0.9.63";

use relay::RelayHandle;

fn main() {
    config::init();
    relay::init_log();
    let (_nogui, opts) = relay::parse_args();
    let rt = std::sync::Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime"),
    );
    match RelayHandle::start(rt.clone(), opts) {
        Ok(h) => {
            println!("browser-relay v{VERSION} (rust, nogui)");
            println!("  phone URL:    {}", h.phone_url());
            println!("  pairing code: {}", h.code());
            rt.block_on(async {
                let _ = tokio::signal::ctrl_c().await;
            });
        }
        Err(e) => {
            eprintln!("fatal: {e}");
            std::process::exit(1);
        }
    }
}
