# browser-relay — native Android app (M3)

Date: 2026-10-11. Status: approved for implementation (Hafidz:
"kalo udah langsung riset dan implementasi app androidnya ya").

## 1. Goal

Native Android client for browser-relay (package
`com.hafidz.browserrelay`), replacing the web client on the phone.
Locked UI = `mock/android-client-mock.html` (ACC'd 2026-10-11:
edge handle + two-tap interaction).

## 2. Locked UI design (from the mock)

- **Fullscreen viewport**: the JPEG frame fills the screen, no
  chrome of its own (Chrome's tab strip is NOT shown — by design,
  CDP screencast is viewport-only).
- **Latency pill** top-right: green dot + "NN ms" (2s ping probe).
- **Edge handle (tools)**: thin pill docked to left/right edge,
  semi-transparent; drag to move (snaps to nearest edge); idle
  3.5s → fades to 38%; **tap 1** pops a round button out of the
  edge; **tap 2** opens the tools sheet; tap on the web content
  collapses it back; when the sheet is open the handle becomes
  the purple × button (bottom-right).
- **Tools sheet** (bottom sheet): URL bar + reload button; tab
  pills (horizontal scroll, active highlighted, × closes,
  + opens new tab); tool grid: Back, Forward, Keyboard, Paste,
  Desktop, Zoom, Find, Settings.
- **browserUp overlay**: when Chrome isn't running on the laptop,
  an overlay says so (driven by `welcome.browserUp` + `status`
  broadcasts); auto-hides when Chrome connects.
- Toast + first-run hint bubble.

## 3. Relay protocol (WebSocket `/ws`, JSON text frames)

Client → relay:
- `{type:"auth", token}` → `welcome` / `error`
- `{type:"ping", t}` → `pong{t}` (latency)
- `{type:"mouse", action:"down"|"move"|"up", x, y,
  button:"left"|"right"|"middle"}` — frame pixel coords
- `{type:"wheel", x, y, dx, dy}`
- `{type:"key", kind:"text", text}` / `{type:"key", kind:"code",
  vk, down}` (VK = Win virtual-key codes: 13 Enter, 8
  Backspace, 9 Tab, 27 Esc, 37-40 arrows, 46 Del, 36/35
  Home/End, 33/34 PgUp/PgDn)
- `{type:"tab", action:"activate"|"close", targetId}` /
  `{type:"tab", action:"new", url}`
- `{type:"emulation", mobile}` (desktop/mobile toggle)
- `{type:"clipboard", action:"get"}` / `{type:"clipboard",
  action:"set", text}`
- `{type:"history", action:"back"|"forward"}` (Back/Forward tools)
- `{type:"tab", action:"reload"}` (reload current tab)
- `{type:"find", text}` (find-in-page; server replies `find{count}`)
- `{type:"chrome", action:"launch"|"close"}` (remote Chrome
  lifecycle; server replies `chrome{action}`)

Relay → client:
- `welcome{driver, browserUp}`, `status{browserUp}`,
  `pong{t}`, `role{driver}`, `frame{data}` (base64 JPEG),
  `tabs{tabs:[{id,title,url,active}]}`, `clipboard{text}`,
  `emulation{mobile}`, `find{count}`, `chrome{action}`,
  `error{message}`

Roles: one driver at a time; viewers are read-only (input
only sent when `driver == true`).

## 4. Pairing

- Panel QR → `{base}/pair?c={code}` → page auto-fires
  `intent://pair?c={c}&host={base}#Intent;scheme=browserrelay;package=com.hafidz.browserrelay;S.browser_fallback_url={web};end`
  (the `&host=` param is a relay-side addition, v0.8.9 —
  carries the phone-visible base URL so QR pairing is
  zero-config).
- App intent-filter: `scheme="browserrelay"`, `host="pair"`.
  Parses `c` (token) + `host` → derives `ws://host/ws`.
- Fallback: manual entry (host:port + 6-digit token), saved in
  SharedPreferences with the last-used server.
- No in-app QR scanner in v1 (system camera → /pair → intent
  covers it; avoids camera + ML Kit deps).

## 5. Touch → input mapping (v1 decisions)

- Single tap → mouse down+up (left click) at frame coords.
- One-finger drag → `wheel` scroll (dx,dy from drag deltas) —
  the standard remote-browser pattern.
- Long-press → right click (down+up, button right).
- Keyboard tool → opens soft keyboard; committed text →
  `{type:"key",kind:"text"}` (matches web client's hidden-input
  path); Enter/Backspace via key codes.
- Pinch → reserved (Zoom tool cycles zoom levels in v1).

## 6. Architecture (no external deps)

- Java 8, plain Views (no Compose — manual javac build, no
  Gradle). minSdk 26, targetSdk 34.
- `WsClient`: hand-rolled RFC 6455 client (text frames only;
  `ws://` only in v1 — LAN use case), reader thread +
  Handler callbacks. Reconnect with backoff.
- `FrameView`: custom View; base64 → `BitmapFactory`;
  draws scaled to fit; exposes frame→view coord mapping.
- `MainActivity`: FrameView + latency pill + EdgeHandle +
  BottomSheet (tools) + tab pills adapter + browserUp overlay.
- `ConnectActivity`: server list / manual entry / intent
  handling; stores `br_host`, `br_token`.
- `build.sh`: javac → d8 → aapt2 → zipalign → apksigner
  (same recipe as kunci-lokal; debug key).
- App icon: reuse the tilted-rings icon (assets).

## 7. Build & test

- `./build.sh 1 1.0` → `browserrelay-1.0.apk`.
- Smoke test on the SDK emulator (system-images present):
  install, manual connect to a local relay instance, verify
  auth + one frame renders + tap sends input.
- Deliver via remote-storage public link (Hafidz cannot open
  sandbox:// links — learned 2026-10-11).

## 8. Out of v1 scope

- wss://, in-app QR scanner, pinch-zoom, multi-window,
  notifications, auto-reconnect across network change,
  per-app VPN/Tailscale handling (OS-level).
