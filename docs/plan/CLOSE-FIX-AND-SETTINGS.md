# Research: slow Chrome close + console flash + restore prompt persists (v0.8.11)

## Symptoms (Hafidz, Win10, v0.8.11)
1. Close Chrome is slow (feels like ~10 s+ hang).
2. A cmd/terminal window flashes during close.
3. The "Pulihkan halaman?" (restore pages) prompt still appears on relaunch.

## Root-cause analysis

### Why slow
`close_chrome()` (chrome.rs) tries graceful close via CDP `Browser.close`
first. The graceful path has a worst case of ~13 s:
- `Browser.close` call wrapped in a 5 s timeout (CDP `call` itself has a
  15 s inner timeout),
- then an 80 × 100 ms = 8 s poll loop waiting for the DevTools port to go quiet.

When the graceful path FAILS, we pay the full ~13 s and then fall back to
`taskkill /F` anyway. The panel calls this on a background thread (no GUI
freeze), but the user still waits ~13 s doing nothing.

### Why the terminal flashes
`tasklist` (PID check) and `taskkill` are console-subsystem executables.
`std::process::Command` spawns them without `CREATE_NO_WINDOW`, so Windows
creates a visible console window for each invocation. v0.8.11 can spawn up
to two of them (tasklist + taskkill) after the long graceful wait, making
the flash very noticeable. (v0.8.10 and earlier also flashed once via
`taskkill /F`, but it was a single quick call.)

### Why the restore prompt persists
The prompt persists ⟺ the graceful path failed ⟺ we fell back to
`taskkill /F`. So the real question is why CDP `Browser.close` fails on
Hafidz's machine. Candidates (cannot verify without a Windows box):
- The hand-rolled CDP chain (raw HTTP GET /json/version → WS dial →
  Browser.close → port poll) has several Windows-specific failure points;
- `Browser.close` may hang if Chrome shows any modal dialog;
- the 5 s call timeout + 8 s poll may simply be mistuned.

Rather than debugging the CDP chain remotely, the fix uses the
Windows-native graceful close, which has no CDP dependency at all.

## Fix plan (v0.8.12)

`taskkill /PID {pid}` WITHOUT `/F` sends `WM_CLOSE` to the process's
top-level windows — the exact equivalent of clicking the window X.
Chrome then runs its normal shutdown and saves the session.

New `close_chrome()`:
1. `taskkill /PID {pid}` (no `/F`), with `CREATE_NO_WINDOW` (0x08000000).
   For headless Chrome (no windows) this errors immediately — fine.
2. Poll (200 ms × up to 4 s) for the PID to disappear.
3. If still alive: CDP `Browser.close` (existing `graceful_browser_close`,
   timeouts tightened: 3 s call + 3 s poll) — covers headless.
4. If still alive AND `pid_is_chrome(pid)`: `taskkill /F` with
   `CREATE_NO_WINDOW`.
5. All `Command` spawns of console programs (`taskkill`, `tasklist`)
   get `.creation_flags(0x08000000)` via `std::os::windows::process::CommandExt`
   (gated `#[cfg(target_os = "windows")]`).

Expected behavior: headed close in 1–2 s, no console flash, no restore prompt.

## Settings UI plan (panel)

New "Settings" view in the egui panel (gear button in the title bar or
a tab), persisted to `%APPDATA%\browser-relay\config.json`:

| Setting | Default | Notes |
|---|---|---|
| Chrome path | auto-detect | text field + "Detect" button; used by `launch_chrome()` |
| Theme | Dark | Dark / Light (egui Visuals) |
| Chrome DevTools port | 9222 | used for `--remote-debugging-port` and CDP base URL; changing it requires relaunching Chrome |
| Relay listen address | 127.0.0.1 | text field |
| Relay listen port | 18765 | number field; changing it requires relay restart (note in UI) |
| Connected phones | — | row showing N connected; click → popup listing each client: IP, driver/viewer role, connected-at, auth method |

Implementation notes:
- Config struct with serde; load on start, save on change.
- The hub already tracks clients; share an `Arc<Mutex<Vec<ClientInfo>>>`
  between hub and panel (same process). ClientInfo: id, peer IP,
  is_driver, connected_at, token label (e.g. "QR pairing" vs manual).
- Theme: `egui::Visuals::dark()` / `light()`; accent stays purple.
- Port changes: show "restart relay to apply" hint; Chrome port change
  takes effect on next Chrome launch.
