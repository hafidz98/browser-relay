// browser-relay web client: fullscreen viewport + edge-handle tools,
// same design language as the Android mock (adapted for desktop).
(function () {
  "use strict";
  var img = document.getElementById("screen");
  var status = document.getElementById("status");
  var peer = document.getElementById("peer");
  var latencyEl = document.getElementById("latency");
  var statusText = document.getElementById("statusText");
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
// A QR scan means the user wants the screen, not the home screen.
var qrScanned = false;

  var authDesc = document.getElementById("authDesc");
  var reqBtn = document.getElementById("reqCode");
  var goBtn = document.getElementById("go");

  function showRequestButton() {
    authBox.style.display = "flex";
    var lbl = document.getElementById("tokenLabel");
    if (lbl) lbl.style.display = "none";
    authDesc.innerHTML = "This device isn't paired yet.<br>Request a code from the PC panel.";
    tokenInput.style.display = "none";
    goBtn.style.display = "none";
    reqBtn.style.display = "block";
    authErr.textContent = "";
  }

  function showPairInput(hint) {
    authBox.style.display = "flex";
    var lbl = document.getElementById("tokenLabel");
    if (lbl) lbl.style.display = "block";
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
    showPairInput("Type the 6-digit code from the PC panel.");
  }

  reqBtn.onclick = function () {
    if (ws && ws.readyState === 1) {
      ws.send(JSON.stringify({ type: "pair_request", fingerprint: fingerprint, name: deviceName }));
      authDesc.textContent = "Requesting code…";
      reqBtn.style.display = "none";
    }
  };

  goBtn.onclick = function () {
    var code = tokenInput.value.trim();
    if (!/^\d{6}$/.test(code)) {
      authErr.textContent = "The code must be 6 digits";
      return;
    }
    if (ws && ws.readyState === 1) {
      goBtn.disabled = true;
      goBtn.textContent = "Connecting…";
      ws.send(JSON.stringify({ type: "auth", token: code, fingerprint: fingerprint, name: deviceName }));
      authDesc.textContent = "Connecting…";
      authErr.textContent = "";
    } else {
      authErr.textContent = "Connection lost — waiting a moment, then retrying";
    }
  };

  var VK = { Enter: 13, Backspace: 8, Tab: 9, Escape: 27, ArrowLeft: 37,
    ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, Delete: 46, Home: 36,
    End: 35, PageUp: 33, PageDown: 34 };

  var roleLabel = "";
  var peerHost = "";
  function updateStatusText() {
    var t = roleLabel || "connecting…";
    if (peerHost) t += " · " + peerHost;
    statusText.textContent = t;
  }
  function setStatus(s, cls) {
    roleLabel = s || "";
    updateStatusText();
    status.className = cls || "";
  }

  // ---- views: the home screen is the entry point, the remote screen is
  // one tap away from there ----
  function openHome() {
    document.body.classList.add("home-open");
    showBrowserDown(!browserUp);
  }
  function closeHome() {
    document.body.classList.remove("home-open", "home-editing");
    showBrowserDown(!browserUp);
    var edit = document.getElementById("homeEdit");
    if (edit) { edit.classList.remove("on"); edit.textContent = "Edit"; }
  }
  // The connection state lives in one place: the steps in the home
  // card, the status pill and the session buttons all follow it.
  function setConnected(on) {
    var dis = document.querySelector('[data-act="disconnect"]');
    if (dis) dis.disabled = !on;
  }

  // The three dots in the home screen's connection card.
  function updateSteps() {
    var set = function (id, on) {
      var el = document.getElementById(id);
      if (el) el.classList.toggle("on", !!on);
    };
    var serverUp = ws && ws.readyState === 1;
    // "Ready" means the phone can really drive Chrome: the socket is
    // up, this device is paired and Chrome is running. A green Ready
    // while Chrome is down would be a lie.
    var ready = authed && browserUp;
    set("stServer", serverUp);
    set("stChrome", browserUp);
    set("stReady", ready);
    // One action at a time: the remote view only opens when it works,
    // otherwise the only useful thing is starting Chrome.
    var open = document.getElementById("openRemote");
    if (open) open.style.display = ready ? "block" : "none";
    var launch = document.getElementById("launchChrome");
    if (launch) launch.style.display = (authed && !browserUp && isDriver) ? "block" : "none";
    // Say why the shortcuts are dimmed, instead of leaving the user
    // to guess.
    var hint = document.getElementById("cardHint");
    if (hint) {
      var showHint = authed && !browserUp;
      hint.style.display = showHint ? "block" : "none";
      if (showHint) {
        hint.textContent = isDriver
          ? "Chrome is not running on the PC. Start it to drive the browser."
          : "Chrome is not running on the PC. The driver has to start it first.";
      }
    }
    setConnected(authed);
    fillPcCard();
  }

  // ---- the tools rail can be folded away (desktop) ----
  (function () {
    var key = "br_rail";
    var set = function (collapsed) {
      document.body.classList.toggle("rail-collapsed", !!collapsed);
      try { localStorage.setItem(key, collapsed ? "1" : "0"); } catch (e) {}
    };
    var saved = null;
    try { saved = localStorage.getItem(key); } catch (e) {}
    if (saved === "1") document.body.classList.add("rail-collapsed");
    var hide = document.getElementById("railToggle");
    var show = document.getElementById("railOpen");
    var after = function () {
      // The view got wider: tell the PC, then place the frame.
      setTimeout(function () {
        if (typeof sendViewport === "function") sendViewport();
        if (typeof fitFrame === "function") fitFrame();
      }, 60);
    };
    if (hide) hide.onclick = function () { set(true); after(); };
    if (show) show.onclick = function () { set(false); after(); };
  })();

  // ---- confirm dialog (the browser's confirm() does not belong here) ----
  var confirmDlg = document.getElementById("confirmDlg");
  var confirmTitle = document.getElementById("confirmTitle");
  var confirmText = document.getElementById("confirmText");
  var confirmOk = document.getElementById("confirmOk");
  var confirmCancel = document.getElementById("confirmCancel");
  var confirmCb = null;
  function askConfirm(title, text, okText, cb) {
    if (!confirmDlg) { cb(); return; }
    confirmTitle.textContent = title;
    confirmText.textContent = text || "";
    confirmOk.textContent = okText || "OK";
    confirmCb = cb;
    confirmDlg.classList.add("open");
  }
  if (confirmOk) confirmOk.onclick = function () {
    var cb = confirmCb; confirmCb = null;
    confirmDlg.classList.remove("open");
    if (cb) cb(true);
  };
  if (confirmCancel) confirmCancel.onclick = function () {
    confirmCb = null; confirmDlg.classList.remove("open");
  };
  if (confirmDlg) confirmDlg.addEventListener("click", function (e) {
    if (e.target === confirmDlg) { confirmCb = null; confirmDlg.classList.remove("open"); }
  });

  // ---- touch has no hover: a long press names the icon ----
  function bindLongPress(el) {
    if (!el) return;
    var label = el.getAttribute("title") || el.getAttribute("aria-label") || "";
    if (!label) return;
    var timer = null;
    var start = function () {
      clearTimeout(timer);
      timer = setTimeout(function () { toast(label); }, 550);
    };
    var stop = function () { clearTimeout(timer); };
    el.addEventListener("pointerdown", start);
    el.addEventListener("pointerup", stop);
    el.addEventListener("pointercancel", stop);
    el.addEventListener("pointerleave", stop);
  }
  ["copyBtn", "reloadBtn", "homeBtn"].forEach(function (id) {
    bindLongPress(document.getElementById(id));
  });

  // ---- the rail scrolls: fade the bottom while there is more ----
  var railBody = document.querySelector(".rail-body");
  if (railBody) {
    var railMore = function () {
      var more = railBody.scrollTop + railBody.clientHeight < railBody.scrollHeight - 2;
      railBody.classList.toggle("more", more);
    };
    railBody.addEventListener("scroll", railMore);
    window.addEventListener("resize", railMore);
    setTimeout(railMore, 0);
  }

  // ---- this PC (host + address) ----
  var pcHost = document.getElementById("pcHost");
  var pcUrl = document.getElementById("pcUrl");
  var pcCard = document.getElementById("pcCard");
  var pcHostName = "";
  function fillPcCard() {
    if (!pcCard) return;
    if (!authed) { pcCard.style.display = "none"; return; }
    pcCard.style.display = "block";
    if (pcHost) pcHost.textContent = pcHostName || "-";
    if (pcUrl) pcUrl.textContent = location.origin;
  }
  var pcCopy = document.getElementById("pcCopy");
  if (pcCopy) pcCopy.onclick = function () {
    var text = location.origin;
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(
        function () { toast("Address copied"); },
        function () { toast("Could not copy the address"); });
    } else {
      toast("Address: " + text);
    }
  };

  // ---- theme ----
  function currentTheme() { return document.documentElement.getAttribute("data-theme") || "dark"; }
  function setTheme(theme) {
    document.documentElement.setAttribute("data-theme", theme);
    try { localStorage.setItem("br_theme", theme); } catch (e) {}
    var btn = document.getElementById("themeToggle");
    if (btn) {
      btn.classList.toggle("on", theme === "light");
      var span = btn.querySelector("span");
      if (span) span.textContent = theme === "light" ? "Dark theme" : "Light theme";
    }
  }
  function toggleTheme() { setTheme(currentTheme() === "light" ? "dark" : "light"); }

  // ---- clean view: hide the client chrome for a bare remote screen ----
  function setClean(on) {
    document.body.classList.toggle("clean", !!on);
    var btn = document.getElementById("cleanToggle");
    if (btn) btn.classList.toggle("on", !!on);
  }

  var browserDownEl = document.getElementById("browserDown");
  var browserUp = false;
  // "Stop Chrome" only makes sense while Chrome is actually running.
  function setChromeRunning(up) {
    browserUp = !!up;
    document.body.classList.toggle("chrome-down", !browserUp);
    var b = document.querySelector('[data-act="chrome-stop"]');
    if (b) b.classList.toggle("off", !browserUp);
    updateSteps();
  }
  function showBrowserDown(down) {
    if (!browserDownEl) return;
    // The home screen owns the viewport, so the remote's empty state
    // must not float over it.
    var onHome = document.body.classList.contains("home-open");
    browserDownEl.style.display = (down && !onHome) ? "flex" : "none";
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
    latencyEl.className = "rail-ping " + (ms < 300 ? "" : ms < 900 ? "warn" : "bad");
    latencyEl.title = "Round-trip to the PC (not input latency)";
  }

  function connect() {
    var proto = location.protocol === "https:" ? "wss:" : "ws:";
    ws = new WebSocket(proto + "//" + location.host + "/ws");
    ws.onopen = function () {
      updateSteps();
      if (tokenInput) tokenInput.disabled = false;
      if (goBtn) goBtn.disabled = false;
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
          setConnected(true);
          authBox.style.display = "none";
          authErr.textContent = "";
          goBtn.disabled = false;
          goBtn.textContent = "Connect";
          setStatus(isDriver ? "Driving Chrome" : "View only", isDriver ? "live" : "view");
          // Which PC are we driving? (the relay sends the host name)
          if (m.host) {
            pcHostName = m.host;
            peerHost = m.host;
            updateStatusText();
            peer.style.display = "flex";
          }
          if (m.browserUp !== undefined) { setChromeRunning(m.browserUp); showBrowserDown(!m.browserUp); }
          updateSteps();
          // A QR scan means "show me the screen" - go straight there.
          if (qrScanned) { qrScanned = false; closeHome(); }
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
          setChromeRunning(m.browserUp);
          showBrowserDown(!m.browserUp);
          break;
        case "pong":
          if (m.t) setLatency(Date.now() - m.t);
          break;
        case "role":
          isDriver = !!m.driver;
          setStatus(isDriver ? "Driving Chrome" : "View only", isDriver ? "live" : "view");
          if (isDriver) sendViewport();
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
          showPairInput("Check the PC panel and type the 6-digit code shown there.");
          break;
        case "error":
          if (!authed) {
            goBtn.disabled = false;
            goBtn.textContent = "Connect";
            if (m.message === "unknown device") {
              showPairingUI();
            } else {
              authDesc.textContent = "Pairing failed";
              authErr.textContent = m.message || "connection failed";
            }
          }
          else toast("error: " + m.message);
          break;
      }
    };
    ws.onclose = function () {
      authed = false;
      browserUp = false;
      setConnected(false);
      openHome();
      if (authDesc) authDesc.textContent = "Connection lost — reconnecting…";
      if (tokenInput) tokenInput.disabled = true;
      if (goBtn) goBtn.disabled = true;
      updateSteps();
      latencyMs.textContent = "—";
      latencyEl.className = "rail-ping pending";
      showBrowserDown(false);
      setStatus("disconnected — retrying…");
      if (!authed && authBox.style.display !== "none") {
        authDesc.textContent = "Connection lost - retrying…";
      }
      setTimeout(connect, 2000);
    };
  }

  function renderTabs(tabs) {
    tabsEl.innerHTML = "";
    var activeUrl = "—";
    // The same target can arrive twice (a tab that re-attached) - show
    // each tab once.
    var seen = {};
    var unique = [];
    tabs.forEach(function (t) {
      if (t && t.id && !seen[t.id]) { seen[t.id] = true; unique.push(t); }
    });
    tabs = unique;
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
    window.__activeUrl = activeUrl;
    if (activeUrl && activeUrl !== "—") {
      // The domain is what identifies the page; the path is secondary.
      var domain = activeUrl, rest = "";
      try {
        var u2 = new URL(activeUrl);
        domain = u2.host;
        rest = (u2.pathname === "/" ? "" : u2.pathname) + u2.search;
      } catch (e) { domain = activeUrl; }
      urlText.textContent = "";
      var dom = document.createElement("b");
      dom.textContent = domain;
      var path = document.createElement("span");
      path.className = "path";
      path.textContent = rest;
      urlText.appendChild(dom);
      urlText.appendChild(path);
      urlText.classList.remove("ph");
      var bar = document.getElementById("urlBar");
      bar.classList.add("live");
      bar.title = activeUrl;
    } else {
      urlText.textContent = "Search or enter address";
      urlText.classList.add("ph");
      var bar2 = document.getElementById("urlBar");
      bar2.classList.remove("live");
      bar2.title = "Open URL";
    }
  }

  // --- the remote view ---
  // The PC renders the page at the size of this view, so the frame can
  // be shown 1:1: no letterbox bars, no rescaling blur. Folding the
  // tool rail reflows the page instead of zooming it.
  function viewBox() {
    var rail = 0;
    if (window.innerWidth >= 900 && !document.body.classList.contains("rail-collapsed")) rail = 300;
    return {
      x: rail, y: 0,
      w: Math.max(1, window.innerWidth - rail),
      h: Math.max(1, window.innerHeight)
    };
  }
  function fitFrame() {
    var nw = img.naturalWidth, nh = img.naturalHeight;
    if (!nw || !nh) return;
    var b = viewBox();
    var z = zoomLevels[zoomIdx] / 100;
    var w = b.w * z;
    var h = w * nh / nw;
    img.style.width = w + "px";
    img.style.height = h + "px";
    img.style.left = (b.x + (b.w - w) / 2) + "px";
    img.style.top = (b.y + (b.h - h) / 2) + "px";
  }
  img.addEventListener("load", fitFrame);

  var lastViewport = "";
  function sendViewport() {
    if (!ws || ws.readyState !== 1 || !authed || !isDriver) return;
    var b = viewBox();
    var dpr = window.devicePixelRatio || 1;
    var key = Math.round(b.w) + "x" + Math.round(b.h) + "@" + dpr;
    if (key === lastViewport) return;
    lastViewport = key;
    ws.send(JSON.stringify({
      type: "viewport", width: Math.round(b.w), height: Math.round(b.h), dpr: dpr
    }));
  }
  window.addEventListener("resize", function () { fitFrame(); sendViewport(); });

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
  // The PC can only use one move at a time. Sending one per frame
  // builds a backlog that shows up as lag, so keep the newest position
  // and let it out at most every 30 ms.
  var moveQueued = null;
  var lastMove = null;
  var moveSentAt = 0;
  function sendMove(x, y, button) {
    var now = Date.now();
    var wait = 30 - (now - moveSentAt);
    if (wait <= 0) {
      moveSentAt = now;
      lastMove = null;
      send({ type: "mouse", action: "move", x: x, y: y, button: button });
    } else {
      lastMove = { x: x, y: y, button: button };
      setTimeout(function () {
        if (!lastMove) return;
        var m = lastMove; lastMove = null;
        moveSentAt = Date.now();
        send({ type: "mouse", action: "move", x: m.x, y: m.y, button: m.button });
      }, wait);
    }
  }
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
            send({ type: "wheel", x: p.x, y: p.y, dx: -dx * 0.55, dy: -dy * 0.55 });
            touchStart.x = e.clientX; touchStart.y = e.clientY;
            return;
          }
          // Not yet a scroll, don't send mouse move (avoid hover effects).
          return;
        }
        sendMove(p.x, p.y, btn(e));
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
    // One notch is a small step, not a scroll through the whole page.
    var k = e.deltaMode === 1 ? 33 : (e.deltaMode === 2 ? 400 : 1);
    var clamp = function (v) { return Math.max(-160, Math.min(160, v)) * 0.55; };
    send({ type: "wheel", x: p.x, y: p.y, dx: clamp(e.deltaX * k), dy: clamp(e.deltaY * k) });
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
    if (open) { sheetOpenedAt = Date.now(); body.classList.remove("sheet-tall"); }
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
      // One tap opens the tools. The old flow needed two taps (the
      // handle expanded into a round button first), which read as a
      // broken control.
      toggleSheet(true);
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
  document.getElementById("sheetClose").onclick = function () { toggleSheet(false); };
  // View controls
  var themeToggle = document.getElementById("themeToggle");
  if (themeToggle) themeToggle.onclick = function () { toggleTheme(); if (navigator.vibrate) { try { navigator.vibrate(8); } catch (e) {} } };
  var cleanToggle = document.getElementById("cleanToggle");
  if (cleanToggle) cleanToggle.onclick = function () { setClean(!document.body.classList.contains("clean")); };
  var cleanExit = document.getElementById("cleanExit");
  if (cleanExit) cleanExit.onclick = function () { setClean(false); };
  var reconnectBtn = document.getElementById("reconnectBtn");
  if (reconnectBtn) reconnectBtn.onclick = function () {
    toast("Reconnecting…");
    try { if (ws) ws.close(); } catch (e) {}
    setTimeout(connect, 120);
  };
  var shortcutsBtn = document.getElementById("shortcutsBtn");
  var shortcutsDlg = document.getElementById("shortcuts");
  if (shortcutsBtn && shortcutsDlg) {
    shortcutsBtn.onclick = function () { shortcutsDlg.classList.add("open"); };
    shortcutsDlg.onclick = function (e) { if (e.target === shortcutsDlg) shortcutsDlg.classList.remove("open"); };
  }
  var openRemote = document.getElementById("openRemote");
  if (openRemote) openRemote.onclick = function () { closeHome(); };
  var launchCard = document.getElementById("launchChrome");
  if (launchCard) launchCard.onclick = function () {
    send({ type: "chrome", action: "launch" });
    toast("Launching Chrome on the PC");
  };

  // Keyboard shortcuts (desktop): they mirror the tool buttons.
  document.addEventListener("keydown", function (e) {
    var tag = (e.target && e.target.tagName) || "";
    if (tag === "INPUT" || tag === "TEXTAREA" || (e.target && e.target.isContentEditable)) return;
    if (e.metaKey || e.ctrlKey || e.altKey) {
      if ((e.metaKey || e.ctrlKey) && (e.key === "c" || e.key === "C")) {
        // Copy the page link (we own the button, so copy it ourselves).
        var link = window.__activeUrl || "";
        if (link && link !== "—" && navigator.clipboard && navigator.clipboard.writeText) {
          navigator.clipboard.writeText(link).then(function () { toast("Link copied"); },
            function () { toast("Could not copy the link"); });
          e.preventDefault();
        }
      }
      return;
    }
    switch (e.key) {
      case "Escape":
        if (shortcutsDlg && shortcutsDlg.classList.contains("open")) { shortcutsDlg.classList.remove("open"); e.preventDefault(); }
        else if (document.body.classList.contains("home-open")) { closeHome(); e.preventDefault(); }
        else if (document.body.classList.contains("sheet-open")) { toggleSheet(false); e.preventDefault(); }
        break;
      case "ArrowLeft": doAction("back"); e.preventDefault(); break;
      case "ArrowRight": doAction("forward"); e.preventDefault(); break;
      case "r": doAction("reload"); break;
      case "f": doAction("find"); break;
      case "k": doAction("keyboard"); break;
      case "p": doAction("paste"); break;
      case "m": doAction("desktop"); break;
      case "l": setClean(!document.body.classList.contains("clean")); break;
      case "t": toggleTheme(); break;
      case "?": if (shortcutsDlg) { shortcutsDlg.classList.toggle("open"); e.preventDefault(); } break;
    }
  });
  // Drag the grip down to dismiss - the grip looks draggable, so it
  // should be.
  (function () {
    var grip = document.querySelector("#toolsheet .grip");
    if (!grip) return;
    var startY = 0, dragging = false;
    grip.addEventListener("pointerdown", function (e) {
      startY = e.clientY; dragging = true;
      try { grip.setPointerCapture(e.pointerId); } catch (err) {}
    });
    grip.addEventListener("pointermove", function (e) {
      if (!dragging) return;
      var dy = e.clientY - startY;
      if (dy > 70) { dragging = false; document.body.classList.remove("sheet-tall"); toggleSheet(false); }
      else if (dy < -70) { dragging = false; document.body.classList.add("sheet-tall"); }
    });
    grip.addEventListener("pointerup", function () { dragging = false; });
    grip.addEventListener("pointercancel", function () { dragging = false; });
  })();
  document.getElementById("backdrop").onclick = function () {
    // Ignore the synthetic click that follows the pointerup which opened
    // the sheet — otherwise the sheet flashes open then immediately closes.
    if (Date.now() - sheetOpenedAt < 400) return;
    toggleSheet(false);
  };
  // Tapping the remote page no longer hides the tool handle: that
  // gesture belongs to the page, not to the client chrome.

  // --- tools sheet actions ---
  // (The "+" button in the tab strip is built in renderTabs and
  // already opens a new tab with a URL.)
  document.getElementById("urlBar").onclick = function () {
    // Pre-fill with the address of the page that is open now, so the
    // full URL is readable and copyable.
    openPrompt("Open URL", window.__activeUrl || "https://", "https://example.com", "Open", function (u) {
      u = (u || "").trim();
      if (u) send({ type: "tab", action: "new", url: u });
    });
  };
  document.getElementById("reloadBtn").onclick = function () {
    send({ type: "tab", action: "reload" });
  };
  document.getElementById("copyBtn").onclick = function () {
    var link = window.__activeUrl || "";
    if (!link || link === "—") { toast("No page to copy yet"); return; }
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(link).then(
        function () { toast("Link copied"); },
        function () { toast("Could not copy the link"); }
      );
    } else {
      toast("Clipboard not available here");
    }
  };

  var desktopBtn = document.querySelector('[data-act="desktop"]');
  var desktopLabel = document.getElementById("desktopLabel");
  function setDesktop(on) {
    desktopBtn.classList.toggle("on", !!on);
    // Say which view you are in (the button switches to the other one).
    desktopLabel.textContent = on ? "Mobile view" : "Desktop view";
  }

  var zoomLevels = [100, 125, 150], zoomIdx = 0;
  var zoomLabel = document.getElementById("zoomLabel");
  function applyZoom() {
    var z = zoomLevels[zoomIdx];
    fitFrame();
    zoomLabel.textContent = z === 100 ? "Zoom" : z + "%";
  }

  function doAction(act) {
    if (navigator.vibrate) { try { navigator.vibrate(8); } catch (e) {} }
    if (act === "back") send({ type: "history", action: "back" });
    else if (act === "reload") send({ type: "tab", action: "reload" });
    else if (act === "forward") send({ type: "history", action: "forward" });
    else if (act === "keyboard") { kbd.focus(); toast("Type — keys go to the page"); }
    else if (act === "paste") {
      // Fetch the laptop clipboard into the prompt; editing +
      // Send pushes it back (then Ctrl+V in the page pastes it).
      send({ type: "clipboard", action: "get" });
    }
    else if (act === "sendclip") {
      // The other direction: put the phone clipboard on the laptop.
      if (!navigator.clipboard || !navigator.clipboard.readText) {
        toast("Clipboard not available on this browser"); return;
      }
      navigator.clipboard.readText().then(function (txt) {
        if (!txt) { toast("Phone clipboard is empty"); return; }
        send({ type: "clipboard", action: "set", text: txt });
        toast("Sent to the laptop clipboard");
      }, function () { toast("Clipboard permission denied"); });
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
      if (!browserUp) { toast("Chrome is not running"); return; }
      askConfirm("Stop Chrome on the laptop?",
        "The browser closes and the relay waits for Chrome to start again.",
        "Stop Chrome", function () { send({ type: "chrome", action: "close" }); });
    }
    else if (act === "disconnect") {
      // Clear pairing and reload to the pairing screen. This forgets
      // the device on the PC as well, so ask first.
      askConfirm("Disconnect this device?",
        "This device is forgotten by the relay and has to pair again.",
        "Disconnect", function () {
          try { localStorage.removeItem("br_fingerprint"); } catch (e) {}
          if (ws) ws.close();
          location.reload();
        });
    }
  }
  document.querySelectorAll("[data-act]").forEach(function (el) {
    el.addEventListener("click", function () { doAction(el.dataset.act); });
  });

  // --- home (home screen): shortcut grid ---
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
          toast("Shortcut removed");
        };
        b.onclick = function () {
          if (document.body.classList.contains("home-editing")) return;
          openUrl(s.url);
        };
        grid.appendChild(b);
      });
      var add = document.createElement("button");
      add.className = "tile add";
      add.innerHTML = '<div class="icon">+</div><span class="label">Add</span>';
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
      if (!name) { toast("Enter a name first"); return; }
      if (!url || url.indexOf(".") < 0) { toast("Invalid URL"); return; }
      shortcuts.push({ name: name, url: url });
      save(); render();
      document.getElementById("homeDlg").classList.remove("open");
      toast("Shortcut saved");
    };
    document.getElementById("homeEdit").onclick = function () {
      var on = document.body.classList.toggle("home-editing");
      this.classList.toggle("on", on);
      this.textContent = on ? "Done" : "Edit";
    };
    document.getElementById("homeSearch").onclick = function () {
      openPrompt("Open URL", "https://", "https://example.com", "Open", openUrl);
    };
    document.getElementById("homeBtn").onclick = function () {
      toggleSheet(false);
      openHome();
    };
    // (No back button in the home screen header: the home screen is the
    // entry point. The tools sheet has a Home button to return here.)
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
      qrScanned = true;
    }
  } catch (e) {}

  // NOTE: br_token is legacy from the old static-token flow (v0.9.16 and
  // earlier buggy handler saved pairing codes here). Do NOT hide the auth
  // box based on it — the fingerprint flow shows pairing UI on demand.
  // Clear the stale value so it doesn't confuse future logic.
  try { localStorage.removeItem("br_token"); } catch (e) {}
  // The home screen is the entry point: the remote screen is one tap away.
  openHome();
  setTheme(currentTheme());
  updateSteps();
  // Always connect — the server tells us if we're unknown (show pairing UI)
  // or welcomes us back (fingerprint reconnect).
  connect();
  setTimeout(function () { var h = document.getElementById("hint"); if (h) h.style.opacity = "0"; }, 8000);
})();
