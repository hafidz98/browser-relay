package com.hafidz.browserrelay;

import android.os.Handler;
import android.os.Looper;
import android.util.Base64;
import android.util.Log;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.IOException;
import java.util.ArrayList;
import java.util.List;

/**
 * browser-relay wire protocol over WsClient. Mirrors
 * shell/assets/client/app.js message for message:
 * <ul>
 *   <li>client -&gt; relay: auth, ping, mouse, wheel, key, tab,
 *       emulation, viewport, clipboard, takeover</li>
 *   <li>relay -&gt; client: welcome, status, paused, pong, role, frame,
 *       tabs, clipboard, emulation, error</li>
 * </ul>
 * Input sends are gated on authed &amp;&amp; driver, like the web
 * client's send() — one driver at a time, viewers are read-only.
 * "takeover" is the one exception: the server lets a viewer take the
 * wheel, so it bypasses that gate.
 *
 * <p>This class owns the connection. It keeps exactly one socket
 * alive, ignores every callback that comes from a socket it already
 * replaced (connection generations) and is the only place that
 * reconnects.
 */
public class RelayClient {
    private static final String TAG = "RelayClient";
    private static final long PING_MS = 2000;
    private static final int MAX_BACKOFF_MS = 10000;
    /** No message from the PC for this long means the link is gone. */
    private static final long DEAD_AFTER_MS = 10000;
    /** Read timeout of an authenticated socket (the PC pings back). */
    private static final int READ_TIMEOUT_MS = 20000;
    /**
     * How many times the client may reconnect on its own before it
     * stops and waits for the user. A phone that retries forever
     * against a wrong address drains the battery and hides the reason.
     */
    private static final int MAX_AUTO_RETRIES = 5;

    public static class TabInfo {
        public String id;
        public String title;
        public String url;
        public boolean active;
    }

    public interface Listener {
        /**
         * @param host the name of the PC we are talking to (the relay
         *             sends it with the welcome), or "" when it did
         *             not say.
         */
        void onWelcome(boolean driver, boolean browserUp, boolean mobile,
                       String host);
        void onStatus(boolean browserUp);
        /** The page is in the background: the picture is frozen. */
        void onPaused(boolean paused);
        /** Chrome runs without the flags that keep it painting. */
        void onFlagsOk(boolean flagsOk);
        void onLatency(long ms);
        void onRole(boolean driver);
        void onFrame(byte[] jpeg);
        void onTabs(List<TabInfo> tabs);
        void onClipboard(String text);
        void onEmulation(boolean mobile);
        void onFind(int count);
        void onChrome(String action);
        void onAuthError(String msg);
        /** An error the relay sent after we were authenticated. */
        void onServerError(String msg);
        /** The pairing code was refused: drop it, use the fingerprint. */
        void onBadToken();
        void onDisconnected();
        /**
         * The connection failed.
         *
         * @param kind one of the {@link WsClient.Failure} constants.
         */
        void onConnError(String msg, int kind);
        /**
         * The automatic reconnect gave up. The UI shows a Reconnect
         * button instead of retrying forever in the background.
         */
        void onRetriesExhausted();
    }

    private final Handler main = new Handler(Looper.getMainLooper());
    private final Listener listener;
    private WsClient ws;
    /** Connection generation: callbacks of a replaced socket are ignored. */
    private int gen = 0;
    private String wsUrl = "";
    /** The pairing code, sent with the first auth only. */
    private String token = "";
    /** The pairing code we already spent on an auth attempt. */
    private String usedToken = "";
    private String fingerprint = "";
    private String deviceName = "";
    private boolean authed;
    private boolean driver;
    private boolean wantConnect;
    private int backoffMs = 2000;
    /** How many automatic reconnects happened since the last success. */
    private int autoRetries = 0;
    /** When the last message from the PC arrived (watchdog). */
    private long lastRxAt = 0;
    /** The view size in CSS pixels, used by viewport/emulation. */
    private int viewW = 0;
    private int viewH = 0;
    private float viewDpr = 1f;
    private float viewZoom = 1f;
    /** The page layout this client wants: phone (true) or desktop. */
    private boolean emulMobile = true;
    /** The layout the PC was told last (so we do not send it twice). */
    private int sentW = -1;
    private int sentH = -1;
    private float sentDpr = -1f;
    private float sentZoom = -1f;
    private boolean sentMobile = false;

    private final Runnable pingTask = new Runnable() {
        @Override
        public void run() {
            if (!wantConnect || !authed || ws == null) return;
            long now = System.currentTimeMillis();
            // A dropped Wi-Fi link leaves the socket open but silent.
            // The PC answers every ping, so silence means the peer is
            // gone: drop the socket and let the reconnect take over.
            if (lastRxAt > 0 && now - lastRxAt > DEAD_AFTER_MS) {
                Log.w(TAG, "no answer from the PC, reconnecting");
                listener.onConnError("the PC stopped answering",
                        WsClient.Failure.LOST);
                WsClient dead = ws;
                ws = null;
                if (dead != null) dead.close();
                return;
            }
            try {
                ws.send(new JSONObject()
                        .put("type", "ping")
                        .put("t", now)
                        .toString());
            } catch (Exception e) {
                Log.w(TAG, "ping: " + e);
            }
            main.postDelayed(this, PING_MS);
        }
    };

    private final Runnable reconnectTask = new Runnable() {
        @Override
        public void run() {
            if (wantConnect) doConnect();
        }
    };

    public RelayClient(Listener listener) {
        this.listener = listener;
    }

    /**
     * Start (or restart) the connection.
     *
     * @param token the single-use pairing code, or "" when the device
     *              is already paired. It is sent with the first auth
     *              only — afterwards the fingerprint alone is the
     *              credential, exactly like the web client.
     */
    public void connect(String wsUrl, String token, String fingerprint, String name) {
        disconnect();
        this.wsUrl = wsUrl;
        this.token = token == null ? "" : token.trim();
        this.fingerprint = fingerprint;
        this.deviceName = name;
        wantConnect = true;
        backoffMs = 2000;
        autoRetries = 0;
        doConnect();
    }

    /** Connect right now, without waiting for the backoff timer. */
    public void reconnectNow() {
        if (wsUrl.isEmpty()) return;
        wantConnect = true;
        backoffMs = 2000;
        autoRetries = 0;
        main.removeCallbacks(reconnectTask);
        doConnect();
    }

    private void doConnect() {
        final int g = ++gen;
        // A new connection: the PC must be told our layout again.
        sentW = -1;
        // The socket we replace is closed right away: two live sockets
        // for one phone would fight over the driver role.
        WsClient old = ws;
        ws = null;
        if (old != null) old.close();
        authed = false;
        driver = false;
        main.removeCallbacks(pingTask);
        final WsClient client = new WsClient();
        ws = client;
        client.connect(wsUrl, new WsClient.Listener() {
            /**
             * The greeting (auth) frame. WsClient calls this on the
             * reader thread, synchronously, right after the 101 and
             * before the first read: the frame is on the wire before
             * the relay's auth deadline and cannot be lost to a main
             * looper that is busy or to a socket that was replaced in
             * the meantime. A failed write throws, so the connection
             * fails with a reason on screen instead of a silent wait.
             */
            @Override
            public void onReady(WsClient socket) throws java.io.IOException {
                if (g != gen) {
                    // A newer socket took over and already closed this
                    // one (the read loop sees `closed` and exits).
                    Log.i(TAG, "old socket, no auth sent");
                    return;
                }
                backoffMs = 2000;
                lastRxAt = System.currentTimeMillis();
                sendAuth(socket);
            }

            @Override
            public void onOpen() {
                if (g != gen) return; // a newer socket took over
                // The auth frame is already on the wire (onReady); this
                // callback is only for the UI and for the bookkeeping.
                Log.i(TAG, "connected to " + wsUrl + ", auth written");
            }

            @Override
            public void onMessage(String text) {
                if (g != gen) return; // a newer socket took over
                lastRxAt = System.currentTimeMillis();
                handle(text);
            }

            @Override
            public void onClose() {
                if (g != gen) return; // a newer socket took over
                authed = false;
                main.removeCallbacks(pingTask);
                if (ws == client) ws = null;
                listener.onDisconnected();
                // Reconnect on our own a few times, then stop and let the
                // user press Reconnect. An endless silent retry hides
                // the reason and burns the battery.
                if (wantConnect && autoRetries < MAX_AUTO_RETRIES) {
                    autoRetries++;
                    main.postDelayed(reconnectTask, backoffMs);
                    backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS);
                } else {
                    wantConnect = false;
                    listener.onRetriesExhausted();
                }
            }

            @Override
            public void onError(String msg, int kind) {
                if (g != gen) return; // a newer socket took over
                Log.w(TAG, "ws: " + msg);
                listener.onConnError(msg, kind);
            }
        });
    }

    /**
     * Build and write the auth frame — the one place the credential
     * rules live, called from the websocket reader thread before its
     * read loop starts.
     *
     * <p>A message from the relay's protocol, exactly like the web
     * client's:
     * <pre>
     *   {"type":"auth","fingerprint":"...","name":"...","token":"123456"}
     * </pre>
     * The pairing code is single use, so it goes out with the FIRST
     * auth only and is marked spent only after it actually reached the
     * wire: a message that failed to be written must not burn the code.
     * Every later connect (and every retry) authenticates with the
     * fingerprint alone.
     *
     * @throws IOException when the frame did not reach the wire; the
     *                     caller turns that into a visible error, never
     *                     into a silent wait for the relay's deadline
     */
    private void sendAuth(WsClient socket) throws IOException {
        boolean withCode = !token.isEmpty() && !token.equals(usedToken);
        final JSONObject auth = new JSONObject();
        try {
            auth.put("type", "auth")
                    .put("fingerprint", fingerprint == null ? "" : fingerprint)
                    .put("name", deviceName == null ? "" : deviceName);
            if (withCode) auth.put("token", token);
        } catch (Exception e) {
            throw new IOException("the auth message could not be built ("
                    + e.getMessage() + ")");
        }
        String json = auth.toString();
        socket.sendOrThrow(json);
        if (withCode) usedToken = token; // spent only once it is on the wire
        Log.i(TAG, "auth written (" + json.length() + " bytes, "
                + (withCode ? "with the pairing code" : "fingerprint only") + ")");
    }

    public void disconnect() {
        wantConnect = false;
        gen++; // ignore every callback of the socket we drop
        main.removeCallbacks(pingTask);
        main.removeCallbacks(reconnectTask);
        WsClient old = ws;
        ws = null;
        if (old != null) {
            old.close();
        }
        authed = false;
        driver = false;
    }

    public boolean isDriver() {
        return authed && driver;
    }

    /** True when the client is connected but only watching. */
    public boolean isViewOnly() {
        return authed && !driver;
    }

    private void handle(String text) {
        final JSONObject m;
        try {
            m = new JSONObject(text);
        } catch (Exception e) {
            return;
        }
        String type = m.optString("type", "");
        switch (type) {
            case "welcome":
                authed = true;
                driver = m.optBoolean("driver", false);
                // A working connection resets the retry budget.
                autoRetries = 0;
                backoffMs = 2000;
                main.removeCallbacks(pingTask);
                main.postDelayed(pingTask, PING_MS);
                // An authenticated socket must not stay silent forever:
                // the PC answers our pings, so a read timeout is safe.
                if (ws != null) ws.setReadTimeoutMs(READ_TIMEOUT_MS);
                listener.onWelcome(driver, m.optBoolean("browserUp", true),
                        m.optBoolean("mobile", false),
                        m.optString("host", ""));
                if (m.has("paused")) listener.onPaused(m.optBoolean("paused", false));
                if (m.has("flagsOk")) listener.onFlagsOk(m.optBoolean("flagsOk", true));
                // The page must be rendered for THIS device: tell the PC
                // our size and our layout every time we get the wheel.
                pushLayout();
                break;
            case "status":
                listener.onStatus(m.optBoolean("browserUp", true));
                if (m.has("paused")) listener.onPaused(m.optBoolean("paused", false));
                if (m.has("flagsOk")) listener.onFlagsOk(m.optBoolean("flagsOk", true));
                break;
            case "paused":
                listener.onPaused(m.optBoolean("paused", false));
                break;
            case "pong":
                listener.onLatency(System.currentTimeMillis() - m.optLong("t", 0));
                break;
            case "role":
                driver = m.optBoolean("driver", false);
                listener.onRole(driver);
                // We are driving now: the PC has to render the page for
                // this device, not for whoever drove before. Forget what
                // was sent last, so it is sent again.
                sentW = -1;
                pushLayout();
                break;
            case "frame": {
                String data = m.optString("data", "");
                if (!data.isEmpty()) {
                    try {
                        listener.onFrame(Base64.decode(data, Base64.DEFAULT));
                    } catch (Exception e) {
                        Log.w(TAG, "frame b64: " + e);
                    }
                }
                break;
            }
            case "tabs": {
                List<TabInfo> tabs = new ArrayList<>();
                JSONArray arr = m.optJSONArray("tabs");
                if (arr != null) {
                    for (int i = 0; i < arr.length(); i++) {
                        JSONObject t = arr.optJSONObject(i);
                        if (t == null) continue;
                        TabInfo ti = new TabInfo();
                        ti.id = t.optString("id", "");
                        ti.title = t.optString("title", "");
                        ti.url = t.optString("url", "");
                        ti.active = t.optBoolean("active", false);
                        tabs.add(ti);
                    }
                }
                listener.onTabs(tabs);
                break;
            }
            case "clipboard":
                if (m.has("text")) listener.onClipboard(m.optString("text", ""));
                break;
            case "emulation":
                emulMobile = m.optBoolean("mobile", false);
                sentMobile = emulMobile;
                listener.onEmulation(emulMobile);
                break;
            case "find":
                listener.onFind(m.optInt("count", 0));
                break;
            case "chrome":
                listener.onChrome(m.optString("action", ""));
                break;
            case "error": {
                String msg = m.optString("message", "");
                if (msg.isEmpty()) msg = m.optString("error", "error");
                if (!authed) {
                    if (isBadToken(msg)) {
                        // The pairing code is single use: it may already
                        // be spent. Drop it and retry with the fingerprint
                        // alone - the device may well be paired already.
                        Log.i(TAG, "pairing code refused, retrying with the fingerprint");
                        token = "";
                        usedToken = "";
                        listener.onBadToken();
                        reconnectNow();
                    } else {
                        if (msg.contains("unknown device")) {
                            // Not paired: retrying would only nag the PC.
                            // The pairing card asks for a code instead.
                            wantConnect = false;
                        }
                        listener.onAuthError(msg);
                    }
                } else {
                    // Authenticated, so this is about what we asked for
                    // (for example "view-only: another client is driving").
                    // "browser unavailable" is the PC's state, not our
                    // mistake: the overlay already says Chrome is down.
                    if (!"browser unavailable".equals(msg)) {
                        listener.onServerError(msg);
                    }
                }
                break;
            }
            default:
                break;
        }
    }

    /** The relay's answer to a refused pairing code. */
    private static boolean isBadToken(String msg) {
        String m = msg == null ? "" : msg.toLowerCase();
        return m.contains("bad token") || m.contains("invalid token")
                || m.contains("bad code") || m.contains("invalid code")
                || m.contains("expired code");
    }

    private void send(JSONObject o) {
        if (authed && driver && ws != null) {
            ws.send(o.toString());
        }
    }

    // ---- client -> relay ----

    /**
     * @param iw image width in pixels of the picture we show (the
     *           decoded frame), so the server can map the click onto
     *           the page exactly.
     */
    public void sendMouse(String action, double x, double y, String button,
                          int iw, int ih) {
        try {
            send(new JSONObject().put("type", "mouse").put("action", action)
                    .put("x", x).put("y", y).put("button", button)
                    .put("iw", iw).put("ih", ih));
        } catch (Exception e) {
            Log.w(TAG, "mouse: " + e);
        }
    }

    public void sendWheel(double x, double y, double dx, double dy,
                           int iw, int ih) {
        try {
            send(new JSONObject().put("type", "wheel")
                    .put("x", x).put("y", y).put("dx", dx).put("dy", dy)
                    .put("iw", iw).put("ih", ih));
        } catch (Exception e) {
            Log.w(TAG, "wheel: " + e);
        }
    }

    public void sendKeyText(String text) {
        if (text == null || text.isEmpty()) return;
        try {
            send(new JSONObject().put("type", "key").put("kind", "text").put("text", text));
        } catch (Exception e) {
            Log.w(TAG, "key text: " + e);
        }
    }

    /** vk = Windows virtual-key codes (13 Enter, 8 Backspace, ...). */
    public void sendKeyCode(int vk, boolean down) {
        try {
            send(new JSONObject().put("type", "key").put("kind", "code")
                    .put("vk", vk).put("down", down));
        } catch (Exception e) {
            Log.w(TAG, "key code: " + e);
        }
    }

    public void sendTabActivate(String targetId) {
        try {
            send(new JSONObject().put("type", "tab").put("action", "activate")
                    .put("targetId", targetId));
        } catch (Exception e) {
            Log.w(TAG, "tab activate: " + e);
        }
    }

    public void sendTabClose(String targetId) {
        try {
            send(new JSONObject().put("type", "tab").put("action", "close")
                    .put("targetId", targetId));
        } catch (Exception e) {
            Log.w(TAG, "tab close: " + e);
        }
    }

    public void sendTabNew(String url) {
        try {
            send(new JSONObject().put("type", "tab").put("action", "new").put("url", url));
        } catch (Exception e) {
            Log.w(TAG, "tab new: " + e);
        }
    }

    /** Ask for the driver role. The server accepts this from any client. */
    public void sendTakeover() {
        if (!authed || ws == null) return;
        try {
            // Not send(): the driver gate would swallow it.
            ws.send(new JSONObject().put("type", "takeover").toString());
        } catch (Exception e) {
            Log.w(TAG, "takeover: " + e);
        }
    }

    /**
     * The size of our view in CSS pixels (plus the device pixel ratio
     * and our own zoom). The PC renders the page at that size, so the
     * picture arrives 1:1.
     */
    public void setViewSize(int cssWidth, int cssHeight, float dpr, float zoom) {
        boolean changed = viewW != cssWidth || viewH != cssHeight
                || viewDpr != dpr || viewZoom != zoom;
        viewW = cssWidth;
        viewH = cssHeight;
        viewDpr = dpr;
        viewZoom = zoom;
        if (changed) pushLayout();
    }

    /** Send the current view size to the PC. */
    public void sendViewport() {
        if (!authed || !driver || ws == null) return;
        if (viewW <= 0 || viewH <= 0) return;
        try {
            ws.send(new JSONObject()
                    .put("type", "viewport")
                    .put("width", viewW)
                    .put("height", viewH)
                    .put("dpr", viewDpr)
                    .put("z", viewZoom)
                    .toString());
        } catch (Exception e) {
            Log.w(TAG, "viewport: " + e);
        }
    }

    /** Request the page layout this client wants, at our real size. */
    public void sendEmulation(boolean mobile) {
        emulMobile = mobile;
        pushLayout();
    }

    private void sendEmulationNow() {
        if (!authed || !driver || ws == null) return;
        int w = viewW > 0 ? viewW : 390;
        int h = viewH > 0 ? viewH : 844;
        try {
            ws.send(new JSONObject()
                    .put("type", "emulation")
                    .put("mobile", emulMobile)
                    .put("width", w)
                    .put("height", h)
                    .toString());
        } catch (Exception e) {
            Log.w(TAG, "emulation: " + e);
        }
    }

    /**
     * Tell the PC both how big our view is and which layout we want.
     * Sent whenever we get the wheel, so a reconnect after the laptop
     * left does not keep the laptop's layout.
     *
     * <p>The same message twice in a row is skipped: the PC only needs
     * to hear it when something actually changed (a new size, a new
     * layout, or a new driver).
     */
    private void pushLayout() {
        if (!authed || !driver) return;
        if (viewW <= 0 || viewH <= 0) return; // our size is not known yet
        boolean same = sentW == viewW && sentH == viewH
                && sentDpr == viewDpr && sentZoom == viewZoom
                && sentMobile == emulMobile;
        if (same) return;
        sendEmulationNow();
        sendViewport();
        sentW = viewW;
        sentH = viewH;
        sentDpr = viewDpr;
        sentZoom = viewZoom;
        sentMobile = emulMobile;
    }

    public void sendClipboardGet() {
        try {
            send(new JSONObject().put("type", "clipboard").put("action", "get"));
        } catch (Exception e) {
            Log.w(TAG, "clipboard get: " + e);
        }
    }

    public void sendClipboardSet(String text) {
        try {
            send(new JSONObject().put("type", "clipboard").put("action", "set")
                    .put("text", text == null ? "" : text));
        } catch (Exception e) {
            Log.w(TAG, "clipboard set: " + e);
        }
    }

    public void sendTabReload() {
        try {
            send(new JSONObject().put("type", "tab").put("action", "reload"));
        } catch (Exception e) {
            Log.w(TAG, "tab reload: " + e);
        }
    }

    public void sendHistory(String action) {
        try {
            send(new JSONObject().put("type", "history").put("action", action));
        } catch (Exception e) {
            Log.w(TAG, "history: " + e);
        }
    }

    public void sendFind(String text) {
        try {
            send(new JSONObject().put("type", "find").put("text", text == null ? "" : text));
        } catch (Exception e) {
            Log.w(TAG, "find: " + e);
        }
    }

    public void sendChrome(String action) {
        try {
            send(new JSONObject().put("type", "chrome").put("action", action));
        } catch (Exception e) {
            Log.w(TAG, "chrome: " + e);
        }
    }
}
