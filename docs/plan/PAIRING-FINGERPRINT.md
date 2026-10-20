# Plan: Single-Use Pairing Codes + Device Fingerprints

## Problem
Current model: one rotating 6-digit code, any client with the code can auth,
no device identity. If the code leaks (shoulder-surfed, screenshot shared),
anyone on the LAN can connect as a viewer/driver. No way to tell devices
apart, no revocation.

## Goal
1. **Pair code = single-use per device.** A code can be consumed exactly once.
   After device A pairs with code X, code X is burned. Device B needs a fresh code.
2. **Device fingerprint contract.** Each device has a stable fingerprint (UUID).
   Pairing creates a contract: fingerprint → {name, paired_at, last_seen}.
   Reconnects use the fingerprint, no code needed. The relay can list and
   revoke paired devices.

## Design

### Fingerprint
- Client-generated UUID v4 on first run, stored durably:
  - Android: SharedPreferences `br_fingerprint`
  - Web: localStorage `br_fingerprint`
- Sent in every `auth` message as `fingerprint`.

### Pairing code lifecycle (relay)
- Codes are 6 digits, generated on demand.
- Each code: `{code, created_at, used: bool}`.
- `auth` with `token` + `fingerprint`:
  - Code valid (exists, unused, not expired — 5 min TTL) → burn it
    (`used=true`), register/update the paired-device contract, `welcome`.
  - Code missing/used/expired → `error: "bad token"`, close.
- `auth` with only `fingerprint`:
  - Fingerprint in paired devices → `welcome` (reconnect, no code).
  - Unknown → `error: "unknown device"`, close.
- Backward compat: valid code + no fingerprint → relay generates a
  fingerprint, returns it as `welcome.fingerprint`; client should store it.
  (Covers old clients during transition.)

### Paired-device contract (persisted)
Stored in `%APPDATA%\browser-relay\paired.json`:
```json
{
  "devices": {
    "<uuid>": {"name": "Redmi Note 10", "paired_at": 123, "last_seen": 456}
  }
}
```
- `last_seen` updated on every auth.
- Panel settings lists devices: name, short fingerprint, paired date,
  last seen, current role (driver/viewer/offline), Revoke button.
- Revoke removes the fingerprint; that device must re-pair with a fresh code.
  If it is currently connected, it is disconnected.

### Driver election (unchanged semantics, better visibility)
- First connected paired device = driver; others = viewers.
- Driver disconnect → earliest-connected viewer promoted.
- `welcome` and `role` messages unchanged; client list now includes
  device name + fingerprint short.

### QR / pair page
- QR always encodes a fresh UNUSED code (generate new after each burn).
- Pair page copy: "single-use code" note.

### Protocol summary
```json
// pairing (first time)
{"type": "auth", "token": "123456", "fingerprint": "uuid", "name": "Redmi Note 10"}
// reconnect
{"type": "auth", "fingerprint": "uuid"}
// welcome (paired=true when contract exists/created)
{"type": "welcome", "driver": true, "browserUp": true, "paired": true, "fingerprint": "uuid"}
// errors: "bad token" | "unknown device"
```

## Implementation steps
1. Relay (`hub.rs`): code store with used-flag + TTL; paired-device store
   with JSON persistence; auth logic per above; `paired_devices()` for panel.
2. Panel (`main.rs` settings): paired-devices list + revoke.
3. Web client (`app.js`): fingerprint gen/store; send in auth; handle
   `welcome.fingerprint`; reconnect without code.
4. Android (`MainActivity`/`RelayClient`): same.
5. E2E: single-use enforcement (second use of code → bad token);
   reconnect via fingerprint; unknown fingerprint rejected; revoke works.

## Out of scope
- Per-device permissions beyond driver/viewer (all paired devices are
  driver-capable; one driver at a time).
- Encrypted transport (LAN trust model unchanged).
