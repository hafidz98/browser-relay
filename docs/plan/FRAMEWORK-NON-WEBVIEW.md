# Framework survey: non-WebView desktop shell for browser-relay

Question (Hafidz, 2026-10-11): after judging the Chrome `--app` panel
impractical and migrating to Wails v3 (WebView2), check frameworks that
do NOT use a webview — other languages (Rust, Dart) are acceptable.

Constraint that decides most of this: the panel must be buildable
FROM THIS VM (Linux, cross-compile to a Windows exe), because that is
the established delivery workflow. A framework that can only be built
on a Windows machine with Visual Studio breaks the workflow, however
good the UI is.

Relay core (CDP, WebSocket, token auth, Chrome management) is Go and
already tested (M0/M1 15/15, Wails v0.6.0). Any non-Go shell either
(a) rewrites the core, or (b) spawns the Go relay as a sidecar and
talks to it over localhost — the panel itself is only ~200 lines.

## Candidates

### Rust — egui (eframe)
- Rendering: own GPU renderer (wgpu/glow), no webview. MIT/Apache.
- Exe size: ~10–20 MB single file. UI is immediate-mode code, can
  match the Obsidian-like panel styling closely enough.
- Tray: not built in; `tray-icon` crate.
- Cross-build from Linux: target `x86_64-pc-windows-gnu` is installed,
  but the build needs a mingw-w64 dlltool/gcc — wiped by VM resets.
  Workaround being spiked: llvm-dlltool + rust-lld shims from the
  portable clang/lld toolchain in `~/workspace/toolchain`.
- Cost if chosen: core rewrite in Rust OR sidecar architecture.

### Rust — iced
- Similar rendering story to egui, Elm-style architecture.
- Still pre-1.0 (API churn), more boilerplate. No advantage over egui
  for a 200-line panel.

### Rust — Slint
- Best declarative tooling of the Rust options.
- License is the blocker: GPLv3 or royalty-free-with-attribution or
  paid. browser-relay may go open source (Jivix ecosystem is OSS);
  GPL on the shell is a real decision, not a detail.

### Dart — Flutter desktop
- Rendering: Skia/Impeller, no webview. Best-looking UI of the list,
  tray via packages.
- DISQUALIFIED on the build constraint: Windows desktop builds
  require a Windows host with Visual Studio; cannot be produced from
  this VM at all. Output is a folder (exe + DLLs), not a single file.

### Go — Fyne
- Rendering: own OpenGL canvas, no webview. Tray built in.
- Only option with ZERO core rewrite: same language, same module,
  shell swaps in where Wails is now. Single exe ~25–30 MB.
- Cross-build from this VM uses the same mingw toolchain as the
  current Wails build (CGO) — no new toolchain risk beyond what
  v0.6.0 already needs.
- UI polish is more utilitarian than Flutter/egui-custom, but the
  panel is a status + buttons screen, well within Fyne's range.

### Also considered
- Avalonia (.NET/C#): Skia rendering, no webview, mature. But adds a
  whole .NET toolchain + core rewrite/sidecar; no win over Fyne/egui.
- Qt (C++/PySide): native widgets, heavy toolchain, licensing care
  (LGPL obligations), oversized for this panel.

## Verdict

- Lowest risk, no rewrite: **Fyne** — Go core untouched, tray
  built in, buildable from the VM with the existing mingw setup.
- If a move to Rust is wanted for its own sake: **egui**, with the
  relay kept as a Go sidecar (do NOT rewrite the tested core for a
  200-line panel). Spike result decides whether the VM can build it.
- Flutter is out (cannot build here). Slint is out unless its
  license is accepted deliberately. Iced offers nothing over egui.

## Spike log

- 2026-10-11: egui hello-panel (`~/workspace/eguispike`, eframe 0.32,
  430x880 viewport, panel mock) — first build died on missing
  `x86_64-w64-mingw32-dlltool` (VM reset wiped mingw, same root cause
  as past toolchain losses).
- 2026-10-11: PROVEN — egui spike cross-compiled from this VM to a
  valid Windows PE GUI exe, 5.9 MB. Trick: symlink
  `x86_64-w64-mingw32-dlltool` -> `llvm-dlltool-18` from the portable
  toolchain (the windows-rs build scripts only need dlltool; apt was
  locked at the time). So the Rust/egui route is buildable here.
- Decision status: NOT yet taken by Hafidz. Fyne remains the
  zero-rewrite Go option; egui is proven-buildable but implies a
  sidecar or core rewrite. Hafidz's next move in the main chat was to
  accelerate the native Android client (M3) instead — mock UI done at
  `mock/android-client-mock.html`, awaiting his review.
