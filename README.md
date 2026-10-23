# browser-relay

Drive your laptop's Chrome from your phone. Logins stay on the laptop —
the phone only receives a live view of the browser and sends back taps
and keystrokes. LAN-first, no cloud, no Google account.

## How it works

`relay.exe` is a small secure gateway on the laptop. Chrome's remote
debugging port only listens on localhost and has no password, so the
relay bridges it to your LAN, adds a token login, and exposes a
least-privilege protocol: JPEG frames out, input events in. It cannot
run JavaScript in your tabs or read your cookies — by design.

## Install (3 steps, ~5 minutes)

**1. Run the relay.** Double-click `relay-0.8.0.exe`. The control panel
opens as a native app window (egui — no browser engine, no WebView2
runtime needed) and the relay sits in the system tray (left-click or
menu: Open panel, Launch/Close Chrome, Quit). Closing the window hides
it to the tray. One single file, all Rust — panel and relay in one
process. Config lives in `%APPDATA%\browser-relay\`.

Click **Launch Chrome**: it starts Chrome on its homepage with a
dedicated remote profile (`%USERPROFILE%\chrome-remote`) and the debug
port enabled. This runs *alongside* your everyday Chrome — no need to
close anything. **Close** shuts that Chrome back down.

Tick **Headless Chrome (no window)** before launching if you don't want
a visible browser window at all (the panel's status line still shows
whether Chrome is connected). The choice is remembered.

(Why a separate profile? Since Chrome 136, `--remote-debugging-port` is
ignored on the default data directory — [official change](https://developer.chrome.com/blog/remote-debugging-port).
The remote profile is fresh, so log in to the sites you need, or sign in
to Chrome Sync to pull in bookmarks/passwords.)

**2. Pair the phone.** Click the **QR icon** next to the pairing code
and scan the code with any QR scanner on your phone (same Wi-Fi).
The landing page opens the native app if it's installed; otherwise it
drops you into the web client already paired — no typing. Prefer
manual? Open the **Phone URL** in the phone browser and enter the
6-digit **pairing code** once — it's remembered afterwards.
(The code is stored in `%APPDATA%\browser-relay\token.txt`; delete the
file and restart to get a new one. Once the Android app exists, drop
`browser-relay.apk` into that same config folder and the QR landing
page gains a download button — the laptop serves the app itself, no
internet needed.)

**3. Drive.** Tap, scroll, type. The top bar shows live latency and
connection state; the tab strip handles open/close/switch (+ opens a URL
sheet — bare domains like `google.com` get `https://` automatically).
Bottom dock: soft keyboard, 📱 mobile-site rendering, clipboard
both ways, pause. All dialogs are in-app bottom sheets — no browser
popups.

The panel shows Chrome's status (green = connected), connected clients,
and can **hide to the tray** — closing the window (×) hides it too; the
relay keeps running. Right-click the tray icon to show it again or quit.

## Reaching the relay from outside your home

On your own network the phone talks to the PC's LAN address. Away from
home that address means nothing, and forwarding a port on the router is
a bad idea here: most home lines in Indonesia sit behind CGNAT (no
routable address at all), the transport is a plain `ws://` stream, and
the pairing code is only six digits.

Use a private mesh instead. [Tailscale](https://tailscale.com) is free
for personal use and needs no router change, no domain and no open port:

1. Sign up at tailscale.com with a Gmail/GitHub/Apple account. A tailnet
   created with a custom domain counts as business use.
2. Install Tailscale on the PC and sign in. It runs as a service and
   starts with Windows.
3. Install the Tailscale app on the phone and sign in to the same
   account.
4. Run `tailscale ip -4` on the PC and note the `100.x.y.z` address.
5. Start the relay, then enter `100.x.y.z:8765` as the server address in
   the app — or run `relay.exe --listen 100.x.y.z` so the panel URL and
   the QR code point at the tailnet address directly (the relay then
   listens only there, so LAN access stops while it runs).
6. Check `tailscale status`: a direct connection is as fast as the LAN.
   When it says `relay "sin"` the traffic goes through a DERP server in
   Singapore — still fine, only a little slower.

Everything the phone sends stays inside the WireGuard tunnel, so the
`ws://` traffic is encrypted on the way even though the relay itself
still speaks plain WebSocket. The pairing code stays on top of that: five
wrong guesses burn the code, and one address can try ten times a minute.

Alternatives, if a mesh is not an option: a Cloudflare Tunnel works for
the browser client only (the phone's browser negotiates `wss://` by
itself, while the Android app speaks `ws://`), and a small VPS running
`rathole` works for both but costs a few dollars a month and adds a round
trip.

## Notes

- The GUI is the default on Windows; run `relay.exe --nogui` for the
  old console mode (e.g. for scripts/services).
- The relay attaches to the remote-profile Chrome from step 1 — it never
  touches your everyday Chrome profile.
- If the panel says Chrome isn't running, click **Launch Chrome** (or
  start it manually with the flags below).
- Manual Chrome launch, if you ever need it:

  ```
  "C:\Program Files\Google\Chrome\Application\chrome.exe" --user-data-dir="%USERPROFILE%\chrome-remote" --remote-debugging-port=9222 --remote-allow-origins=* --disable-backgrounding-occluded-windows --disable-renderer-backgrounding
  ```

  Sanity check: `http://127.0.0.1:9222/json/version` should show JSON.
- The remote profile (`chrome-remote`) is separate from your daily
  profile by design — Chrome 136+ refuses the debug port on the default
  data directory for security (it would let any local program steal
  cookies). Use Chrome Sync or log in again inside the remote profile.
- First client to connect becomes the driver; others watch read-only.
- Mobile 📱 mode also changes what the laptop window shows (same tab).
- No audio in v1 (Chrome's screencast protocol carries no sound).
- Reaching the laptop from outside your Wi-Fi: run Tailscale on both
  devices and start the relay with `--listen <tailscale-ip>`.

## Pakai profil Chrome tertentu

Sejak Chrome 136, `--remote-debugging-port` **wajib** ditemani
`--user-data-dir` ke folder non-default — jadi "pakai Profile 9
langsung" sudah tidak bisa lagi (Chrome sengaja memblokirnya supaya
malware tidak bisa mencuri cookie lewat debug port).

Pola yang didukung: satu folder data khusus untuk remote, misal
`%USERPROFILE%\chrome-remote` seperti di langkah 1. Kalau mau sesi dari
Profile 9, opsi realistisnya: login ulang di profil remote itu, atau
nyalakan Chrome Sync di Profile 9 lalu sign-in di profil remote supaya
bookmark/password ikut.

Butuh dua instance debug sekaligus (port berbeda)? Contoh instance kedua:

```
"C:\Program Files\Google\Chrome\Application\chrome.exe" --user-data-dir="C:\Users\kamu\chrome-remote-2" --remote-debugging-port=9223 --remote-allow-origins=*
```

Lalu jalanin relay dengan:

```
relay.exe --cdp http://127.0.0.1:9223
```

## Building from source

Everything is Rust since v0.8.0 (the Go core under `relay/` is kept
frozen at v0.7.0 as the reference implementation). One script
(Rust + MinGW) builds the shipped artifact:

```
./build-windows.sh        # -> relay-<version>.exe
```

Console-only relay for testing on any OS (no GUI dependencies):

```
cd shell && cargo build --bin relay-nogui --no-default-features
```

E2E suite (needs a headless Chromium with the debug port on 9222):

```
python3 tests/e2e.py
```

Architecture: `docs/plan/FULL-RUST.md`. The previous Wails/WebView2
shell is still in the Go tree behind a build tag (`go build -tags
wails`); see `docs/plan/MIGRASI-EGUI.md`.

Flags: `--port` (default 8765), `--listen` (default: auto LAN IP),
`--token` (override the stored pairing code), `--cdp` (Chrome DevTools
URL), `--nogui` (console mode).
