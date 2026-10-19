# Migration plan: Wails shell -> egui (Rust) shell

Status: approved by Hafidz 2026-10-11 ("kita coba pake egui").
Supersedes MIGRASI-WAILS.md for the desktop shell. Target: **v0.7.0**.

## Why

The Wails panel (v0.6.0) works but depends on the WebView2 runtime being
present and healthy on the laptop. Hafidz asked to evaluate non-WebView
frameworks (Rust/Dart included). A spike proved egui (eframe 0.32) can be
cross-compiled from the Linux dev VM to a Windows GUI exe (14.2 MB,
PE32+ valid), so the Rust shell is buildable in this environment.
Flutter was rejected: Windows builds require a Windows machine + VS2022.

## Architecture: native shell + proven Go core (sidecar)

Rewriting the relay core (CDP screencast, hand-rolled RFC6455 WebSocket,
hub, Win32 clipboard — ~2000 lines, E2E-tested) in Rust would put a
working product at risk for zero user-visible gain. Instead:

```
relay-0.7.0.exe  (Rust + egui, the only file the user downloads)
  |- embeds relay-core.exe (Go) as bytes; extracts it to
  |  %APPDATA%\browser-relay\relay-core.exe on first run / version change
  |- spawns the core hidden with `--service`, holding its stdin pipe
  |  open; if the shell dies, the pipe closes and the core exits
  |- panel UI (egui) mirrors the approved Wails panel 1:1
  |- tray icon (tray-icon crate): Open panel / Launch Chrome /
  |  Close Chrome / Quit; window close hides to tray
  `- Chrome management reimplemented in Rust (same paths, same args,
     same chrome.pid contract as chrome_mgmt_windows.go)

relay-core.exe  (Go, built without the `wails` build tag)
  |- `--service` mode: bind the LAN port first, serve the phone client,
  |  Watch + maintainBrowser loops, never fatal when Chrome is absent
  |  (mirrors desktop-mode resilience, minus the GUI)
  `- writes status.json in the config dir every second:
     {connected, clients, phoneUrl, code, version}
```

Shell <-> core contract is deliberately tiny: one child process, one
stdin watchdog pipe, one status file. Launch/Close Chrome and the
headless preference stay shell-side (they are local file/process ops,
exactly what PanelService did from Go).

## Go core changes (minimal, additive)

1. `--service` flag in main.go: serve + maintain, no fatal Probe flow.
2. status.json writer (1s tick, atomic rename) in service mode.
3. `app_windows.go` moves behind `//go:build windows && wails`;
   a `windows && !wails` stub keeps `runDesktop` linked for core builds.
   The Wails shell stays in-tree and buildable with `-tags wails`.
4. `appVersion` -> "0.7.0". Core stays CGO-free; console mode untouched.

## Rust shell (crate `shell/`)

- eframe 0.32, `#![windows_subsystem = "windows"]`, window 430x880
  (min 360x560), background #161616, Obsidian-style styling.
- Panel: header + version; status card (dot + title + sub); Browser card
  (Launch Chrome primary, Close, headless checkbox); Connection card
  (phone URL + Copy, pairing code big + Copy, connected phones count);
  hint footer; transient toast for action results.
- Status: poll status.json every 1s. Missing/stale (>3s) or child
  exited -> "Relay core is not running" state + Restart button.
- Chrome mgmt: find chrome.exe (ProgramFiles / ProgramFiles(x86) /
  LOCALAPPDATA); refuse double-launch when CDP /json/version answers;
  launch args identical to Go (dedicated %USERPROFILE%\chrome-remote
  profile, debug port 9222, --remote-allow-origins=*, unthrottle flags,
  --headless=new when pref on, opens chrome://newtab/); write PID to
  chrome.pid; Close = read PID, `taskkill /PID <pid> /F`, remove file.
- Single instance: named mutex; second launch writes a `show-panel`
  sentinel in the config dir; the running instance polls it and shows
  the window (mirrors Wails OnSecondInstanceLaunch).
- Config dir: %APPDATA%\browser-relay (token.txt shared with the core —
  the core remains the token's owner; the shell reads the code from
  status.json, not the file).

## Build

`build-windows.sh` (repo root):
1. Go core: `GOOS=windows GOARCH=amd64 CGO_ENABLED=0 go build` (no tags)
   -> `relay-core.exe` (repo root, gitignored).
2. Rust shell: `cargo build --release --target x86_64-pc-windows-gnu`
   with MinGW linker + the llvm-dlltool PATH shim (see spike notes)
   -> shipped as `relay-0.7.0.exe`.

## Risks / honest limits

- The egui panel cannot be visually verified in this VM (no Windows
  display); layout mirrors the already-approved Wails panel and the
  logic paths are compile-checked. First real visual check is on
  Hafidz's laptop, same as v0.6.0.
- egui redraw-on-poll: status/tray polling uses a 250 ms repaint timer;
  CPU cost is negligible for a panel.
- Two processes instead of one: mitigated by the stdin watchdog (core
  cannot outlive the shell) and by Quit killing the child explicitly.
- Antivirus may dislike extract-and-run; the core is extracted once to
  the app's own config dir with a version marker, not on every start.

## Out of scope

- Any change to the phone client, the WS protocol, or CDP behavior.
- Rewriting the core in Rust (revisit only if the sidecar hurts).
- Removing the Wails shell from the tree (kept behind its build tag).
