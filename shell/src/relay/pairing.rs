//! Single-use pairing codes + device fingerprint contracts.
//!
//! Security model:
//! - The relay holds ONE current pairing code (6 digits). It is single-use:
//!   the moment a device pairs with it, the code is burned and a fresh one
//!   is generated. A code also expires after CODE_TTL.
//! - A 6-digit code is a small secret (900k values), so guessing it is
//!   capped: after MAX_WRONG_ATTEMPTS wrong guesses the code is invalidated
//!   and a fresh one is generated, so one code never takes more hits than
//!   that. The panel log says when that happens.
//! - Each device has a stable fingerprint (client-generated UUID). Pairing
//!   creates a contract: fingerprint -> {name, paired_at, last_seen},
//! persisted to paired.json. Reconnects present only the fingerprint.
//!   A paired device keeps working no matter how many codes were burned:
//!   its fingerprint is its credential, the code is only for the first
//!   pairing.
//! - The panel can list and revoke paired devices; revoking a connected
//!   device drops it (it must re-pair with a fresh code).

use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::rlog;

/// How long an unused pairing code stays valid.
const CODE_TTL_SECS: u64 = 300;

/// Cooldown between pairing requests from the same IP.
const REQUEST_COOLDOWN_SECS: u64 = 10;

/// Wrong guesses allowed against one pairing code. The next guess after
/// this many burns the code: it is invalidated and a new one is
/// generated, so the code on the panel never takes more hits than this.
const MAX_WRONG_ATTEMPTS: u32 = 5;

fn gen_code() -> String {
    format!("{}", rand::rng().random_range(100_000..=999_999))
}

/// A fresh code that is guaranteed to differ from `old`. A burned
/// code must never come back: the panel would show the same digits
/// again and the user would think nothing happened.
fn gen_code_except(old: &str) -> String {
    let mut c = gen_code();
    for _ in 0..32 {
        if c != old {
            break;
        }
        c = gen_code();
    }
    c
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Put a fresh code on the panel. The new code is guaranteed to differ
/// from the old one, and the wrong-guess counter starts over: the
/// budget of MAX_WRONG_ATTEMPTS belongs to ONE code.
fn rotate_code(inner: &mut Inner) {
    inner.code = gen_code_except(&inner.code);
    inner.code_born = Instant::now();
    inner.wrong_attempts = 0;
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
    /// Wrong guesses against the current pairing code. At
    /// MAX_WRONG_ATTEMPTS the code is invalidated and a fresh one is
    /// generated, so a single code never takes more hits than that.
    wrong_attempts: u32,
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
                wrong_attempts: 0,
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
            if now_unix().saturating_sub(p.requested_at) <= CODE_TTL_SECS {
                return p.code.clone();
            }
        }
        if inner.code_born.elapsed().as_secs() > CODE_TTL_SECS {
            rotate_code(&mut inner);
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
            let age = now_unix().saturating_sub(p.requested_at);
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
    ///
    /// A 6-digit code is only 900k values, so guessing it is capped:
    /// after MAX_WRONG_ATTEMPTS wrong guesses the code is invalidated
    /// and a fresh one is generated, so one code never takes more hits
    /// than that.
    ///
    /// A device that is already paired keeps working: from then on its
    /// fingerprint is the credential, so a stale token neither counts
    /// as a guess nor burns the code the panel shows.
    pub fn try_pair(
        &self,
        code: &str,
        fingerprint: &str,
        name: &str,
    ) -> Result<(), &'static str> {
        self.try_pair_at(code, fingerprint, name, now_unix())
    }

    /// try_pair with an explicit clock: the windows are time-based, so
    /// the tests drive the clock instead of the wall clock.
    fn try_pair_at(
        &self,
        code: &str,
        fingerprint: &str,
        name: &str,
        now: u64,
    ) -> Result<(), &'static str> {
        if fingerprint.is_empty() {
            return Err("fingerprint required");
        }
        let mut inner = self.inner.lock().unwrap();
        self.refresh_code(&mut inner);
        // Match against the panel QR code…
        let mut matched = code == inner.code;
        // …or a pending client-initiated request.
        if !matched {
            if let Some(p) = &inner.pending {
                if code == p.code && now.saturating_sub(p.requested_at) <= CODE_TTL_SECS {
                    matched = true;
                }
            }
        }
        // A paired device is trusted: from then on its fingerprint is
        // the credential, so a stale token is not a guess — it must not
        // count against the cap and must not burn the code the panel
        // shows either.
        let paired = inner.devices.contains_key(fingerprint);
        if !matched && !paired {
            // Wrong guess: it counts against the current code.
            inner.wrong_attempts += 1;
            let attempts = inner.wrong_attempts;
            if attempts >= MAX_WRONG_ATTEMPTS {
                if self.fixed_code.is_some() {
                    // The --token test override is pinned, so it cannot
                    // be invalidated - only the counter moves.
                    rlog!("pairing refused: wrong code ({attempts}/{MAX_WRONG_ATTEMPTS}) [fixed code]");
                } else {
                    // Too many hits on this code: invalidate it and put
                    // a fresh one on the panel. The attacker gets
                    // MAX_WRONG_ATTEMPTS tries per code, not endless ones.
                    rlog!("pairing code burned after {attempts} wrong attempts");
                }
                self.burn_codes(&mut inner);
            } else {
                rlog!("pairing refused: wrong code ({attempts}/{MAX_WRONG_ATTEMPTS})");
            }
            return Err("bad token");
        }
        // Burn: single-use (unless a fixed test code). Fresh panel code;
        // clear the pending request.
        if matched {
            self.burn_codes(&mut inner);
            inner.pending = None;
        }
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

    /// Replace the code(s) a client can pair with. A fixed code (the
    /// --token test override) is never replaced: it is pinned on
    /// purpose, so only the guess counter moves. The pending
    /// request-code is always replaced - it is a separate guessable
    /// secret, whatever the panel code is.
    fn burn_codes(&self, inner: &mut Inner) {
        if self.fixed_code.is_some() {
            inner.wrong_attempts = 0;
        } else {
            rotate_code(inner);
        }
        if let Some(p) = &mut inner.pending {
            p.code = gen_code_except(&p.code);
        }
    }

    /// Replace the panel code when it expired. A fixed code (the
    /// --token test override) is never replaced.
    fn refresh_code(&self, inner: &mut Inner) {
        if inner.code_born.elapsed().as_secs() <= CODE_TTL_SECS {
            return;
        }
        if self.fixed_code.is_some() {
            inner.code_born = Instant::now();
            return;
        }
        rotate_code(inner);
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
