# browser-relay — Plan v0.1

Drive the laptop's Chrome from the phone (and a Windows client) over the
local network / Tailscale. Login sessions stay on the laptop; the phone
only receives a video stream of the browser viewport and sends back input
events. Local-first for low latency, no Google account, no cloud relay.

Status: PLAN (2026-10-10). Requested by Hafidz after asking whether a
user-controllable browser client app was feasible. Supersedes the
Chrome Remote Desktop idea (whole-desktop sharing, traffic via Google
servers, needs Google login on both ends).

**v0.2 (2026-10-10, same day)** — revised per Hafidz feedback: LAN primary
+ Tailscale optional, user-selectable Chrome path, tab management in scope
("sewajarnya browser"), audio + clipboard research findings, relay-vs-direct
rationale (verified), native-Android analysis.

Related: this is the standalone extraction of the Jivix live co-driving
design (PLAN.md section C2, `~/workspace/agent-harness-runtime/`):
CDP `Page.startScreencast` for the live view, `Input.dispatch*` for
takeover, one browser per session, driver-handover protocol.

## 1. Goals

- G1: Open the phone, see the laptop's Chrome live, click/type as if
      sitting at the laptop.
- G2: Sessions, cookies, passwords stay on the laptop. The phone never
      holds credentials; losing the phone leaks no logins.
- G3: Latency low enough for interactive use on LAN / Tailscale
      (target: input-to-photon < 150 ms on local Wi-Fi).
- G4: Zero-install friction on the laptop: a single `.exe`, no Python /
      Node / admin rights required. Chrome path auto-detected from the
      running Chrome process (exe path + profile from its command line);
      manual selection is only a fallback override (first-run config).
- G5: Android client as a real APK (WebView wrapper, proven build
      pattern in `~/workspace/apk-pelacak/build.sh`).
- G6: LAN-first networking. Phone and laptop talk directly over the local
      network; Tailscale is an opt-in path for reaching the laptop from
      outside the LAN, not the default.

## 2. Non-goals (v1)

- Audio streaming (CDP screencast is video-only; no tab audio on the phone).
- File download to the phone (downloads land on the laptop; v2 can add
  a file bridge).
- Multi-user / multi-browser sessions. One browser, one driver at a time.
- A Windows client, period. Dropped 2026-10-10 per Hafidz — Chrome runs on
  the PC itself, so driving it from the same PC is pointless. (If a second
  PC ever needs access, the C3 web client works from any browser as-is.)

## 3. Architecture

```
 Laptop (Windows)                              Phone (Android)
┌──────────────────────────────┐              ┌────────────────────────────┐
│ Chrome  ──CDP──► relay.exe   │  Tailscale   │ APK (WebView)              │
│  --remote-debugging-port=9222│◄────────────►│  web client (canvas +      │
│  (his Profile 9: logins stay │  ws://100.x  │   input capture)           │
│   on disk, untouched)        │  :8765       │                            │
└──────────────────────────────┘              └────────────────────────────┘
         ▲                                                  ▲
         │ localhost only                                   │ token auth
   Chrome DevTools Protocol                          JPEG frames ──►
                                                     input events ◄──
```

Components:

- **C1 — Chrome (existing install).** Launched once with
  `--remote-debugging-port=9222` added to the shortcut. The relay attaches
  to his real profile (logins, extensions, history all intact) instead of a
  fresh headless instance.
- **C2 — Relay server (`relay.exe`, Go, single binary).** Owns one CDP
  session: starts screencast, fans JPEG frames out to clients, translates
  client input events into `Input.dispatchMouseEvent` /
  `Input.dispatchKeyEvent`, manages tabs via the Target domain
  (list/activate/new/close), serves the web client over HTTP, enforces the
  auth token, binds to the LAN interface by default (Tailscale interface
  opt-in).

### Why a relay at all (and not direct CDP from the phone?)

Hafidz asked exactly this. Two decisive reasons, both verified:

1. **The debug port is not reachable from the phone.** Chrome binds
   `--remote-debugging-port` to `127.0.0.1` only — Chrome 136+ even
   ignores `--remote-debugging-address=0.0.0.0`. From the phone,
   `laptop:9222` simply doesn't exist. Something on the laptop must
   bridge localhost to the LAN. That bridge is the relay. (Forcing the
   port open via portproxy/socat is possible on older builds, which
   leads to reason 2.)
2. **The debug port has no auth and grants full control.** Anything that
   reaches it can `Runtime.evaluate` arbitrary JS in his logged-in tabs
   and read cookies via the Network domain. Exposing that to the LAN
   (or the tailnet) is unacceptable. The relay terminates the privileged
   CDP locally and exposes a least-privilege protocol instead: JPEG
   frames out, input events in, token-authenticated. No JS evaluation,
   no cookie access.

So the relay is a security + transport boundary, not boilerplate. A
"smart client speaking raw CDP" would still need the localhost bridge,
and would put full browser control one reachable port away.
- **C3 — Web client (HTML/JS, no build step).** Renders frames on
  `<canvas>`/`<img>`, captures pointer + keyboard, maps client coordinates
  to the browser viewport (scale + devicePixelRatio), sends input events.
  Works in any browser and inside the Android WebView.
- **C4 — Android APK.** Thin WebView wrapper around C3: fullscreen,
  keep-screen-on, connects to `ws://<laptop-tailnet-ip>:8765`.
- **C5 — Windows client: DROPPED** (2026-10-10, Hafidz: "kan Chrome-nya di
  PC, tinggal buka"). Fair — a client on the same machine as the browser is
  pointless. The C3 web client still works from any browser on the LAN if
  ever needed (e.g. a second PC), but nothing Windows-specific ships.

Why Go for the relay: cross-compile `GOOS=windows` from this VM,
single static `.exe`, no runtime to install on his laptop. CDP is just
JSON over WebSocket — no heavy dependency needed.

## 4. Protocol (relay <-> client, v1)

WebSocket, first message must be `{ "type": "auth", "token": "..." }`.
Then:

- Server -> client: `{ "type": "frame", "data": "<base64 jpeg>" }`. The server
  sends CDP's `screencastFrameAck` immediately; backpressure is handled by
  dropping stale frames for slow clients (latest-frame-wins, per-client
  channel). On every attach the server also pushes one
  `Page.captureScreenshot` still first, so the client never stares at a
  blank screen on static pages (screencast only emits on repaint).
- Client -> server: `{ "type": "mouse", ... }`, `{ "type": "key", ... }`,
  `{ "type": "wheel", ... }`, `{ "type": "viewport", "w":, "h":, "dpr": }`.
- Client -> server: `{ "type": "emulation", "mobile": true|false }` — toggles
  mobile emulation on the active tab (mobile UA + 390x844 viewport + touch
  emulation, via `Network.setUserAgentOverride` +
  `Emulation.setDeviceMetricsOverride`); auto-applied to new tabs while on.
- Server -> client: `{ "type": "info", "viewport": {...}, "url": "...", "title": "..." }`
  on connect and on navigation (cheap tab-awareness without full tab mgmt).

Screencast params: `format: "jpeg", quality: 60-80, maxWidth/maxHeight`
matched to the phone screen, `everyNthFrame: 1`. Tune quality vs.
bandwidth in M1; JPEG at phone resolution is typically 30-80 KB/frame.

## 5. Security model

- Token auth: relay generates a random token on first run, printed in its
  console and saved to `%APPDATA%\browser-relay\token.txt`. No token, no
  frames, no input. Token rotation = delete the file, restart.
- Bind to the laptop's LAN IPv4 by default, with the Tailscale `100.x.y.z`
  as an opt-in second listener — never `0.0.0.0`, never exposed to the
  internet. Token auth is mandatory on every listener. Plain `ws://` on the
  trusted LAN/tailnet is acceptable for v1 (`wss` optional later).
- One driver at a time: the first authenticated client holds the control
  lock; further clients are view-only until the driver disconnects or
  releases. Prevents two thumbs fighting over one cursor.
- The relay only speaks to Chrome on `127.0.0.1:9222`; the debug port is
  never bound publicly (a remote debugger = full browser control).

Threats accepted in v1 (documented, not fixed): anyone with the token AND
tailnet access can drive the browser — same as sitting at the keyboard.
Token leak = rotate the token. No clipboard sync in v1 (avoids password
exfiltration path); revisit deliberately.

## 6. Gotchas & constraints

1. **Chrome must be restarted once with the flag.** CDP cannot attach to an
   already-running Chrome that wasn't launched with
   `--remote-debugging-port`. One-time setup: Hafidz edits his Chrome
   shortcut (append the flag after `chrome.exe`) and restarts Chrome.
   After that, the relay auto-detects the running Chrome (exe path +
   user-data-dir from the process command line — manual path selection is
   only a fallback override) and attaches to it. The relay never launches
   Chrome itself in the normal flow; if the debug port isn't reachable, it
   says so instead of guessing. (Tab strip is M2 scope, see §8 Q3.)
2. **No audio.** CDP screencast carries no audio. Video-only pages are
   fine; YouTube plays mute on the phone.
3. **IME / non-Latin input on Android.** `Input.dispatchKeyEvent` with raw
   key codes handles Latin well; CJK/compose input needs
   `Input.insertText`. Plan: send `insertText` for committed text from the
   Android soft keyboard; verify in M3.
4. **Coordinate mapping.** Phone CSS pixels -> browser viewport pixels via
   the `viewport` message (scale + DPR). Pinch-zoom is client-side only in
   v1 (zoom the rendered frame, recompute mapping) — simpler than driving
   browser zoom.
5. **Chrome updates** can change CDP, but `Page.startScreencast` /
   `Input.dispatch*` have been stable for years. M0 verifies against his
   installed Chrome version.
6. **Battery/data.** Continuous JPEG stream over Tailscale; fine on Wi-Fi,
   heavy on mobile data — add a quality toggle + pause button in the
   client (M3).

## 7. Milestones

- **M0 — Verify (VM + his laptop info).** Confirm CDP screencast flow
  against a local Chromium here; collect his Chrome version + confirm
  Tailscale runs on laptop and phone.
- **M1 — Relay + web client prototype.** Go relay: CDP attach, screencast
  fan-out, input dispatch, token auth, serves C3. Tested laptop-local
  (browser tab as the "phone").
- **M2 — Real profile + LAN.** Attach to his Profile 9 via the
  shortcut flag; bind LAN IP (Tailscale opt-in); text clipboard sync
  (Win32 API in the relay <-> client, see §12); tab strip like a normal
  browser (list/activate/new/close via Target domain); mobile-emulation
  toggle (mobile UA + viewport + touch, auto-applied to new tabs);
  end-to-end from the
  phone's browser. Latency measurement (input-to-photon, rough).
- **M3 — Android APK.** WebView wrapper, fullscreen, keep-awake,
  reconnect on network change, quality toggle, `insertText` path for the
  soft keyboard. Build with the existing `build.sh` pattern.
- **M4 — Polish + docs.** Reconnect/backoff, view-only vs driver mode UI,
  README (install: 1. edit Chrome shortcut, 2. run relay.exe, 3. install
  APK, 4. enter token), token rotation steps.
- **M5 — Hardening + research bets (optional).** `wss`, file bridge for
  downloads, idle auto-pause, native Android client evaluation (see §13),
  audio via companion extension (see §11).

## 8. Open questions for Hafidz

- Q1 (resolved 2026-10-10): Chrome path — auto-detect common install
  locations, user override via first-run config. Nothing needed from Hafidz.
- Q2 (resolved 2026-10-10): Tailscale is opt-in; LAN is primary. Nothing needed.
- Q3 (resolved 2026-10-10): yes — tab strip like a normal browser
  (list/activate/new/close) is in scope for M2.
- Q4 (resolved 2026-10-10): Windows client dropped entirely — Hafidz
  pointed out it's pointless since Chrome is on the PC itself ("kan
  Chrome-nya di PC, tinggal buka wkwk").

## 9. Deliverables

- `relay/` — Go source, builds to `relay.exe` (`GOOS=windows go build`).
- `client/` — `index.html` + `app.js` (web client, also loaded by the APK).
- `android/` — WebView wrapper project + `build.sh` entry.
- `docs/plan/PLAN.md` (this file), dev logs under `dev-logs/`.
- README with the 4-step install.

## 11. Research: audio (2026-10-10)

CDP screencast carries no audio — verified, no workaround on the protocol
side. Two routes investigated:

**Route A (recommended): companion Chrome extension + `chrome.tabCapture`.**
`chrome.tabCapture` exposes a MediaStream containing the tab's audio
([docs](https://developer.chrome.com/docs/extensions/reference/api/tabCapture)).
MV3 pattern: the service worker calls
`chrome.tabCapture.getMediaStreamId({targetTabId})`, then an offscreen
document converts the stream ID into a real MediaStream via
`getUserMedia({audio: {mandatory: {chromeMediaSource: 'tab',
chromeMediaSourceId: id}}})`. The extension streams Opus audio (WebRTC or
plain WebSocket) to the phone client. Granularity is per-tab — the YouTube
tab plays, other tabs stay silent — which is the right behavior.
Costs and caveats:
- Extension installed unpacked (developer mode): one-time setup step.
- Capture requires a user gesture on the extension (action-button click);
  design: click once to "enable audio", stays on afterwards.
- Capturing a tab's audio mutes it on the laptop speakers unless re-routed
  through an AudioContext to the destination (documented pattern) — a UX
  decision, not a blocker.
- The hard part is A/V sync between CDP JPEG frames and the Opus stream:
  needs a shared clock / jitter-buffer design. This is the research risk.
- Extension <-> relay coordination: the extension talks to the relay over
  a local WebSocket; the relay forwards audio packets to the authenticated
  phone client.

**Route B (rejected): OS-level loopback (WASAPI) inside the relay.**
Captures *system* audio, not per-tab; needs cgo or heavy syscall code in
Go. Worse on every axis.

Verdict: audio is feasible via Route A, scoped as M5. Not v1 — the A/V
sync design needs its own pass.

## 12. Research: clipboard (2026-10-10)

Text clipboard sync laptop <-> phone is straightforward, no extension
needed: the Go relay reads/writes the Windows clipboard via the Win32 API
(`OpenClipboard` / `GetClipboardData` / `SetClipboardData`) through plain
syscalls — no cgo. Protocol: `{type:'clipboard', action:'get'|'set', text}`
over the existing authenticated WebSocket; the Android client mirrors to
its own clipboard manager. v1 = plain text only (images/files later).
Security note: clipboards often hold passwords/OTPs. Mitigation is the
existing token auth + LAN/tailnet-only binding, plus explicit UX: the
relay only touches the Windows clipboard when the client asks (no silent
background polling). Scoped for M2.

## 13. Android: WebView wrapper vs native (2026-10-10)

Hafidz asked whether the Android client can be native. Yes.

- **WebView wrapper (plan default, M3):** the C3 web client runs inside a
  WebView; APK built with the proven manual toolchain (javac/d8/aapt2).
  Fastest to ship, least code. Cost: WebView rendering + input latency
  overhead, less control over touch gestures and the soft keyboard.
- **Native (Java, no Kotlin needed):** custom View rendering JPEG frames
  as Bitmaps on a SurfaceView, direct touch/key handling, a minimal
  WebSocket client in pure Java (~100 lines, no extra dependencies).
  Better frame pacing and input fidelity; roughly 2-3x the work of the
  wrapper, and the manual build gets more complex (still Gradle-free for a
  single-activity app, but no Kotlin stdlib unless bundled).

Verdict: ship the WebView wrapper first (M3), measure input-to-photon
latency and feel; go native only if it disappoints. Both speak the same
relay protocol, so switching clients costs nothing server-side.

8. **Mobile emulation changes the laptop view too.** Emulation is applied
   to the shared tab, so while mobile mode is on, his laptop's Chrome
   window also renders the mobile layout. Toggle off to restore desktop
   rendering — one tab, one rendering.

## 10. Estimates

Rough, honest: M0–M2 ~1 focused session (protocol + relay + local test),
M3 ~1 session (APK + input quirks), M4 docs ~half session. The unknowns
are all on his laptop (Chrome flag, Tailscale reachability) — the code
itself is a few hundred lines of Go + ~200 lines of JS.
