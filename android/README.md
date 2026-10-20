# browser-relay — native Android client (M3)

Native Android client for browser-relay (`com.hafidz.browserrelay`),
replacing the web client on the phone. Locked UI per
`../mock/android-client-mock.html` (ACC'd 2026-10-11: edge handle +
two-tap interaction).

## Build (no Gradle)

```sh
./build.sh [versionCode] [versionName]   # -> browserrelay-1.0.apk
```

Recipe mirrors `~/workspace/kunci-lokal/build.sh`:
`javac` (Java 8, android.jar only — zero external deps) → `d8`
(`--min-api 26`) → `aapt2 compile`/`link` (generates `R.java` into
`gen/`, compiled with the rest) → add `classes.dex` → `zipalign` →
`apksigner` (debug key in `build/`). SDK: `~/workspace/android-sdk`
(JDK 17, build-tools 34.0.0, `platforms/android-34-ext12`).

## Source layout

| File | Role |
|---|---|
| `WsClient.java` | Hand-rolled RFC 6455 client: text frames only, `ws://` only. Validates `Sec-WebSocket-Accept`, masks all client frames, answers server pings, handles fragmentation. Reader thread; callbacks on the main thread. |
| `RelayClient.java` | Wire protocol, mirroring `../shell/assets/client/app.js` message for message. Client→relay: `auth`, `ping`, `mouse`, `wheel`, `key` (text/code with Win VK codes), `tab` (activate/close/new), `emulation`, `clipboard` (get/set). Relay→client: `welcome`, `status`, `pong`, `role`, `frame`, `tabs`, `clipboard`, `emulation`, `error`. Input sends gated on authed+driver (one driver at a time; viewers read-only). Ping every 2 s; reconnect with backoff. |
| `FrameView.java` | Fullscreen JPEG view (fit-center) + touch→input: single tap = left click, one-finger drag = wheel scroll (content follows finger), long-press = right click. `toFrame()` is the inverse of the draw transform — the same math as app.js `toFrame`. |
| `EdgeHandleView.java` | Purely visual edge handle: docked pill → round button (tap 1) → purple × (sheet open). |
| `MainActivity.java` | Wires it all: latency pill, two-tap handle state machine (drag-to-move with edge snap, 3.5 s idle fade, tap-on-content collapses), tools bottom sheet (URL bar → new tab, tab pills activate/close/new, Back/Forward/Keyboard/Paste/Desktop/Zoom/Find/Settings), `browserUp` overlay driven by `welcome`/`status`, first-run hint, hidden-`EditText` soft-keyboard path (committed text → `key` text; Enter/Backspace → VK codes). SingleTask; handles `browserrelay://pair?c=…&host=…` intents. |
| `ConnectActivity.java` | Manual entry fallback (host:port + 6-digit token), saved as `br_host`/`br_token` in SharedPreferences. |

Pairing: the relay's `/pair` page fires
`intent://pair?c={c}&host={base}#Intent;scheme=browserrelay;…;end`;
the app parses `c` + `host` and derives `ws://host/ws` — zero manual
entry after a QR scan. No in-app QR scanner in v1 (system camera →
`/pair` → intent covers it).

## v1.1 (2026-10-11): wired the tools the relay now supports

Back/Forward (`history` via Page.getNavigationHistory), Reload
(`tab` action `reload`), Find (dialog → `find` via
DOM.performSearch, count shown as toast), Launch Chrome button in
the browserDown overlay (`chrome` action `launch`). Still stubbed:
Zoom, Settings (toast "segera hadir").

Also out of v1: `wss://`, pinch-zoom, in-app QR scanner,
auto-reconnect across network change, notifications.

## Verification

- `./build.sh 1 1.0` → signed APK, `aapt dump badging`:
  `package: name='com.hafidz.browserrelay' versionCode='1' versionName='1.0'`,
  `launchable-activity: com.hafidz.browserrelay.MainActivity`.
- Wire-level test (`/tmp/wsmirror.py`, a faithful Python mirror of
  `WsClient`'s handshake + masked-frame encoding) against a live
  `relay-nogui` + headless Chromium: 101 handshake, accept validation,
  `welcome{driver,browserUp}`, masked `mouse`/`wheel`/`ping` accepted,
  `pong` echo, real JPEG `frame`, `tabs` — all passed.
- Emulator smoke test: pending (emulator was still booting when the
  session ended; AVD `kltest` exists and a cron keeps it alive —
  install via `adb install browserrelay-1.0.apk`, then pair with
  `adb shell am start -a android.intent.action.VIEW -d
  "browserrelay://pair?c=424242&host=http%3A%2F%2F10.0.2.2%3A18765"`,
  and watch `adb logcat -s MainActivity RelayClient` for
  `welcome driver=true` + `frame #1 …`).
