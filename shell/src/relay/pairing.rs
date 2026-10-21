//! Single-use pairing codes + device fingerprint contracts.
//!
//! Security model:
//! - The relay holds ONE current pairing code (6 digits). It is single-use:
//!   the moment a device pairs with it, the code is burned and a fresh one
//!   is generated. A code also expires after CODE_TTL.
//! - Each device has a stable fingerprint (client-generated UUID). Pairing
//!   creates a contract: fingerprint -> {name, paired_at, last_seen},
//!   persisted to paired.json. Reconnects present only the fingerprint.
//! - The panel can list and revoke paired devices; revoking a connected
//!   device drops it (it must re-pair with a fresh code).

use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// How long an unused pairing code stays valid.
const CODE_TTL_SECS: u64 = 300;

/// Cooldown between pairing requests from the same IP.
const REQUEST_COOLDOWN_SECS: u64 = 10;

fn gen_code() -> String {
    format!("{}", rand::rng().random_range(100_000..=999_999))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PairedDevice {
    pub name: String,
    pub paired_at: u64,
    pub last_seen: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct PairedFile {
    devices: HashMap<String, PairedDevice>,
}

/// A pending client-initiated pairing request (code shown on the panel).
#[derive(Clone)]
pub struct PairRequest {
    pub fingerprint: String,
    pub name: String,
    pub ip: String,
    pub code: String,
    pub requested_at: u64,
}

struct Inner {
    code: String,
    code_born: Instant,
    devices: HashMap<String, PairedDevice>,
    /// Active client-initiated request (None when no one is waiting).
    pending: Option<PairRequest>,
    /// Last request time per IP, for rate limiting.
    last_request: HashMap<String, u64>,
}

pub struct PairingManager {
    inner: Mutex<Inner>,
    file: std::path::PathBuf,
    /// Test override (--token flag): fixed code, never burned.
    fixed_code: Option<String>,
}

impl PairingManager {
    pub fn new(config_dir: &std::path::Path, fixed_code: Option<String>) -> Self {
        let file = config_dir.join("paired.json");
        let devices = fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str::<PairedFile>(&s).ok())
            .map(|f| f.devices)
            .unwrap_or_default();
        let code = fixed_code.clone().unwrap_or_else(gen_code);
        Self {
            inner: Mutex::new(Inner {
                code,
                code_born: Instant::now(),
                devices,
                pending: None,
                last_request: HashMap::new(),
            }),
            file,
            fixed_code,
        }
    }

    /// The current (unused) pairing code shown on the panel / QR.
    pub fn code(&self) -> String {
        if let Some(fixed) = &self.fixed_code {
            return fixed.clone();
        }
        let mut inner = self.inner.lock().unwrap();
        // If there's a live pending request, show ITS code — that's what
        // the requester must type. Otherwise show the rotating QR code.
        if let Some(p) = &inner.pending {
            if now_unix() - p.requested_at <= CODE_TTL_SECS {
                return p.code.clone();
            }
        }
        if inner.code_born.elapsed().as_secs() > CODE_TTL_SECS {
            inner.code = gen_code();
            inner.code_born = Instant::now();
        }
        inner.code.clone()
    }

    /// Seconds left before the current pairing code expires.
    /// The panel shows it so the code does not silently rotate under
    /// the user's eyes (it used to change every 5 minutes with no
    /// indication at all).
    pub fn code_ttl_remaining(&self) -> u64 {
        let inner = self.inner.lock().unwrap();
        if let Some(p) = &inner.pending {
            let age = now_unix() - p.requested_at;
            if age <= CODE_TTL_SECS {
                return CODE_TTL_SECS - age;
            }
        }
        let age = inner.code_born.elapsed().as_secs();
        CODE_TTL_SECS.saturating_sub(age)
    }

    /// Client-initiated pairing request: generates a single-use code and
    /// stages it for display on the panel. Rate-limited per IP; only one
    /// pending request at a time (a new one replaces the old).
    /// Returns the code, or an error string for the client.
    pub fn request_code(
        &self,
        fingerprint: &str,
        name: &str,
        ip: &str,
    ) -> Result<String, &'static str> {
        if fingerprint.is_empty() {
            return Err("fingerprint required");
        }
        // Already paired? No need for a code — just tell them to connect.
        if self.is_paired(fingerprint) {
            return Err("already paired");
        }
        let now = now_unix();
        let mut inner = self.inner.lock().unwrap();
        // If this fingerprint already has a live pending request, return
        // the same code instead of hitting the cooldown.
        if let Some(p) = &inner.pending {
            if p.fingerprint == fingerprint && now - p.requested_at <= CODE_TTL_SECS {
                return Ok(p.code.clone());
            }
        }
        if let Some(last) = inner.last_request.get(ip) {
            if now - last < REQUEST_COOLDOWN_SECS {
                return Err("too many requests");
            }
        }
        inner.last_request.insert(ip.to_string(), now);
        let code = gen_code();
        inner.pending = Some(PairRequest {
            fingerprint: fingerprint.to_string(),
            name: if name.is_empty() {
                "Unknown device".to_string()
            } else {
                // Cap length: client-supplied, shown on the panel.
                name.chars().take(40).collect()
            },
            ip: ip.to_string(),
            code: code.clone(),
            requested_at: now,
        });
        Ok(code)
    }

    /// The pending pairing request for the panel, if any and not expired.
    pub fn pending_request(&self) -> Option<PairRequest> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(p) = inner.pending.clone() {
            if now_unix() - p.requested_at > CODE_TTL_SECS {
                inner.pending = None;
                return None;
            }
            Some(p)
        } else {
            None
        }
    }

    /// Dismiss the pending request (panel Cancel button).
    pub fn cancel_request(&self) {
        self.inner.lock().unwrap().pending = None;
    }

    /// Attempt to pair with a code. On success the code is burned
    /// (single-use) and a fresh one is generated; the fingerprint
    /// contract is created/updated. Accepts both the panel QR code
    /// and a pending request-code.
    pub fn try_pair(
        &self,
        code: &str,
        fingerprint: &str,
        name: &str,
    ) -> Result<(), &'static str> {
        if fingerprint.is_empty() {
            return Err("fingerprint required");
        }
        let mut inner = self.inner.lock().unwrap();
        // Refresh the panel QR code when it expires.
        if inner.code_born.elapsed().as_secs() > CODE_TTL_SECS {
            inner.code = gen_code();
            inner.code_born = Instant::now();
        }
        // Match against the panel QR code…
        let mut matched = code == inner.code;
        // …or a pending client-initiated request.
        if !matched {
            if let Some(p) = &inner.pending {
                if code == p.code && now_unix() - p.requested_at <= CODE_TTL_SECS {
                    matched = true;
                }
            }
        }
        if !matched {
            return Err("bad token");
        }
        // Burn: single-use (unless a fixed test code). Fresh panel code;
        // clear the pending request.
        if self.fixed_code.is_none() {
            inner.code = gen_code();
            inner.code_born = Instant::now();
        }
        inner.pending = None;
        let now = now_unix();
        inner
            .devices
            .entry(fingerprint.to_string())
            .and_modify(|d| {
                d.last_seen = now;
                if !name.is_empty() {
                    d.name = name.chars().take(40).collect();
                }
            })
            .or_insert(PairedDevice {
                name: if name.is_empty() {
                    "Unknown device".to_string()
                } else {
                    name.chars().take(40).collect()
                },
                paired_at: now,
                last_seen: now,
            });
        drop(inner);
        self.save();
        Ok(())
    }

    /// Reconnect path: is this fingerprint a paired device?
    pub fn is_paired(&self, fingerprint: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if let Some(d) = inner.devices.get_mut(fingerprint) {
            d.last_seen = now_unix();
            drop(inner);
            self.save();
            true
        } else {
            false
        }
    }

    /// Remove a paired device. Returns true when it existed.
    pub fn revoke(&self, fingerprint: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let out = inner.devices.remove(fingerprint).is_some();
        drop(inner);
        if out {
            self.save();
        }
        out
    }

    pub fn list(&self) -> Vec<(String, PairedDevice)> {
        let inner = self.inner.lock().unwrap();
        let mut v: Vec<_> = inner
            .devices
            .iter()
            .map(|(k, d)| (k.clone(), d.clone()))
            .collect();
        v.sort_by_key(|(_, d)| d.paired_at);
        v
    }

    fn save(&self) {
        let inner = self.inner.lock().unwrap();
        let f = PairedFile {
            devices: inner.devices.clone(),
        };
        if let Ok(s) = serde_json::to_string_pretty(&f) {
            let _ = fs::write(&self.file, s);
        }
    }
}
