# Migration plan: Wails shell (v0.6.0)

Decision (Hafidz, 2026-10-11): the Chrome `--app` panel window "nggk
praktikal" — move the desktop shell to **Wails v3**.

## What changes
- Desktop shell = Wails v3 (beta.28, pinned): real WebView2 window
  (430×880) + built-in system tray + single-instance. No Chrome needed
  to show the panel; panel HTML/CSS/JS is reused as-is.
- Panel JS talks to Go via Wails service bindings
  (`Call.ByName("main.PanelService.*")`) instead of the HTTP API.
  `/panel` + `/api/*` HTTP endpoints are removed.
- Relay core (hub/ws/cdp/browser) unchanged; the LAN HTTP server keeps
  only the phone surface (`/`, `/app.js`, `/ws`).
- `--nogui` console mode unchanged (same binary, chosen by flag).
- Old hand-rolled Win32 tray (`gui_windows.go`) and the `--app` window
  launcher are deleted. Fatal-startup MessageBox stays (few syscalls).

## Build
- Now needs CGO: `GOOS=windows CGO_ENABLED=1 CC=x86_64-w64-mingw32-gcc
  go build -tags wails`. GUI code lives behind the `wails` build tag;
  a build without the tag falls back to console mode.
- Target needs the WebView2 runtime (preinstalled on Win10 1803+/Win11
  via Edge). Exe ~16MB (was 9.8MB pure Go).

## Spike result (2026-10-11)
Hello-world Wails v3 app cross-compiled from this VM to a Windows exe
successfully (MinGW 13.2, wails v3.0.0-beta.28, Go 1.25 toolchain).

## Risks
- v3 is beta: pin the exact version; keep the shell thin so an upgrade
  or a retreat to the pure-Go build stays cheap.
- WebView2 rendering quirks are inherited from the OS runtime.
