# browser-relay WebSocket API

Protocol version: matches server `VERSION` (e.g. `0.9.56`). Breaking changes bump
the minor version and are noted in git tags.

Transport: `ws://<host>:<port>/ws` — JSON text frames only.

All client→server messages share the envelope:

```json
{
  "type": "<message-type>",
  "token": "<pairing-code-or-empty>",
  "fingerprint": "<client-uuid>",
  "name": "<human-readable-device-name>"
}
```

`token` is the 6-digit pairing code on first connect, empty afterwards (the
`fingerprint` becomes the credential). `name` is shown in the panel device list.

---

## Client → Server

### `auth`
First message after WS open. Server replies `welcome` or `pair_wait` / `error`.

```json
{ "type": "auth", "token": "123456", "fingerprint": "uuid…", "name": "Redmi Note 10" }
```

### `ping`
Keepalive / latency probe. Server replies `pong` echoing `t`.

```json
{ "type": "ping", "t": 1697000000000 }
```

### `mouse`
Pointer input. Coordinates are **frame pixels** (see `toFrame` mapping below).
Server divides by the device scale factor (2.0 when mobile emulation is on,
1.0 otherwise) before dispatching to CDP.

```json
{ "type": "mouse", "action": "down|move|up", "x": 123.4, "y": 456.7, "button": "left|middle|right" }
```

### `wheel`
Scroll. `dx`/`dy` in pixels (positive = scroll down/right).

```json
{ "type": "wheel", "x": 123.4, "y": 456.7, "dx": 0, "dy": -120 }
```

### `key`
Two kinds:
- `kind: "text"` — commit text: `{ "type": "key", "kind": "text", "text": "hello" }`
- `kind: "code"` — Windows virtual-key code, with `down` bool:
  `{ "type": "key", "kind": "code", "vk": 13, "down": true }`

### `tab`
```json
{ "type": "tab", "action": "activate|new|close|reload", "targetId": "<cdp-target-id>", "url": "https://…" }
```
`url` only used with `action: "new"`. `targetId` only with `activate`/`close`.

### `clipboard`
```json
{ "type": "clipboard", "action": "get" }              // server replies {type:"clipboard", text:"…"}
{ "type": "clipboard", "action": "set", "text": "…" }
```

### `emulation`
Toggle mobile emulation. `width`/`height` are the CSS viewport size used for
`Emulation.setDeviceMetricsOverride` (server clamps to 320–1920 × 480–1080,
applies `deviceScaleFactor: 2`).

```json
{ "type": "emulation", "mobile": true, "width": 390, "height": 844 }
```

Clients should send their preference once after `welcome` (mobile clients send
`mobile: true`; desktop clients send `mobile: false`) so one client's setting
doesn't leak into another's session.

If no tab is attached yet the server still accepts the message and echoes it
back — the preference is stored and applied on the next attach, so clients
don't get an error while Chrome is still starting.

### `history`
```json
{ "type": "history", "action": "back|forward" }
```

### `find`
```json
{ "type": "find", "text": "query" }   // server replies {type:"find", count: N}
```

### `chrome`
```json
{ "type": "chrome", "action": "launch|close" }
```
`launch` starts Chrome with the remote-debug profile; `close` quits it.

### `viewport` (informational)
Reserved for future dynamic screencast bounds. Currently ignored by the server.

---

## Server → Client

### `welcome`
Sent after successful `auth`. `paired: true` means the fingerprint is known.

```json
{ "type": "welcome", "driver": true, "mobile": false, "browserUp": true, "paired": true, "host": "DESKTOP-ABC" }
```

`host` is the machine name of the PC running the relay, so the client can
show which computer it is driving.

### `pair_wait`
Fingerprint unknown — client must show the pairing-code UI and re-`auth`
with the code.

### `role`
Driver assignment changed (only one driver at a time; others are view-only).

```json
{ "type": "role", "driver": false }
```

### `status`
Chrome availability changed.

```json
{ "type": "status", "browserUp": false }
```

### `pong`
```json
{ "type": "pong", "t": 1697000000000 }
```

### `frame`
JPEG screenshot, base64. Broadcast to all authed clients on every screencast
frame (server caps queue per client; slow clients drop frames, never block).

```json
{ "type": "frame", "data": "<base64-jpeg>" }
```

### `tabs`
```json
{ "type": "tabs", "tabs": [{ "id": "…", "title": "…", "url": "…", "active": true }] }
```

### `clipboard`
Reply to `clipboard/get`: `{ "type": "clipboard", "text": "…" }`

### `emulation`
Echo after applying: `{ "type": "emulation", "mobile": true }`

### `find`
Reply with match count: `{ "type": "find", "count": 3 }`

### `chrome`
Echo after action: `{ "type": "chrome", "action": "launch" }`

### `error`
```json
{ "type": "error", "message": "…" }
```

---

## Coordinate mapping (`toFrame`)

The screencast frame may be letterboxed inside the view (`object-fit: contain`
on web; fit-center in `FrameView.java`). Client input coordinates must be mapped
from view pixels to frame pixels:

```
fx = (viewX - offsetX) / scale
fy = (viewY - offsetY) / scale
```

where `scale = min(viewW / frameW, viewH / frameH)` and the offsets center the
frame. Reject points outside the frame bounds. The server then divides by the
device scale factor to get CSS pixels for CDP.

## HTTP endpoints (same port)

- `GET /` — web client
- `GET /app.js` — web client script
- `GET /pair?c=<code>` — pairing landing page. With a **valid, unused** code it
  shows the code and deep-links into the Android app (`intent://` + a
  `browserrelay://` fallback). With an **expired, burned or wrong** code it
  renders an explicit error page instead of the success copy — pairing codes
  are single-use and expire after 5 minutes, and the panel shows the
  remaining time.
- `GET /app.apk` — the Android client, when an APK is staged in the config dir
- `GET /ws` — the WebSocket endpoint (see above)
