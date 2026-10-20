# Dev log 2026-10-11 — egui migration (v0.7.0)

## Decisions (Hafidz)

- Android client mock UI: **approved** after 3 revisions (edge handle,
  two-tap open, tap-screen dismisses, grid icon, consistent 50px size).
  Design locked; native APK build comes later.
- Desktop shell: **try egui** ("sekarang coba fokus selesain kita coba
  pake egui"). Wails v0.6.0 remains the shipped fallback.

## Framework evaluation (done earlier today)

- egui spike cross-compiled from this VM: 14.2 MB PE32+ GUI exe, valid.
  (An earlier 5.9 MB figure was reported in error and corrected openly;
  the exe did not exist at that time — the final link had failed after
  a VM replacement removed MinGW. Full dep tree compiled; dlltool need
  solved with an llvm-dlltool symlink shim.)
- Flutter rejected: no Windows build possible from this VM.
- Fyne noted as the no-rewrite Go option; not chosen.

## Plan

docs/plan/MIGRASI-EGUI.md — egui shell + embedded Go core sidecar
(--service mode + status.json contract), panel mirrored 1:1 from the
approved Wails layout, Wails code kept behind the `wails` build tag.

## Progress

- [x] Plan written; Go sources re-read for exact contracts
      (PanelService.Status fields, chrome launch args, PID file).
- [x] Go core: --service + status.json + build tags + v0.7.0.
      Smoke test on Linux PASS: binds immediately, lazy CDP attach,
      status.json = {connected:true, clients:0, phoneUrl, code,
      version:"0.7.0"}, phone page HTTP 200, stdin watchdog exits the
      core exactly when the pipe closes. Windows core exe 9.6 MB;
      `-tags wails` build still compiles (old shell preserved).
- [x] Rust shell crate: panel UI mirroring the Wails layout, tray
      (tray-icon), single-instance (loopback lock + show-panel
      sentinel), core manager (extract embedded Go core, spawn
      --service hidden, stdin watchdog, status.json polling, Restart
      on Down), Chrome mgmt in Rust (same paths/args/PID contract,
      taskkill for Close). cargo check PASS after two egui 0.32 API
      fixes (close_requested() is a method; CornerRadius is u8).
- [x] Integrated build + verification + delivery. NOTE: the dev VM
      was replaced mid-build; finished artifacts vanished between
      instances twice. Final verified artifact on the delivery
      instance: relay-0.7.0.exe = 24,624,849 bytes, PE32+ (GUI)
      x86-64, embedded-core string check PASS. Git tag v0.7.0.
      First visual/functional run of the egui panel happens on
      Hafidz's laptop (no Windows display in this VM) — the Go core's
      service mode is fully smoke-tested, the shell logic is
      compile-verified only.
