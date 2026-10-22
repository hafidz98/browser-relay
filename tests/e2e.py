#!/usr/bin/env python3
"""E2E suite for the Rust relay (relay-nogui) vs headless Chromium.

Mirrors the M1 Go-core checks and adds the v0.8.0 QR pairing routes.
Usage: python3 tests/e2e.py [relay-binary]
Env: RELAY_PORT (default 18765), CDP_PORT (default 9222).
"""
import base64
import json
import os
import socket
import struct
import subprocess
import sys
import time
import urllib.request

RELAY_PORT = int(os.environ.get("RELAY_PORT", "18765"))
CDP_PORT = int(os.environ.get("CDP_PORT", "9222"))
TOKEN = "424242"
BASE = f"http://127.0.0.1:{RELAY_PORT}"

passed = 0
failed = 0


def check(name, ok, detail=""):
    global passed, failed
    if ok:
        passed += 1
        print(f"  PASS {name}")
    else:
        failed += 1
        print(f"  FAIL {name} {detail}")


def http_get(path):
    with urllib.request.urlopen(BASE + path, timeout=5) as r:
        return r.status, r.read()


def http_status(path):
    try:
        with urllib.request.urlopen(BASE + path, timeout=5) as r:
            return r.status
    except urllib.error.HTTPError as e:
        return e.code


class WS:
    """Minimal RFC6455 client (masked text frames)."""

    def __init__(self, port, path="/ws"):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout=5)
        key = base64.b64encode(os.urandom(16)).decode()
        req = (
            f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
            "Upgrade: websocket\r\nConnection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        self.sock.sendall(req.encode())
        buf = b""
        while b"\r\n\r\n" not in buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise ConnectionError("handshake closed")
            buf += chunk
        assert b"101" in buf.split(b"\r\n")[0], buf[:100]
        self.buf = buf.split(b"\r\n\r\n", 1)[1]
        self.msgs = []
        self.last_tabs = None  # most recent tabs message seen on this conn

    def send(self, obj):
        data = json.dumps(obj).encode()
        mask = os.urandom(4)
        header = bytearray([0x81])
        n = len(data)
        if n < 126:
            header.append(0x80 | n)
        elif n < 65536:
            header.append(0x80 | 126)
            header += struct.pack(">H", n)
        else:
            header.append(0x80 | 127)
            header += struct.pack(">Q", n)
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
        self.sock.sendall(bytes(header) + mask + masked)

    def _read_exact(self, n):
        while len(self.buf) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError("closed")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def recv_msg(self, timeout=5.0):
        if self.msgs:
            return self.msgs.pop(0)
        self.sock.settimeout(timeout)
        while True:
            h = self._read_exact(2)
            op = h[0] & 0x0F
            n = h[1] & 0x7F
            if n == 126:
                n = struct.unpack(">H", self._read_exact(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self._read_exact(8))[0]
            payload = self._read_exact(n) if n else b""
            if op == 0x8:
                raise ConnectionError("close frame")
            if op == 0x9:
                continue
            if op in (0x1, 0x2):
                text = payload.decode("utf-8", "replace")
                try:
                    parsed = json.loads(text)
                    if parsed.get("type") == "tabs":
                        self.last_tabs = parsed
                except (json.JSONDecodeError, AttributeError):
                    pass
                return text

    def wait_for(self, pred, timeout=6.0, label=""):
        """Collect messages until pred(msg dict) is true."""
        end = time.time() + timeout
        seen = []
        while time.time() < end:
            try:
                raw = self.recv_msg(timeout=max(0.1, end - time.time()))
            except (socket.timeout, TimeoutError):
                break
            try:
                m = json.loads(raw)
            except json.JSONDecodeError:
                continue
            seen.append(m)
            if pred(m):
                return m, seen
        return None, seen

    def close(self):
        try:
            self.sock.close()
        except Exception:
            pass


def main():
    print("== HTTP routes ==")
    st, body = http_get("/")
    check("GET / serves client", st == 200 and b"browser-relay" in body.lower())
    st, _ = http_get("/app.js")
    check("GET /app.js", st == 200)
    st, body = http_get(f"/pair?c={TOKEN}")
    html = body.decode("utf-8", "replace")
    check(
        "GET /pair valid code -> intent + fallback",
        st == 200
        and "intent://pair" in html
        and "browserrelay" in html
        and f"/?code={TOKEN}" in html,
    )
    # The landing page must NOT reveal whether the code is the current
    # one: it renders the same page for every code, so guessing over
    # HTTP buys an attacker nothing. Compare the two pages with the
    # code itself masked out — they must be identical.
    st_bad, body_bad = http_get("/pair?c=000000")
    masked_ok = body.replace(TOKEN.encode(), b"XXXXXX")
    masked_bad = body_bad.replace(b"000000", b"XXXXXX")
    check(
        "GET /pair does not leak whether the code is current",
        st_bad == 200 and masked_ok == masked_bad,
    )
    check(
        "GET /pair wrong code still offers the deep link",
        st_bad == 200 and b"intent://pair" in body_bad,
    )
    check("GET /app.apk 404 (none staged)", http_status("/app.apk") == 404)
    check("GET /nope 404", http_status("/nope") == 404)

    print("== WS auth ==")
    ws = WS(RELAY_PORT)
    ws.send({"type": "mouse", "action": "move", "x": 1, "y": 1})
    m, _ = ws.wait_for(lambda m: m.get("type") == "error", 3)
    check("pre-auth input rejected", m and m.get("message") == "auth required", str(m))
    ws.close()

    ws = WS(RELAY_PORT)
    ws.send({"type": "auth", "token": "999999", "fingerprint": "e2e-bad-fp"})
    m, _ = ws.wait_for(lambda m: m.get("type") == "error", 3)
    check("bad token error", m and m.get("message") == "bad token", str(m))
    try:
        ws.recv_msg(timeout=2)
        closed = False
    except (ConnectionError, socket.timeout, TimeoutError):
        closed = True
    check("bad token closes connection", closed)
    ws.close()

    print("== pairing: fingerprint reconnect + unknown device + pair_request ==")
    # Fingerprint-only reconnect (driver paired above with e2e-driver-fp
    # in the driver session below — do a fresh pair first here).
    p = WS(RELAY_PORT)
    p.send({"type": "auth", "token": TOKEN, "fingerprint": "e2e-pair-fp", "name": "e2e-pair"})
    m, _ = p.wait_for(lambda m: m.get("type") == "welcome", 5)
    check("pair with code + fingerprint -> welcome", m is not None, str(m))
    p.close()
    # Reconnect with fingerprint only (no token).
    p2 = WS(RELAY_PORT)
    p2.send({"type": "auth", "fingerprint": "e2e-pair-fp"})
    m, _ = p2.wait_for(lambda m: m.get("type") == "welcome", 5)
    check("fingerprint-only reconnect -> welcome", m is not None, str(m))
    p2.close()
    # Unknown fingerprint.
    u = WS(RELAY_PORT)
    u.send({"type": "auth", "fingerprint": "e2e-never-paired"})
    m, _ = u.wait_for(lambda m: m.get("type") == "error", 3)
    check("unknown fingerprint rejected", m and m.get("message") == "unknown device", str(m))
    u.close()
    # Pairing request flow.
    r = WS(RELAY_PORT)
    r.send({"type": "pair_request", "fingerprint": "e2e-req-fp", "name": "e2e-req"})
    m, _ = r.wait_for(lambda m: m.get("type") == "pair_wait", 3)
    check("pair_request -> pair_wait", m is not None, str(m))
    r.close()

    print("== driver session ==")
    d = WS(RELAY_PORT)
    d.send({"type": "auth", "token": TOKEN, "fingerprint": "e2e-driver-fp", "name": "e2e-driver"})
    m, _ = d.wait_for(lambda m: m.get("type") == "welcome", 5)
    check("welcome driver=true", m and m.get("driver") is True, str(m))
    check(
        "welcome browserUp=true (chromium running)",
        m and m.get("browserUp") is True,
        str(m),
    )
    m, _ = d.wait_for(lambda m: m.get("type") == "tabs", 5)
    check("tabs received", m and len(m.get("tabs", [])) >= 1, str(m)[:120])
    m, _ = d.wait_for(lambda m: m.get("type") == "frame", 8)
    check("frame received", m and len(m.get("data", "")) > 1000, str(m)[:80])

    d.send({"type": "ping", "t": 123456})
    m, _ = d.wait_for(lambda m: m.get("type") == "pong", 3)
    check("ping -> pong echoes t", m and m.get("t") == 123456, str(m))

    d.send({"type": "mouse", "action": "move", "x": 10, "y": 10, "button": "left"})
    d.send({"type": "key", "kind": "text", "text": "hi"})
    d.send({"type": "wheel", "x": 5, "y": 5, "dx": 0, "dy": 120})
    time.sleep(0.6)
    errs = [x for x in d.msgs if x.get("type") == "error"]
    m, drained = d.wait_for(lambda m: False, 0.3)
    errs += [x for x in drained if x.get("type") == "error"]
    check("input accepted (no errors)", not errs, str(errs))

    print("== viewer + promotion ==")
    v = WS(RELAY_PORT)
    v.send({"type": "auth", "token": TOKEN, "fingerprint": "e2e-viewer-fp", "name": "e2e-viewer"})
    m, _ = v.wait_for(lambda m: m.get("type") == "welcome", 5)
    check("viewer welcome driver=false", m and m.get("driver") is False, str(m))
    v.send({"type": "mouse", "action": "move", "x": 1, "y": 1})
    m, _ = v.wait_for(lambda m: m.get("type") == "error", 3)
    check(
        "viewer input blocked",
        m and m.get("message", "").startswith("view-only"),
        str(m),
    )
    d.close()
    m, _ = v.wait_for(
        lambda m: m.get("type") == "role" and m.get("driver") is True, 5
    )
    check("promotion on driver disconnect", m is not None, str(m))

    print("== tabs + emulation (as promoted driver) ==")
    # Baseline from the connection's tracked last-tabs (earlier waits
    # may already have consumed the snapshot message itself).
    deadline = time.time() + 5
    while v.last_tabs is None and time.time() < deadline:
        v.wait_for(lambda m: False, 0.3)
    orig_ids = {t["id"] for t in (v.last_tabs or {}).get("tabs", [])}
    orig_n = len(orig_ids)
    v.send({"type": "tab", "action": "new", "url": "example.com"})
    m, _ = v.wait_for(
        lambda m: m.get("type") == "tabs" and len(m.get("tabs", [])) > orig_n, 8
    )
    check("tab new grows tab list", m is not None, str(m)[:120])
    if m:
        added = [t for t in m["tabs"] if t["id"] not in orig_ids]
        if added:
            v.send({"type": "tab", "action": "close", "targetId": added[0]["id"]})
            m2, _ = v.wait_for(
                lambda m: m.get("type") == "tabs"
                and len(m.get("tabs", [])) == orig_n,
                8,
            )
            check("tab close shrinks list", m2 is not None)
        else:
            check("tab close shrinks list", False, "no new tab id in list")
    v.send({"type": "emulation", "mobile": True})
    m, _ = v.wait_for(lambda m: m.get("type") == "emulation", 5)
    check("emulation echo mobile=true", m and m.get("mobile") is True, str(m))
    v.send({"type": "emulation", "mobile": False})

    v.send({"type": "clipboard", "action": "get"})
    m, _ = v.wait_for(
        lambda m: m.get("type") in ("clipboard", "error"), 4
    )
    check("clipboard get answered (ok or env error)", m is not None, str(m)[:100])
    v.close()

    print("== browser-down indicator (dead CDP relay) ==")
    binary = sys.argv[1] if len(sys.argv) > 1 else None
    if binary:
        port2 = RELAY_PORT + 1
        proc = subprocess.Popen(
            [binary, "--nogui", "--listen", "127.0.0.1", "--port", str(port2),
             "--token", TOKEN, "--cdp", "http://127.0.0.1:9999"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            ok = False
            for _ in range(40):
                try:
                    with urllib.request.urlopen(
                        f"http://127.0.0.1:{port2}/", timeout=1
                    ) as r:
                        ok = r.status == 200
                    break
                except Exception:
                    time.sleep(0.25)
            w2 = WS(port2)
            w2.send({"type": "auth", "token": TOKEN, "fingerprint": "e2e-driver-fp", "name": "e2e-driver"})
            m, _ = w2.wait_for(lambda m: m.get("type") == "welcome", 5)
            check(
                "dead-CDP welcome browserUp=false",
                ok and m and m.get("browserUp") is False,
                str(m),
            )
            w2.close()
        finally:
            proc.terminate()
    else:
        check("dead-CDP welcome browserUp=false", False, "no relay binary arg")

    print(f"\n== {passed} passed, {failed} failed ==")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
