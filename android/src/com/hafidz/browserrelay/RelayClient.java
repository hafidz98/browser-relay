package com.hafidz.browserrelay;

import android.os.Handler;
import android.os.Looper;
import android.util.Base64;
import android.util.Log;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;

/**
 * browser-relay wire protocol over WsClient. Mirrors
 * shell/assets/client/app.js message for message:
 * <ul>
 *   <li>client -&gt; relay: auth, ping, mouse, wheel, key, tab,
 *       emulation, clipboard</li>
 *   <li>relay -&gt; client: welcome, status, pong, role, frame,
 *       tabs, clipboard, emulation, error</li>
 * </ul>
 * Input sends are gated on authed &amp;&amp; driver, like the web
 * client's send() — one driver at a time, viewers are read-only.
 */
public class RelayClient {
    private static final String TAG = "RelayClient";
    private static final long PING_MS = 2000;
    private static final int MAX_BACKOFF_MS = 10000;

    public static class TabInfo {
        public String id;
        public String title;
        public String url;
        public boolean active;
    }

    public interface Listener {
        void onWelcome(boolean driver, boolean browserUp, boolean mobile);
        void onStatus(boolean browserUp);
        void onLatency(long ms);
        void onRole(boolean driver);
        void onFrame(byte[] jpeg);
        void onTabs(List<TabInfo> tabs);
        void onClipboard(String text);
        void onEmulation(boolean mobile);
        void onFind(int count);
        void onChrome(String action);
        void onAuthError(String msg);
        void onDisconnected();
        void onConnError(String msg);
        void onPairWait();
    }

    private final Handler main = new Handler(Looper.getMainLooper());
    private final Listener listener;
    private WsClient ws;
    private String wsUrl;
    private String token;
    private String fingerprint = "";
    private String deviceName = "";
    private boolean authed;
    private boolean driver;
    private boolean wantConnect;
    private int backoffMs = 2000;

    private final Runnable pingTask = new Runnable() {
        @Override
        public void run() {
            if (wantConnect && authed && ws != null) {
                try {
                    ws.send(new JSONObject()
                            .put("type", "ping")
                            .put("t", System.currentTimeMillis())
                            .toString());
                } catch (Exception e) {
                    Log.w(TAG, "ping: " + e);
                }
                main.postDelayed(this, PING_MS);
            }
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

    public void connect(String wsUrl, String token, String fingerprint, String name) {
        disconnect();
        this.wsUrl = wsUrl;
        this.token = token;
        this.fingerprint = fingerprint;
        this.deviceName = name;
        wantConnect = true;
        backoffMs = 2000;
        doConnect();
    }

    private void doConnect() {
        authed = false;
        driver = false;
        ws = new WsClient();
        ws.connect(wsUrl, new WsClient.Listener() {
            @Override
            public void onOpen() {
                backoffMs = 2000;
                try {
                    JSONObject auth = new JSONObject()
                            .put("type", "auth")
                            .put("fingerprint", fingerprint);
                    // Single-use QR/pair code takes precedence when present.
                    if (token != null && !token.isEmpty()) {
                        auth.put("token", token).put("name", deviceName);
                    }
                    ws.send(auth.toString());
                } catch (Exception e) {
                    Log.w(TAG, "auth send: " + e);
                }
            }

            @Override
            public void onMessage(String text) {
                handle(text);
            }

            @Override
            public void onClose() {
                authed = false;
                main.removeCallbacks(pingTask);
                listener.onDisconnected();
                if (wantConnect) {
                    main.postDelayed(reconnectTask, backoffMs);
                    backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS);
                }
            }

            @Override
            public void onError(String msg) {
                Log.w(TAG, "ws: " + msg);
                listener.onConnError(msg);
            }
        });
    }

    /** Ask the relay for a single-use pairing code (shown on the PC panel). */
    public void requestPairing() {
        try {
            ws.send(new JSONObject()
                    .put("type", "pair_request")
                    .put("fingerprint", fingerprint)
                    .put("name", deviceName)
                    .toString());
        } catch (Exception e) {
            Log.w(TAG, "pair_request send: " + e);
        }
    }

    /** Pair with a code read off the PC panel. */
    public void pairWithCode(String code) {
        this.token = code;
        // Re-auth on the live socket without a full reconnect.
        try {
            ws.send(new JSONObject()
                    .put("type", "auth")
                    .put("token", code)
                    .put("fingerprint", fingerprint)
                    .put("name", deviceName)
                    .toString());
        } catch (Exception e) {
            Log.w(TAG, "pair auth send: " + e);
        }
    }

    public void disconnect() {
        wantConnect = false;
        main.removeCallbacks(pingTask);
        main.removeCallbacks(reconnectTask);
        if (ws != null) {
            ws.close();
            ws = null;
        }
        authed = false;
        driver = false;
    }

    public boolean isDriver() {
        return authed && driver;
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
            case "pair_wait":
                listener.onPairWait();
                break;
            case "welcome":
                authed = true;
                driver = m.optBoolean("driver", false);
                main.removeCallbacks(pingTask);
                main.postDelayed(pingTask, PING_MS);
                listener.onWelcome(driver, m.optBoolean("browserUp", true),
                        m.optBoolean("mobile", false));
                break;
            case "status":
                listener.onStatus(m.optBoolean("browserUp", true));
                break;
            case "pong":
                listener.onLatency(System.currentTimeMillis() - m.optLong("t", 0));
                break;
            case "role":
                driver = m.optBoolean("driver", false);
                listener.onRole(driver);
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
                listener.onEmulation(m.optBoolean("mobile", false));
                break;
            case "find":
                listener.onFind(m.optInt("count", 0));
                break;
            case "chrome":
                listener.onChrome(m.optString("action", ""));
                break;
            case "error":
                String msg = m.optString("message", "error");
                if (!authed) {
                    listener.onAuthError(msg);
                } else {
                    Log.w(TAG, "relay error: " + msg);
                }
                break;
            default:
                break;
        }
    }

    private void send(JSONObject o) {
        if (authed && driver && ws != null) {
            ws.send(o.toString());
        }
    }

    // ---- client -> relay ----

    public void sendMouse(String action, double x, double y, String button) {
        try {
            send(new JSONObject().put("type", "mouse").put("action", action)
                    .put("x", x).put("y", y).put("button", button));
        } catch (Exception e) {
            Log.w(TAG, "mouse: " + e);
        }
    }

    public void sendWheel(double x, double y, double dx, double dy) {
        try {
            send(new JSONObject().put("type", "wheel")
                    .put("x", x).put("y", y).put("dx", dx).put("dy", dy));
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

    public void sendEmulation(boolean mobile) {
        sendEmulation(mobile, 390, 844);
    }

    public void sendEmulation(boolean mobile, int width, int height) {
        try {
            send(new JSONObject()
                    .put("type", "emulation")
                    .put("mobile", mobile)
                    .put("width", width)
                    .put("height", height));
        } catch (Exception e) {
            Log.w(TAG, "emulation: " + e);
        }
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
