// browser-relay web client: fullscreen viewport + edge-handle tools,
// same design language as the Android mock (adapted for desktop).
(function () {
  "use strict";
  var img = document.getElementById("screen");
  var status = document.getElementById("status");
  var latencyEl = document.getElementById("latency");
  var latencyMs = document.getElementById("latencyMs");
  var authBox = document.getElementById("auth");
  var authErr = document.getElementById("authErr");
  var tokenInput = document.getElementById("token");
  var tabsEl = document.getElementById("tabs");
  var urlText = document.getElementById("urlText");
  var kbd = document.getElementById("kbd");
  var ws = null, authed = false, isDriver = false;
  var token = localStorage.getItem("br_token") || "";
  // Stable device fingerprint (the pairing contract identity).
  var fingerprint = localStorage.getItem("br_fingerprint") || "";
  if (!fingerprint) {
    fingerprint = "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, function (c) {
      var r = Math.random() * 16 | 0, v = c === "x" ? r : (r & 0x3 | 0x8);
      return v.toString(16);
    });
    try { localStorage.setItem("br_fingerprint", fingerprint); } catch (e) {}
  }
  var deviceName = "Web client";
  // Single-use QR code from ?code= (consumed on first connect).
  var qrCode = null;

  var authDesc = document.getElementById("authDesc");
  var reqBtn = document.getElementById("reqCode");
  var goBtn = document.getElementById("go");

  function showRequestButton() {
    authBox.style.display = "flex";
    authDesc.innerHTML = "Perangkat ini belum terpair.<br>Minta kode dari panel PC.";
    tokenInput.style.display = "none";
    goBtn.style.display = "none";
    reqBtn.style.display = "block";
    authErr.textContent = "";
  }

  function showPairInput(hint) {
    authBox.style.display = "flex";
    authDesc.textContent = hint;
    tokenInput.style.display = "block";
    tokenInput.value = "";
    goBtn.style.display = "block";
    reqBtn.style.display = "none";
    authErr.textContent = "";
    // Don't auto-focus on mobile — it pops the keyboard unexpectedly.
    // tokenInput.focus();
  }

  // Simplified: on "unknown device", show the input directly.
  // The panel always displays the current code — no need for a
  // separate "request" step.
  function showPairingUI() {
    showPairInput("Ketik 6 digit kode dari panel PC.");
  }

  reqBtn.onclick = function () {
    if (ws && ws.readyState === 1) {
      ws.send(JSON.stringify({ type: "pair_request", fingerprint: fingerprint, name: deviceName }));
      authDesc.textContent = "Meminta kode…";
      reqBtn.style.display = "none";
    }
  };

  goBtn.onclick = function () {
    var code = tokenInput.value.trim();
    if (!/^\d{6}$/.test(code)) {
      authErr.textContent = "Kode harus 6 digit";
      return;
    }
    if (ws && ws.readyState === 1) {
      goBtn.disabled = true;
      goBtn.textContent = "Menghubungkan…";
      ws.send(JSON.stringify({ type: "auth", token: code, fingerprint: fingerprint, name: deviceName }));
      authDesc.textContent = "Menghubungkan…";
      authErr.textContent = "";
    } else {
      authErr.textContent = "Koneksi terputus — tunggu sebentar lalu coba lagi";
    }
  };

  var VK = { Enter: 13, Backspace: 8, Tab: 9, Escape: 27, ArrowLeft: 37,
    ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, Delete: 46, Home: 36,
    End: 35, PageUp: 33, PageDown: 34 };

  function setStatus(s) { status.textContent = s; }

  var browserDownEl = document.getElementById("browserDown");
  function showBrowserDown(down) {
    if (browserDownEl) browserDownEl.style.display = down ? "flex" : "none";
  }

  // Remote Chrome launch from the client.
  var launchBtn = document.getElementById("launchChromeBtn");
  if (launchBtn) launchBtn.onclick = function () {
    launchBtn.disabled = true;
    launchBtn.textContent = "Launching…";
    send({ type: "chrome", action: "launch" });
    setTimeout(function () {
      launchBtn.disabled = false;
      launchBtn.innerHTML = '<svg viewBox="0 0 24 24"><path d="M8 5.5v13l11-6.5z"/></svg> Launch Chrome';
    }, 4000);
  };

  // ---- toast ----
  var toastEl = document.getElementById("toast"), tTimer = null;
  function toast(msg) {
    toastEl.textContent = msg; toastEl.classList.add("show");
    clearTimeout(tTimer); tTimer = setTimeout(function () { toastEl.classList.remove("show"); }, 1700);
  }

  // ---- text prompt modal ----
  var promptEl = document.getElementById("prompt"),
      promptTitle = document.getElementById("promptTitle"),
      promptInput = document.getElementById("promptInput"),
      promptOk = document.getElementById("promptOk"),
      promptCancel = document.getElementById("promptCancel"),
      promptCb = null;
  function openPrompt(title, value, placeholder, okText, cb) {
    promptTitle.textContent = title;
    promptInput.value = value || "";
    promptInput.placeholder = placeholder || "";
    promptOk.textContent = okText || "OK";
    promptCb = cb;
    promptEl.classList.add("open");
    setTimeout(function () { promptInput.focus(); promptInput.select(); }, 60);
  }
  function closePrompt() { promptEl.classList.remove("open"); promptCb = null; }
  promptOk.onclick = function () { var v = promptInput.value, cb = promptCb; closePrompt(); if (cb) cb(v); };
  promptCancel.onclick = closePrompt;
  promptEl.addEventListener("click", function (e) { if (e.target === promptEl) closePrompt(); });

  // ---- latency probe ----
  setInterval(function () {
    if (ws && ws.readyState === 1 && authed)
      ws.send(JSON.stringify({ type: "ping", t: Date.now() }));
  }, 2000);
  function setLatency(ms) {
    latencyMs.textContent = ms + " ms";
    latencyEl.className = ms < 120 ? "" : ms < 350 ? "warn" : "bad";
  }

  function connect() {
    var proto = location.protocol === "https:" ? "wss:" : "ws:";
    ws = new WebSocket(proto + "//" + location.host + "/ws");
    ws.onopen = function () {
      // QR code (single-use) takes precedence; else fingerprint reconnect.
      // BUT: if the user is mid-pairing (input field visible), don't auto-auth —
      // let them click Hubungkan manually. Auto-auth without token would
      // get "unknown device" and reset the UI back to the request button.
      var pairingInputVisible = tokenInput.style.display !== "none" &&
                                authBox.style.display !== "none";
      if (qrCode) {
        var c = qrCode; qrCode = null;
        ws.send(JSON.stringify({ type: "auth", token: c, fingerprint: fingerprint, name: deviceName }));
      } else if (!pairingInputVisible) {
        ws.send(JSON.stringify({ type: "auth", fingerprint: fingerprint }));
      }
      // else: wait for user to click Hubungkan
    };
    ws.onmessage = function (ev) {
      var m;
      try { m = JSON.parse(ev.data); } catch (e) { return; }
      switch (m.type) {
        case "welcome":
          authed = true; isDriver = !!m.driver;
          authBox.style.display = "none";
          authErr.textContent = "";
          goBtn.disabled = false;
          goBtn.textContent = "Hubungkan";
          setStatus(isDriver ? "driving" : "view only");
          if (m.browserUp !== undefined) showBrowserDown(!m.browserUp);
          // Auto-set emulation mode on first connect based on device type.
          // Mobile devices get mobile emulation; desktop gets desktop mode.
          // This ensures the emulation matches the current client, not a
          // stale setting from a previous client.
          if (!window.__autoMobileDone) {
            window.__autoMobileDone = true;
            var isMobileDevice = /Android|iPhone|iPad|iPod|Mobile/i.test(navigator.userAgent) ||
              (navigator.maxTouchPoints > 1 && Math.min(screen.width, screen.height) < 768);
            send({ type: "emulation", mobile: isMobileDevice, width: 390, height: 844 });
          }
          break;
        case "status":
          showBrowserDown(!m.browserUp);
          break;
        case "pong":
          if (m.t) setLatency(Date.now() - m.t);
          break;
        case "role":
          isDriver = !!m.driver;
          setStatus(isDriver ? "driving" : "view only");
          break;
        case "frame":
          img.src = "data:image/jpeg;base64," + m.data;
          break;
        case "tabs": renderTabs(m.tabs || []); break;
        case "clipboard":
          if (m.text !== undefined) {
            openPrompt("Laptop clipboard", m.text, "Type or paste text…", "Send", function (t) {
              if (t !== m.text) {
                send({ type: "clipboard", action: "set", text: t });
                toast("Sent — press Ctrl+V in the page");
              }
            });
          }
          break;
        case "emulation":
          setDesktop(m.mobile);
          break;
        case "find":
          toast(m.count + " match" + (m.count === 1 ? "" : "es"));
          break;
        case "chrome":
          if (m.action === "launch") toast("Chrome launching…");
          break;
        case "pair_wait":
          // Code is now on the PC panel — show the input.
          showPairInput("Cek panel di PC, ketik 6 digit kode yang muncul.");
          break;
        case "error":
          if (!authed) {
            goBtn.disabled = false;
            goBtn.textContent = "Hubungkan";
            if (m.message === "unknown device") {
              showPairingUI();
            } else {
              authDesc.textContent = "Pairing gagal";
              authErr.textContent = m.message || "connection failed";
            }
          }
          else toast("error: " + m.message);
          break;
      }
    };
    ws.onclose = function () {
      authed = false;
      latencyMs.textContent = "— ms";
      latencyEl.className = "";
      showBrowserDown(false);
      setStatus("disconnected — retrying…");
      if (!authed && authBox.style.display !== "none") {
        authDesc.textContent = "Koneksi terputus — mencoba lagi…";
      }
      setTimeout(connect, 2000);
    };
  }

  function renderTabs(tabs) {
    tabsEl.innerHTML = "";
    var activeUrl = "—";
    tabs.forEach(function (t) {
      if (t.active) activeUrl = t.url || t.title || "—";
      var d = document.createElement("div");
      d.className = "tab" + (t.active ? " on" : "");
      d.title = t.url || "";
      var fav = document.createElement("span");
      fav.className = "fav";
      fav.style.background = "#" + ((t.id.charCodeAt(0) * 2654435761) % 0xffffff).toString(16).padStart(6, "0");
      var label = document.createElement("span");
      label.textContent = t.title || t.url || "tab";
      var x = document.createElement("span");
      x.className = "x"; x.textContent = "×";
      x.onclick = function (e) {
        e.stopPropagation();
        send({ type: "tab", action: "close", targetId: t.id });
      };
      d.appendChild(fav); d.appendChild(label); d.appendChild(x);
      d.onclick = function () {
        send({ type: "tab", action: "activate", targetId: t.id });
      };
      tabsEl.appendChild(d);
    });
    var nb = document.createElement("button");
    nb.id = "newtab"; nb.textContent = "+"; nb.title = "New tab";
    nb.onclick = function () {
      openPrompt("Open URL", "https://", "https://example.com", "Open", function (u) {
        u = (u || "").trim();
        if (u) send({ type: "tab", action: "new", url: u });
      });
    };
    tabsEl.appendChild(nb);
    urlText.textContent = activeUrl;
  }

  // --- input: map client pixels -> frame pixels ---
  function toFrame(e) {
    var r = img.getBoundingClientRect();
    if (!img.naturalWidth || !r.width) return null;
    return {
      x: (e.clientX - r.left) * img.naturalWidth / r.width,
      y: (e.clientY - r.top) * img.naturalHeight / r.height
    };
  }
  function send(o) { if (ws && authed && isDriver) ws.send(JSON.stringify(o)); }

  var touchStart = null;
  img.addEventListener("pointerdown", function (e) {
    var p = toFrame(e); if (!p) return;
    try { img.setPointerCapture(e.pointerId); } catch (err) {}
    // For touch: DON'T send mouse down immediately. Wait to see if it's
    // a tap (click) or a drag (scroll). This prevents accidental clicks
    // when the user intends to scroll.
    if (e.pointerType === "touch") {
      touchStart = { x: e.clientX, y: e.clientY, px: p.x, py: p.y, moved: false, id: e.pointerId };
    } else {
      send({ type: "mouse", action: "down", x: p.x, y: p.y, button: btn(e) });
    }
    e.preventDefault();
  });
  var moveQueued = null;
  img.addEventListener("pointermove", function (e) {
    if (!moveQueued) {
      moveQueued = {};
      requestAnimationFrame(function () {
        var p = toFrame(e); moveQueued = null;
        if (!p) return;
        // Touch drag = scroll. If finger moved >10px, it's a scroll gesture.
        if (e.pointerType === "touch" && touchStart && e.pointerId === touchStart.id) {
          var dx = e.clientX - touchStart.x, dy = e.clientY - touchStart.y;
          if (!touchStart.moved && Math.hypot(dx, dy) > 10) touchStart.moved = true;
          if (touchStart.moved) {
            send({ type: "wheel", x: p.x, y: p.y, dx: -dx, dy: -dy });
            touchStart.x = e.clientX; touchStart.y = e.clientY;
            return;
          }
          // Not yet a scroll, don't send mouse move (avoid hover effects).
          return;
        }
        send({ type: "mouse", action: "move", x: p.x, y: p.y, button: btn(e) });
      });
    }
  });
  img.addEventListener("pointerup", function (e) {
    var p = toFrame(e); if (!p) return;
    // Touch: if it was a tap (not moved), send click (down+up).
    // If it was a scroll, send nothing (avoid accidental click).
    if (e.pointerType === "touch" && touchStart) {
      var wasScroll = touchStart.moved;
      touchStart = null;
      if (!wasScroll) {
        send({ type: "mouse", action: "down", x: p.x, y: p.y, button: "left" });
        send({ type: "mouse", action: "up", x: p.x, y: p.y, button: "left" });
      }
      return;
    }
    touchStart = null;
    send({ type: "mouse", action: "up", x: p.x, y: p.y, button: btn(e) });
  });
  img.addEventListener("pointercancel", function () { touchStart = null; });
  img.addEventListener("wheel", function (e) {
    var p = toFrame(e); if (!p) return;
    send({ type: "wheel", x: p.x, y: p.y, dx: e.deltaX, dy: e.deltaY });
    e.preventDefault();
  }, { passive: false });
  img.addEventListener("contextmenu", function (e) { e.preventDefault(); });
  function btn(e) {
    return e.button === 2 ? "right" : e.button === 1 ? "middle" : "left";
  }

  // --- keyboard (physical) ---
  document.addEventListener("keydown", function (e) {
    if (!authed || authBox.style.display !== "none") return;
    if (e.target === kbd || e.target === tokenInput || e.target === promptInput) return;
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
      send({ type: "key", kind: "text", text: e.key });
      e.preventDefault();
    } else if (VK[e.key]) {
      send({ type: "key", kind: "code", vk: VK[e.key], down: true });
      e.preventDefault();
    }
  });
  document.addEventListener("keyup", function (e) {
    if (!authed || authBox.style.display !== "none") return;
    if (e.target === kbd || e.target === tokenInput || e.target === promptInput) return;
    if (VK[e.key]) send({ type: "key", kind: "code", vk: VK[e.key], down: false });
  });
  kbd.addEventListener("input", function () {
    if (kbd.value) { send({ type: "key", kind: "text", text: kbd.value }); kbd.value = ""; }
  });

  // --- tools sheet ---
  var body = document.body, fab = document.getElementById("fab");
  var sheetOpenedAt = 0;
  function toggleSheet(force) {
    var open = typeof force === "boolean" ? force : !body.classList.contains("sheet-open");
    if (open) sheetOpenedAt = Date.now();
    body.classList.toggle("sheet-open", open);
    fab.classList.toggle("open", open);
    clearTimeout(roundTimer);
    if (open) {
      fab.classList.remove("idle", "round");
      clearTimeout(idleTimer);
      // Remember the user's position so close restores it.
      fab.dataset.preTop = fab.style.top || "";
      // Park the × above the sheet so it never covers the tool grid.
      var sheetH = document.getElementById("toolsheet").offsetHeight;
      fab.style.left = "auto";
      fab.style.right = "16px";
      fab.style.top = "auto";
      fab.style.bottom = (sheetH + 16) + "px";
    }
    else {
      var leftSide = fab.classList.contains("side-left");
      fab.style.left = leftSide ? "0px" : "auto";
      fab.style.right = leftSide ? "auto" : "0px";
      fab.style.top = fab.dataset.preTop || "";
      fab.style.bottom = "";
      armIdle();
    }
    var hint = document.getElementById("hint"); if (hint) hint.style.display = "none";
  }
  var drag = null, idleTimer = null, roundTimer = null;
  function popRound() {
    var r = fab.getBoundingClientRect();
    var leftSide = fab.classList.contains("side-left");
    fab.style.top = r.top + "px";
    fab.style.left = leftSide ? "14px" : "auto";
    fab.style.right = leftSide ? "auto" : "14px";
    fab.classList.add("round"); fab.classList.remove("idle");
    clearTimeout(idleTimer); clearTimeout(roundTimer);
    roundTimer = setTimeout(collapseRound, 3500);
  }
  function collapseRound() {
    if (!fab.classList.contains("round") || body.classList.contains("sheet-open")) return;
    var leftSide = fab.classList.contains("side-left");
    fab.classList.remove("round");
    fab.style.left = leftSide ? "0px" : "auto";
    fab.style.right = leftSide ? "auto" : "0px";
    armIdle();
  }
  function armIdle() {
    fab.classList.remove("idle");
    clearTimeout(idleTimer);
    idleTimer = setTimeout(function () {
      if (!body.classList.contains("sheet-open")) fab.classList.add("idle");
    }, 3500);
  }
  fab.addEventListener("pointerdown", function (e) {
    drag = { x: e.clientX, y: e.clientY, moved: false, wasOpen: body.classList.contains("sheet-open") };
    if (drag.wasOpen) return;
    try { fab.setPointerCapture(e.pointerId); } catch (err) {}
    fab.classList.remove("idle"); clearTimeout(idleTimer); clearTimeout(roundTimer);
    e.preventDefault();
  });
  fab.addEventListener("pointermove", function (e) {
    if (!drag || drag.wasOpen) return;
    if (!drag.moved && Math.hypot(e.clientX - drag.x, e.clientY - drag.y) > 7) {
      drag.moved = true; fab.classList.add("dragging");
      var r = fab.getBoundingClientRect();
      drag.offX = drag.x - r.left; drag.offY = drag.y - r.top;
      fab.style.width = "50px"; fab.style.height = "50px"; fab.style.borderRadius = "50%";
    }
    if (drag.moved) {
      drag.nx = Math.min(Math.max(e.clientX - drag.offX, 4), innerWidth - 54);
      drag.ny = Math.min(Math.max(e.clientY - drag.offY, 48), innerHeight - 140);
      fab.style.left = drag.nx + "px"; fab.style.top = drag.ny + "px"; fab.style.right = "auto";
    }
  });
  fab.addEventListener("pointerup", function () {
    if (!drag) return;
    var d = drag; drag = null;
    if (d.wasOpen) { toggleSheet(false); return; }
    fab.classList.remove("dragging");
    fab.style.width = ""; fab.style.height = ""; fab.style.borderRadius = "";
    if (!d.moved) {
      if (fab.classList.contains("round")) toggleSheet(true);
      else popRound();
      return;
    }
    fab.classList.remove("round");
    var leftSide = (d.nx + 25) < innerWidth / 2;
    var top = Math.min(Math.max(d.ny + 3, 56), innerHeight - 130);
    fab.style.left = leftSide ? "0px" : "auto";
    fab.style.right = leftSide ? "auto" : "0px";
    fab.style.top = top + "px";
    fab.classList.toggle("side-left", leftSide);
    armIdle();
  });
  armIdle();
  document.getElementById("backdrop").onclick = function () {
    // Ignore the synthetic click that follows the pointerup which opened
    // the sheet — otherwise the sheet flashes open then immediately closes.
    if (Date.now() - sheetOpenedAt < 400) return;
    toggleSheet(false);
  };
  img.addEventListener("pointerdown", function () {
    if (fab.classList.contains("round")) collapseRound();
  });

  // --- tools sheet actions ---
  document.getElementById("urlBar").onclick = function () {
    openPrompt("Open URL", "https://", "https://example.com", "Open", function (u) {
      u = (u || "").trim();
      if (u) send({ type: "tab", action: "new", url: u });
    });
  };
  document.getElementById("reloadBtn").onclick = function () {
    send({ type: "tab", action: "reload" });
  };

  var desktopBtn = document.querySelector('[data-act="desktop"]');
  var desktopLabel = document.getElementById("desktopLabel");
  function setDesktop(on) {
    desktopBtn.classList.toggle("on", !!on);
    desktopLabel.textContent = on ? "Mobile" : "Desktop";
  }

  var zoomLevels = [100, 125, 150], zoomIdx = 0;
  var zoomLabel = document.getElementById("zoomLabel");
  function applyZoom() {
    var z = zoomLevels[zoomIdx];
    img.style.width = z + "%";
    img.style.height = z + "%";
    zoomLabel.textContent = z === 100 ? "Zoom" : z + "%";
  }

  document.querySelectorAll("[data-act]").forEach(function (el) {
    el.addEventListener("click", function () {
      var act = el.dataset.act;
      if (act === "back") send({ type: "history", action: "back" });
      else if (act === "forward") send({ type: "history", action: "forward" });
      else if (act === "keyboard") { kbd.focus(); toast("Type — keys go to the page"); }
      else if (act === "paste") {
        // Fetch the laptop clipboard into the prompt; editing +
        // Send pushes it back (then Ctrl+V in the page pastes it).
        send({ type: "clipboard", action: "get" });
      }
      else if (act === "desktop") {
        var on = !desktopBtn.classList.contains("on");
        send({ type: "emulation", mobile: on, width: 390, height: 844 });
      }
      else if (act === "zoom") {
        zoomIdx = (zoomIdx + 1) % zoomLevels.length;
        applyZoom();
      }
      else if (act === "find") {
        openPrompt("Find in page", "", "Search text…", "Find", function (q) {
          if ((q || "").trim()) send({ type: "find", text: q.trim() });
        });
      }
      else if (act === "chrome-stop") {
        if (confirm("Stop Chrome on the laptop?")) send({ type: "chrome", action: "close" });
      }
      else if (act === "disconnect") {
        // Clear pairing and reload to the pairing screen.
        try { localStorage.removeItem("br_fingerprint"); } catch (e) {}
        if (ws) ws.close();
        location.reload();
      }
      else if (act === "settings") toast("Settings — coming soon");
    });
  });

  // --- home (beranda): shortcut grid ---
  (function () {
    var grid = document.getElementById("homeGrid");
    var ICONS = {
      facebook: '<svg viewBox="0 0 24 24" fill="#fff"><path d="M13.5 21v-7h2.4l.4-3h-2.8V9.1c0-.9.3-1.5 1.6-1.5h1.3V4.9c-.3 0-1.2-.1-2.2-.1-2.2 0-3.7 1.3-3.7 3.8V11H8v3h2.5v7h3z"/></svg>',
      youtube: '<svg viewBox="0 0 24 24" fill="#fff"><path d="M21.6 7.2a2.6 2.6 0 0 0-1.8-1.9C18.2 5 12 5 12 5s-6.2 0-7.8.3A2.6 2.6 0 0 0 2.4 7.2 27 27 0 0 0 2 12a27 27 0 0 0 .4 4.8 2.6 2.6 0 0 0 1.8 1.9c1.6.3 7.8.3 7.8.3s6.2 0 7.8-.3a2.6 2.6 0 0 0 1.8-1.9A27 27 0 0 0 22 12a27 27 0 0 0-.4-4.8zM10 15V9l5.2 3L10 15z"/></svg>',
      google: '<svg viewBox="0 0 24 24"><path fill="#fff" d="M21.35 11.1H12v2.9h5.35c-.5 2.4-2.7 3.6-5.35 3.6a5.9 5.9 0 1 1 0-11.8c1.5 0 2.9.6 3.9 1.5l2.1-2.1A8.9 8.9 0 1 0 12 20.9c4.4 0 8.1-3.1 8.1-8.9 0-.3 0-.6-.1-.9z"/></svg>',
      x: '<svg viewBox="0 0 24 24" fill="#fff"><path d="M17.7 3H21l-7.3 8.3L22.2 21h-6.7l-5.2-6.2L4.3 21H1l7.8-8.9L1.8 3h6.9l4.7 5.7L17.7 3zm-1.2 16h1.9L7.1 4.9H5L16.5 19z"/></svg>',
      instagram: '<svg viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2"><rect x="3" y="3" width="18" height="18" rx="5"/><circle cx="12" cy="12" r="4"/><circle cx="17.2" cy="6.8" r="1.2" fill="#fff" stroke="none"/></svg>',
      tiktok: '<svg viewBox="0 0 24 24" fill="#fff"><path d="M16.6 3c.4 2.3 1.9 3.7 4.4 3.9v3c-1.7 0-3.2-.5-4.4-1.4v6.6a6.1 6.1 0 1 1-6.1-6.1c.3 0 .7 0 1 .1v3.1a3 3 0 1 0 2 2.9V3h3.1z"/></svg>',
      github: '<svg viewBox="0 0 24 24" fill="#fff"><path d="M12 2a10 10 0 0 0-3.2 19.5c.5.1.7-.2.7-.5v-1.7c-2.8.6-3.4-1.2-3.4-1.2-.5-1.2-1.1-1.5-1.1-1.5-.9-.6.1-.6.1-.6 1 .1 1.5 1 1.5 1 .9 1.6 2.4 1.1 3 .9.1-.7.4-1.1.6-1.4-2.2-.3-4.6-1.1-4.6-5 0-1.1.4-2 1-2.7-.1-.2-.4-1.3.1-2.6 0 0 .8-.3 2.7 1a9.4 9.4 0 0 1 5 0c1.9-1.3 2.7-1 2.7-1 .5 1.3.2 2.4.1 2.6.6.7 1 1.6 1 2.7 0 3.9-2.4 4.7-4.6 5 .4.3.7.9.7 1.9v2.8c0 .3.2.6.7.5A10 10 0 0 0 12 2z"/></svg>',
      wikipedia: '<svg viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3 3.5 3 14 0 18M12 3c-3 3.5-3 14 0 18"/></svg>'
    };
    var COLORS = {
      facebook: "#1877f2", youtube: "#ff0000", google: "#1a73e8", x: "#000000",
      instagram: "linear-gradient(135deg,#f9ce34,#ee2a7b,#6228d7)", tiktok: "#111111",
      github: "#24292f", wikipedia: "#636363"
    };
    var DEFAULTS = [
      { name: "Facebook", url: "facebook.com", icon: "facebook" },
      { name: "YouTube", url: "youtube.com", icon: "youtube" },
      { name: "Google", url: "google.com", icon: "google" },
      { name: "X", url: "x.com", icon: "x" },
      { name: "Instagram", url: "instagram.com", icon: "instagram" },
      { name: "TikTok", url: "tiktok.com", icon: "tiktok" },
      { name: "GitHub", url: "github.com", icon: "github" },
      { name: "Wikipedia", url: "wikipedia.org", icon: "wikipedia" }
    ];
    var shortcuts;
    try { shortcuts = JSON.parse(localStorage.getItem("br_home") || "null") || DEFAULTS.slice(); }
    catch (e) { shortcuts = DEFAULTS.slice(); }
    function save() { try { localStorage.setItem("br_home", JSON.stringify(shortcuts)); } catch (e) {} }
    function esc(s) { return String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;"); }
    function iconFor(s) {
      var key = s.icon && ICONS[s.icon] ? s.icon : null;
      var glyph = key ? ICONS[key] :
        '<span style="color:#fff;font-weight:700;font-size:24px">' + esc((s.name || "?").charAt(0).toUpperCase()) + "</span>";
      return '<div class="icon" style="background:' + (key ? COLORS[key] : "#8b72f0") + '">' + glyph + "</div>";
    }
    function openUrl(url) {
      url = (url || "").trim();
      if (!url) return;
      send({ type: "tab", action: "new", url: url });
      closeHome();
      toggleSheet(false);
    }
    window.__brOpenHome = openHome;
    function openHome() { document.body.classList.add("home-open"); }
    function closeHome() {
      document.body.classList.remove("home-open", "home-editing");
      document.getElementById("homeEdit").classList.remove("on");
      document.getElementById("homeEdit").textContent = "Edit";
    }
    function render() {
      grid.innerHTML = "";
      shortcuts.forEach(function (s, i) {
        var b = document.createElement("button");
        b.className = "tile";
        b.innerHTML = iconFor(s) + '<span class="label">' + esc(s.name) + '</span><span class="del">×</span>';
        b.title = s.url;
        b.querySelector(".del").onclick = function (e) {
          e.stopPropagation();
          shortcuts.splice(i, 1); save(); render();
          toast("Shortcut dihapus");
        };
        b.onclick = function () {
          if (document.body.classList.contains("home-editing")) return;
          openUrl(s.url);
        };
        grid.appendChild(b);
      });
      var add = document.createElement("button");
      add.className = "tile add";
      add.innerHTML = '<div class="icon">+</div><span class="label">Tambah</span>';
      add.onclick = function () {
        document.getElementById("homeDlgName").value = "";
        document.getElementById("homeDlgUrl").value = "";
        document.getElementById("homeDlg").classList.add("open");
        setTimeout(function () { document.getElementById("homeDlgName").focus(); }, 60);
      };
      grid.appendChild(add);
    }
    document.getElementById("homeDlgCancel").onclick = function () {
      document.getElementById("homeDlg").classList.remove("open");
    };
    document.getElementById("homeDlgOk").onclick = function () {
      var name = document.getElementById("homeDlgName").value.trim();
      var url = document.getElementById("homeDlgUrl").value.trim().replace(/^https?:\/\//, "").replace(/\/$/, "");
      if (!name) { toast("Isi namanya dulu"); return; }
      if (!url || url.indexOf(".") < 0) { toast("URL-nya belum valid"); return; }
      shortcuts.push({ name: name, url: url });
      save(); render();
      document.getElementById("homeDlg").classList.remove("open");
      toast("Shortcut tersimpan");
    };
    document.getElementById("homeEdit").onclick = function () {
      var on = document.body.classList.toggle("home-editing");
      this.classList.toggle("on", on);
      this.textContent = on ? "Selesai" : "Edit";
    };
    document.getElementById("homeSearch").onclick = function () {
      openPrompt("Buka URL", "https://", "https://example.com", "Buka", openUrl);
    };
    document.getElementById("homeBtn").onclick = function () {
      toggleSheet(false);
      openHome();
    };
    document.getElementById("homeBack").onclick = function () {
      closeHome();
    };
    render();
  })();

  // --- auth ---
  // NOTE: the "go" button handler is defined once at the top of this file
  // (goBtn.onclick, ~line 70) which sends auth+token over the live WS.
  // The old static-token handler below was removed in v0.9.17 — it used to
  // overwrite goBtn.onclick with a localStorage+reconnect flow that never
  // sent the pairing code to the server.
  tokenInput.addEventListener("keydown", function (e) {
    if (e.key === "Enter") document.getElementById("go").click();
  });

  try {
    var qp = new URLSearchParams(location.search).get("code");
    if (qp && /^\d{6}$/.test(qp)) {
      // QR scan: single-use code — auth immediately with it.
      token = qp;
      try { localStorage.setItem("br_token", token); } catch (e) {}
      history.replaceState(null, "", location.pathname);
      // Defer until ws is open; connect() sends fingerprint-only first,
      // so stash the QR code for the onopen handler to prefer.
      qrCode = qp;
    }
  } catch (e) {}

  // NOTE: br_token is legacy from the old static-token flow (v0.9.16 and
  // earlier buggy handler saved pairing codes here). Do NOT hide the auth
  // box based on it — the fingerprint flow shows pairing UI on demand.
  // Clear the stale value so it doesn't confuse future logic.
  try { localStorage.removeItem("br_token"); } catch (e) {}
  // Always connect — the server tells us if we're unknown (show pairing UI)
  // or welcomes us back (fingerprint reconnect).
  connect();
  setTimeout(function () { var h = document.getElementById("hint"); if (h) h.style.opacity = "0"; }, 8000);
})();
