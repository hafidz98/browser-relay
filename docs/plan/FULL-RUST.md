# Plan: full-Rust relay (v0.8.0)

Status: approved by Hafidz 2026-10-11 ("Gas full Rust sekalian").
Supersedes the sidecar architecture of MIGRASI-EGUI.md. The Go tree
stays in the repo frozen at v0.7.0 as the reference implementation.

## Goal

One language, one process, one binary: the egui shell and the relay
core in a single Rust executable (~15 MB expected vs 24.6 MB hybrid).
No extract/spawn/watchdog/status.json machinery.

## The spec

The Go implementation is the specification; behavior must match:

- `hub.go` — token auth (15 s deadline), one driver + viewers with
  oldest-first promotion, message schema (`clientMsg`), error strings,
  broadcast drop policy for slow clients (queue 8, frames dropped).
- `browser.go` — CDP session: attach (stopScreencast before close,
  retry once), screencast params (jpeg q65, 1280x800, everyNth 1),
  initial still screenshot, frame ack, input dispatch, tabs via
  DevTools HTTP + Target.createTarget, 3 s watch loop (follow tab
  closed on laptop, broadcast tab changes), mobile emulation
  (UA override + 390x844@3 metrics + touch; restore orig UA).
- `cdp.go` — JSON-RPC over WS: ids, 15 s call timeout, fail-all on
  close, events to handler, fire-and-forget Notify (consumes an id).
- `ws.go` — NOT ported by hand: tokio-tungstenite implements RFC 6455
  on both sides (phone server + CDP client, Origin header on dial).
- `main.go` — config dir + token.txt (6-digit, legacy rotation),
  relay.log tee, LAN IP detection (first up non-loopback non-link-local
  IPv4), screencast defaults.
- Phone client (`client/index.html`, `app.js`) — unchanged; the
  canonical copy moves to `shell/assets/client/`.

## Rust architecture (single crate, tokio on a background thread)

```
main (egui or --nogui)
 |- tokio runtime (background thread)
 |    |- http/ws server task (bind LAN addr; route by peeking the
 |    |  request line: / + /app.js static, /ws upgrade)
 |    |- hub (clients in a Mutex map; per-client mpsc queue 8 +
 |    |  writer task; reader task per client)
 |    |- browser task(s): CDP client, watch loop 3 s,
 |    |  maintain-attach loop 2 s, status writer 1 s
 |    `- shared state: Arc<Mutex<SharedStatus>> read by the panel
 `- egui panel (unchanged UI; CoreState now derives from SharedStatus)
```

CDP HTTP calls (/json/*) use a small hand-rolled HTTP/1.1 client over
tokio TcpStream (Content-Length + chunked decode) — no reqwest.
Clipboard via the `arboard` crate (same get/set text contract).
LAN IP via `if-addrs`. Pairing code via `rand` (OS-seeded).

`--nogui` console mode is kept (banner + block forever) — it is the
E2E test surface, same as the Go binary.

## Verification

- New E2E suite (`tests/e2e.py`, ~14 checks mirroring the M1 suite):
  static files, auth required, bad token closed, welcome/role/tabs,
  frame arrives, ping/pong, input accepted, viewer blocked +
  promotion on driver disconnect, tab new/close, emulation echo.
- Client JS logic test stays green (client untouched).
- `cargo check` + Windows release build; PE + size verified.
- Visual panel check still happens on Hafidz's laptop (no Windows
  display in the VM) — same caveat as v0.6/v0.7.

## Risks

- Async port of a goroutine design: deadlocks around the browser
  mutex are the main hazard — the CDP handle is cloned out of the
  lock before every call, exactly like the Go code does.
- tokio-tungstenite handshake needs the request line before consuming
  the stream: solved by peeking (TcpStream::peek), never reading.
