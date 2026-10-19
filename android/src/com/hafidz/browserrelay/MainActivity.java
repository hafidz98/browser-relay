package com.hafidz.browserrelay;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.res.ColorStateList;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Color;
import android.net.Uri;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.text.Editable;
import android.text.TextWatcher;
import android.util.Log;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.GridLayout;
import android.widget.HorizontalScrollView;
import android.widget.ImageButton;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.TextView;
import android.widget.Toast;

import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

/**
 * The native browser-relay client. Fullscreen frame viewport, latency
 * pill, edge handle with the locked two-tap interaction, tools bottom
 * sheet, tab pills, and the browserUp overlay.
 */
public class MainActivity extends Activity
        implements RelayClient.Listener, FrameView.InputSink {

    private static final String TAG = "MainActivity";
    private static final String PREFS = "browserrelay";
    private static final long IDLE_MS = 3500;

    // tool grid spec: icon, label, action id
    private static final String[] TOOL_LABELS = {
            "Back", "Forward", "Keyboard", "Paste",
            "Desktop", "Zoom", "Find", "Settings"
    };
    private static final int[] TOOL_ICONS = {
            R.drawable.ic_back, R.drawable.ic_forward,
            R.drawable.ic_keyboard, R.drawable.ic_paste,
            R.drawable.ic_desktop, R.drawable.ic_zoom,
            R.drawable.ic_find, R.drawable.ic_settings
    };
    private static final String[] TOOL_ACTIONS = {
            "back", "forward", "keyboard", "paste",
            "desktop", "zoom", "find", "settings"
    };

    private FrameLayout root;
    private FrameView frameView;
    private EdgeHandleView handle;
    private View browserDown;
    private View backdrop;
    private View hint;
    private LinearLayout sheet;
    private LinearLayout tabRow;
    private GridLayout toolGrid;
    private TextView latencyText;
    private View latencyDot;
    private TextView urlText;
    private ImageButton reloadBtn;
    private EditText kbd;

    private RelayClient relay;
    private SharedPreferences prefs;
    private final Handler handler = new Handler(Looper.getMainLooper());

    // ---- edge handle state machine (locked two-tap design) ----
    private enum HState { DOCKED, ROUND, OPEN }
    private HState hstate = HState.DOCKED;
    private EdgeHandleView.Side hside = EdgeHandleView.Side.RIGHT;
    private boolean sheetOpen;
    private boolean hintShown;
    private float rootW;
    private float rootH;
    /// Handle Y before the sheet opened (restored on close).
    private float preSheetHandleY = -1f;
    private boolean dragging;
    private boolean dragMoved;
    private float downRawX, downRawY, grabDX, grabDY, dragNX, dragNY;
    private final Runnable idleRunnable = this::fadeHandleIdle;
    private final Runnable roundRunnable = this::collapseRound;

    // ---- connection state ----
    private boolean authed;
    private boolean browserUp = true;
    private boolean mobile;
    private boolean firstWelcome = true;

    // ---- frame decode (latest-only, off the main thread) ----
    private final ExecutorService frameWorker = Executors.newSingleThreadExecutor();
    private final AtomicReference<byte[]> pendingFrame = new AtomicReference<>();
    private final AtomicBoolean decoding = new AtomicBoolean(false);
    private int framesLogged;

    private final Map<String, ImageView> toolIcons = new HashMap<>();
    private boolean kbdGuard;

    // ---- design helpers (match mockup) ----

    // Home fields.
    private View homeView;
    private GridLayout homeGrid;
    private boolean homeEditing;
    // Open tabs cache (for shortcut highlight).
    private java.util.List<RelayClient.TabInfo> openTabs =
            new java.util.ArrayList<>();

    /** Section label: small caps, muted, letter-spaced. */
    private TextView makeSectionLabel(String text) {
        TextView tv = new TextView(this);
        tv.setText(text);
        tv.setTextSize(11);
        tv.setTextColor(0xFF9A9A9A);
        tv.setTypeface(null, android.graphics.Typeface.BOLD);
        tv.setLetterSpacing(0.08f);
        return tv;
    }

    /** Card background: dark, rounded, subtle border. */
    private android.graphics.drawable.GradientDrawable makeCardBg(int borderColor) {
        android.graphics.drawable.GradientDrawable bg =
                new android.graphics.drawable.GradientDrawable();
        bg.setColor(0xFF1E1E1E);
        bg.setCornerRadius(dp(14));
        bg.setStroke((int) dp(1), borderColor);
        return bg;
    }

    /** Status dot view. */
    private View makeDot(int color, int sizeDp) {
        View dot = new View(this);
        android.graphics.drawable.GradientDrawable bg =
                new android.graphics.drawable.GradientDrawable();
        bg.setShape(android.graphics.drawable.GradientDrawable.OVAL);
        bg.setColor(color);
        dot.setBackground(bg);
        int s = (int) dp(sizeDp);
        dot.setLayoutParams(new LinearLayout.LayoutParams(s, s));
        return dot;
    }

    // ---- home (beranda) ----
    // ---- pairing card (integrated into beranda) ----
    private LinearLayout pairingCard;
    private LinearLayout statusRow;
    private android.widget.EditText pairHostInput;
    private android.widget.EditText pairCodeInput;
    private android.widget.Button pairRequestBtn;
    private android.widget.Button pairConnectBtn;
    private TextView statusHostText;
    private TextView gateIndicator;
    private WsClient pairWs;
    private boolean pairRequested = false;
    private static final String HOME_PREFS = "br_home";
    private static final String[][] HOME_DEFAULTS = {
            {"Facebook", "facebook.com", "#1877F2"},
            {"YouTube", "youtube.com", "#FF0000"},
            {"Google", "google.com", "#1A73E8"},
            {"X", "x.com", "#000000"},
            {"Instagram", "instagram.com", "#EE2A7B"},
            {"TikTok", "tiktok.com", "#111111"},
            {"GitHub", "github.com", "#24292F"},
            {"Wikipedia", "wikipedia.org", "#636363"},
    };

    // ============================== lifecycle ==============================

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_main);

        prefs = getSharedPreferences(PREFS, MODE_PRIVATE);
        root = findViewById(R.id.root);
        frameView = findViewById(R.id.frame);
        handle = findViewById(R.id.handle);
        browserDown = findViewById(R.id.browserDown);
        backdrop = findViewById(R.id.backdrop);
        hint = findViewById(R.id.hint);
        sheet = findViewById(R.id.sheet);
        tabRow = findViewById(R.id.tabRow);
        toolGrid = findViewById(R.id.toolGrid);
        latencyText = findViewById(R.id.latencyText);
        latencyDot = findViewById(R.id.latencyDot);
        // Tap the latency pill to manually reconnect when disconnected.
        android.view.View latencyPill = findViewById(R.id.latencyPill);
        latencyPill.setOnClickListener(v -> {
            if (!authed) userReconnect();
        });
        urlText = findViewById(R.id.urlText);
        reloadBtn = findViewById(R.id.reloadBtn);
        kbd = findViewById(R.id.kbd);

        frameView.setInputSink(this);
        relay = new RelayClient(this);

        buildToolGrid();
        setupHandle();
        setupSheet();
        setupKeyboard();
        setupHome();

        handlePairIntent(getIntent());
        updateBrowserDown();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        if (handlePairIntent(intent)) {
            connect();
        }
    }

    @Override
    protected void onResume() {
        super.onResume();
        connect();
    }

    @Override
    protected void onPause() {
        super.onPause();
        cancelReconnect();
        relay.disconnect();
        authed = false;
        handler.removeCallbacks(idleRunnable);
        handler.removeCallbacks(roundRunnable);
    }

    @Override
    public void onBackPressed() {
        if (homeView != null && homeView.getVisibility() == View.VISIBLE) {
            closeHome();
        } else if (sheetOpen) {
            closeSheet();
        } else {
            super.onBackPressed();
        }
    }

    // ============================== pairing ==============================

    /**
     * Handles browserrelay://pair?c=XXXXXX&host=... intents (fired by
     * the relay's /pair landing page after a QR scan). Returns true
     * when credentials were adopted.
     */
    private boolean handlePairIntent(Intent intent) {
        if (intent == null || !Intent.ACTION_VIEW.equals(intent.getAction())) return false;
        Uri data = intent.getData();
        if (data == null || !"browserrelay".equals(data.getScheme())) return false;
        String code = data.getQueryParameter("c");
        String host = data.getQueryParameter("host"); // e.g. http://192.168.1.5:8080
        if (code == null || !code.matches("\\d{6}")) return false;
        String hostPort = host == null ? ""
                : host.replaceFirst("^https?://", "").replaceAll("/+$", "");
        if (hostPort.isEmpty()) return false;
        prefs.edit().putString("br_host", hostPort).putString("br_token", code).apply();
        Toast.makeText(this, "Paired — connecting…", Toast.LENGTH_SHORT).show();
        return true;
    }

    private void connect() {
        String hostPort = prefs.getString("br_host", null);
        if (hostPort == null) {
            // No saved host: stay on beranda, the pairing card is visible.
            // (ConnectActivity is legacy — pairing lives in MainActivity now.)
            updatePairingCard(); updateGateIndicator();
            return;
        }
        String token = prefs.getString("br_token", "");
        String fp = getFingerprint();
        Log.i(TAG, "connecting to ws://" + hostPort + "/ws");
        relay.connect("ws://" + hostPort + "/ws", token, fp, deviceName());
    }

    /** Stable device fingerprint (the pairing contract identity). */
    private String getFingerprint() {
        String fp = prefs.getString("br_fingerprint", null);
        if (fp == null) {
            fp = java.util.UUID.randomUUID().toString();
            prefs.edit().putString("br_fingerprint", fp).apply();
        }
        return fp;
    }

    private String deviceName() {
        return android.os.Build.MODEL != null ? android.os.Build.MODEL : "Android";
    }

    private String fingerprint() {
        String fp = prefs.getString("br_fingerprint", null);
        if (fp == null) {
            fp = java.util.UUID.randomUUID().toString();
            prefs.edit().putString("br_fingerprint", fp).apply();
        }
        return fp;
    }

    // ============================== RelayClient.Listener ==============================

    @Override
    public void onWelcome(boolean driver, boolean browserUp, boolean mobile) {
        authed = true;
        cancelReconnect(); // Stop auto-reconnect on success.
        this.browserUp = browserUp;
        this.mobile = mobile;
        updateBrowserDown();
        updatePairingCard(); updateGateIndicator();
        updateDesktopTool();
        Log.i(TAG, "welcome driver=" + driver + " browserUp=" + browserUp);
        Toast.makeText(this, driver ? "Kamu mengendalikan" : "Mode lihat saja",
                Toast.LENGTH_SHORT).show();
        if (firstWelcome) {
            firstWelcome = false;
            hint.setVisibility(View.VISIBLE);
            hint.setAlpha(1f);
            hint.setOnClickListener(v -> hint.setVisibility(View.GONE));
            handler.postDelayed(() -> hint.animate().alpha(0f).setDuration(400)
                    .withEndAction(() -> hint.setVisibility(View.GONE)), 6000);
            // This client is always a phone: request mobile emulation so the
            // viewport matches, and our setting doesn't leak from a desktop.
            relay.sendEmulation(true, 390, 844);
        }
    }

    @Override
    public void onStatus(boolean browserUp) {
        this.browserUp = browserUp;
        updateBrowserDown();
    }

    @Override
    public void onLatency(long ms) {
        latencyText.setText(ms + " ms");
        int color = ms < 120 ? 0xFF3FB950 : ms < 350 ? 0xFFD29922 : 0xFFF85149;
        latencyDot.setBackgroundTintList(ColorStateList.valueOf(color));
    }

    @Override
    public void onRole(boolean driver) {
        Toast.makeText(this, driver ? "Kamu mengendalikan" : "Mode lihat saja",
                Toast.LENGTH_SHORT).show();
    }

    @Override
    public void onFrame(byte[] jpeg) {
        pendingFrame.set(jpeg);
        if (decoding.compareAndSet(false, true)) {
            frameWorker.execute(() -> {
                try {
                    byte[] data;
                    while ((data = pendingFrame.getAndSet(null)) != null) {
                        BitmapFactory.Options opts = new BitmapFactory.Options();
                        opts.inPreferredConfig = Bitmap.Config.RGB_565;
                        final Bitmap bmp = BitmapFactory.decodeByteArray(
                                data, 0, data.length, opts);
                        if (bmp != null) {
                            runOnUiThread(() -> {
                                frameView.setFrame(bmp);
                                if (framesLogged < 3) {
                                    framesLogged++;
                                    Log.i(TAG, "frame #" + framesLogged
                                            + " " + bmp.getWidth() + "x" + bmp.getHeight());
                                }
                            });
                        }
                    }
                } finally {
                    decoding.set(false);
                }
            });
        }
    }

    @Override
    public void onTabs(List<RelayClient.TabInfo> tabs) {
        openTabs = new java.util.ArrayList<>(tabs);
        runOnUiThread(() -> {
            renderTabs(tabs);
            // Refresh shortcut highlights.
            if (homeGrid != null) renderHome();
            renderSheetShortcuts();
        });
    }

    @Override
    public void onClipboard(String text) {
        ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        if (cm != null) {
            cm.setPrimaryClip(ClipData.newPlainText("relay", text));
            Toast.makeText(this, "Clipboard laptop disalin", Toast.LENGTH_SHORT).show();
        }
    }

    @Override
    public void onEmulation(boolean mobile) {
        this.mobile = mobile;
        updateDesktopTool();
    }

    @Override
    public void onFind(int count) {
        runOnUiThread(() -> Toast.makeText(this,
                count + " hasil", Toast.LENGTH_SHORT).show());
    }

    @Override
    public void onChrome(String action) {
        // The welcome/status broadcast hides the browserDown overlay
        // once Chrome is up; nothing else to do here.
    }

    @Override
    public void onAuthError(String msg) {
        if ("unknown device".equals(msg)) {
            // Not paired yet — show the pairing card on beranda.
            runOnUiThread(() -> {
                Toast.makeText(this, "Belum terpair — minta kode dari panel PC",
                        Toast.LENGTH_LONG).show();
                authed = false;
                updatePairingCard(); updateGateIndicator();
            });
            return;
        }
        Toast.makeText(this, "Gagal: " + msg + " — cek kode pairing",
                Toast.LENGTH_LONG).show();
        runOnUiThread(() -> {
            authed = false;
            updatePairingCard(); updateGateIndicator();
        });
    }

    @Override
    public void onPairWait() {
        runOnUiThread(() -> Toast.makeText(this,
                "Cek panel di PC — ketik kode yang muncul",
                Toast.LENGTH_LONG).show());
    }

    @Override
    public void onDisconnected() {
        boolean wasAuthed = authed;
        authed = false;
        runOnUiThread(() -> {
            latencyText.setText("— ms");
            latencyDot.setBackgroundTintList(ColorStateList.valueOf(0xFF6B6B6B));
            updateBrowserDown();
            if (wasAuthed) {
                // Server died or network dropped — auto-reconnect with
                // the saved fingerprint instead of showing the pairing card.
                scheduleReconnect();
            } else {
                updatePairingCard(); updateGateIndicator();
            }
        });
    }

    private int reconnectAttempts = 0;
    private Runnable reconnectRunnable;

    /** Auto-reconnect with exponential backoff (fingerprint auth). */
    private void scheduleReconnect() {
        if (reconnectRunnable != null) {
            handler.removeCallbacks(reconnectRunnable);
        }
        reconnectAttempts = 0;
        // Show reconnecting state in the status row.
        if (statusHostText != null) {
            statusHostText.setText("Menghubungkan ulang…");
        }
        updatePairingCard(); updateGateIndicator();
        reconnectRunnable = new Runnable() {
            @Override public void run() {
                if (authed) return; // Already reconnected.
                reconnectAttempts++;
                Log.i(TAG, "reconnect attempt " + reconnectAttempts);
                connect();
                // Exponential backoff: 2s, 4s, 8s, max 30s.
                long delay = Math.min(2000L * (1L << Math.min(reconnectAttempts, 4)), 30000L);
                handler.postDelayed(this, delay);
            }
        };
        handler.post(reconnectRunnable);
    }

    /** Cancel pending reconnect (e.g. user manually disconnects). */
    private void cancelReconnect() {
        if (reconnectRunnable != null) {
            handler.removeCallbacks(reconnectRunnable);
            reconnectRunnable = null;
        }
        reconnectAttempts = 0;
    }

    /** User-friendly error text: never leak java.io.* class names. */
    static String friendlyError(String msg) {
        if (msg == null) return "koneksi gagal";
        String m = msg.toLowerCase();
        // Strip exception class prefixes.
        msg = msg.replaceAll("^[a-zA-Z0-9_.]+\\.(IOException|Exception|SocketException|UnknownHostException|ConnectException|SocketTimeoutException):\\s*", "");
        if (m.contains("econnrefused") || m.contains("connection refused")) {
            return "server menolak koneksi — pastikan relay jalan di PC";
        }
        if (m.contains("ehostunreach") || m.contains("enetunreach") || m.contains("unreachable")) {
            return "host tidak terjangkau — cek WiFi / IP";
        }
        if (m.contains("unknownhost") || m.contains("unable to resolve")) {
            return "host tidak dikenal — cek alamat server";
        }
        if (m.contains("timeout") || m.contains("timed out")) {
            return "koneksi timeout — server tidak merespons";
        }
        if (m.contains("broken pipe") || m.contains("connection reset")) {
            return "koneksi terputus";
        }
        // Fallback: trimmed raw message, no package prefixes.
        msg = msg.replaceAll("[a-z]+\\.[a-z0-9_.]+\\.", "");
        if (msg.length() > 80) msg = msg.substring(0, 80) + "…";
        return msg.isEmpty() ? "koneksi gagal" : msg;
    }

    private long lastConnErrToast = 0;
    /** True when the current connect attempt was user-initiated. */
    private boolean manualConnect = false;

    @Override
    public void onConnError(String msg) {
        // Background auto-reconnect stays silent — no toast spam.
        // Only a manual reconnect attempt surfaces the error.
        if (!manualConnect) return;
        manualConnect = false;
        long now = System.currentTimeMillis();
        if (now - lastConnErrToast < 8000) return;
        lastConnErrToast = now;
        String friendly = friendlyError(msg);
        runOnUiThread(() -> Toast.makeText(this,
                "Koneksi gagal: " + friendly, Toast.LENGTH_LONG).show());
    }

    /** Manual reconnect (tap the latency pill when disconnected). */
    private void userReconnect() {
        manualConnect = true;
        runOnUiThread(() -> {
            latencyText.setText("…");
            Toast.makeText(this, "Menghubungkan…", Toast.LENGTH_SHORT).show();
        });
        connect();
    }

    private void updateBrowserDown() {
        browserDown.setVisibility(authed && !browserUp ? View.VISIBLE : View.GONE);
    }

    // ============================== FrameView.InputSink ==============================

    @Override
    public boolean canDrive() {
        return relay.isDriver();
    }

    @Override
    public void mouse(String action, float x, float y, String button) {
        relay.sendMouse(action, x, y, button);
    }

    @Override
    public void wheel(float x, float y, float dx, float dy) {
        relay.sendWheel(x, y, dx, dy);
    }

    @Override
    public void onContentDown() {
        // Tapping the web content collapses a popped-out round button.
        if (hstate == HState.ROUND) collapseRound();
    }

    // ============================== edge handle ==============================

    private float dp(float v) {
        return v * getResources().getDisplayMetrics().density;
    }

    private void setupHandle() {
        root.post(() -> {
            rootW = root.getWidth();
            rootH = root.getHeight();
            placeDocked(rootH * 0.6f);
            armIdle();
            // Park the (invisible, but laid-out) sheet below the screen.
            sheet.post(() -> sheet.setTranslationY(sheet.getHeight()));
        });

        handle.setOnTouchListener((v, ev) -> {
            switch (ev.getActionMasked()) {
                case MotionEvent.ACTION_DOWN:
                    if (hstate == HState.OPEN) return false; // let click close it
                    downRawX = ev.getRawX();
                    downRawY = ev.getRawY();
                    grabDX = ev.getRawX() - v.getX();
                    grabDY = ev.getRawY() - v.getY();
                    dragMoved = false;
                    dragging = true;
                    handler.removeCallbacks(idleRunnable);
                    handler.removeCallbacks(roundRunnable);
                    v.setAlpha(1f);
                    return true;
                case MotionEvent.ACTION_MOVE:
                    if (!dragging || hstate == HState.OPEN) return true;
                    if (!dragMoved
                            && Math.hypot(ev.getRawX() - downRawX, ev.getRawY() - downRawY) > dp(7)) {
                        dragMoved = true;
                        setHandleSize(dp(50), dp(50));
                        handle.setDragging(true);
                    }
                    if (dragMoved) {
                        dragNX = clamp(ev.getRawX() - grabDX, dp(4), rootW - dp(54));
                        dragNY = clamp(ev.getRawY() - grabDY, dp(48), rootH - dp(140));
                        v.setX(dragNX);
                        v.setY(dragNY);
                    }
                    return true;
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_CANCEL:
                    if (hstate == HState.OPEN) return false;
                    dragging = false;
                    handle.setDragging(false);
                    if (!dragMoved) {
                        if (hstate == HState.ROUND) openSheet(); // second tap
                        else popRound();                        // first tap
                    } else {
                        // release: snap to the nearest edge
                        hside = (dragNX + dp(25) < rootW / 2f)
                                ? EdgeHandleView.Side.LEFT : EdgeHandleView.Side.RIGHT;
                        handle.setSide(hside);
                        float top = clamp(dragNY + dp(3), dp(56), rootH - dp(130));
                        placeDocked(top);
                        armIdle();
                    }
                    return true;
                default:
                    return false;
            }
        });

        handle.setOnClickListener(v -> {
            if (hstate == HState.OPEN) closeSheet();
        });
    }

    private float clamp(float v, float lo, float hi) {
        return Math.max(lo, Math.min(hi, v));
    }

    private void setHandleSize(float w, float h) {
        ViewGroup.LayoutParams lp = handle.getLayoutParams();
        if (lp == null) lp = new FrameLayout.LayoutParams((int) w, (int) h);
        lp.width = (int) w;
        lp.height = (int) h;
        handle.setLayoutParams(lp);
    }

    /** Docked pill at the active edge. */
    private void placeDocked(float topY) {
        hstate = HState.DOCKED;
        handle.setState(EdgeHandleView.State.DOCKED);
        setHandleSize(dp(20), dp(56));
        handle.setX(hside == EdgeHandleView.Side.LEFT ? 0 : rootW - dp(20));
        handle.setY(topY);
    }

    /** First tap: the handle pops out of the edge as a round button. */
    private void popRound() {
        hstate = HState.ROUND;
        handle.setState(EdgeHandleView.State.ROUND);
        setHandleSize(dp(50), dp(50));
        float y = handle.getY();
        handle.setX(hside == EdgeHandleView.Side.LEFT ? dp(14) : rootW - dp(14) - dp(50));
        handle.setY(y);
        handler.removeCallbacks(roundRunnable);
        handler.postDelayed(roundRunnable, IDLE_MS); // ignored -> slips back
    }

    private void collapseRound() {
        if (hstate != HState.ROUND || sheetOpen) return;
        placeDocked(handle.getY());
        armIdle();
    }

    private void fadeHandleIdle() {
        if (hstate == HState.DOCKED && !sheetOpen) {
            handle.animate().alpha(0.38f).setDuration(350).start();
        }
    }

    private void armIdle() {
        handle.setAlpha(1f);
        handler.removeCallbacks(idleRunnable);
        handler.postDelayed(idleRunnable, IDLE_MS);
    }

    // ============================== bottom sheet ==============================

    private void setupSheet() {
        backdrop.setOnClickListener(v -> closeSheet());
        reloadBtn.setOnClickListener(v -> relay.sendTabReload());
        urlText.setOnClickListener(v -> askUrl());
        findViewById(R.id.homeBtn).setOnClickListener(v -> {
            closeSheet();
            openHome();
        });
        findViewById(R.id.launchChromeBtn).setOnClickListener(v -> {
            relay.sendChrome("launch");
            Toast.makeText(this, "Chrome launching…", Toast.LENGTH_SHORT).show();
        });
        // Shortcut grid in the sheet (mirrors beranda, for quick access
        // from the viewport without going back home).
        renderSheetShortcuts();
    }

    private void renderSheetShortcuts() {
        GridLayout grid = findViewById(R.id.sheetShortcuts);
        if (grid == null) return;
        grid.removeAllViews();
        final java.util.List<String[]> list = loadHome();
        // Show first 8 shortcuts.
        for (int i = 0; i < Math.min(8, list.size()); i++) {
            final String[] s = list.get(i);
            LinearLayout tile = new LinearLayout(this);
            tile.setOrientation(LinearLayout.VERTICAL);
            tile.setGravity(Gravity.CENTER_HORIZONTAL);
            tile.setPadding(0, (int) dp(6), 0, (int) dp(6));
            GridLayout.LayoutParams lp = new GridLayout.LayoutParams(
                    GridLayout.spec(i / 4), GridLayout.spec(i % 4, 1f));
            lp.width = 0;
            tile.setLayoutParams(lp);
            TextView icon = new TextView(this);
            icon.setText(s[0].substring(0, 1).toUpperCase());
            icon.setTextSize(18);
            icon.setTextColor(0xFFDADADA);
            icon.setGravity(Gravity.CENTER);
            android.graphics.drawable.GradientDrawable bg =
                    new android.graphics.drawable.GradientDrawable();
            bg.setCornerRadius(dp(12));
            try {
                bg.setColor(android.graphics.Color.parseColor(s[2]));
            } catch (Exception e) {
                bg.setColor(0xFF8B72F0);
            }
            bg.setAlpha(60);
            icon.setBackground(bg);
            int sz = (int) dp(44);
            LinearLayout.LayoutParams ilp = new LinearLayout.LayoutParams(sz, sz);
            ilp.setMargins(0, 0, 0, (int) dp(4));
            icon.setLayoutParams(ilp);
            TextView name = new TextView(this);
            name.setText(s[0]);
            name.setTextSize(10);
            name.setTextColor(0xFF9A9A9A);
            name.setGravity(Gravity.CENTER);
            name.setMaxLines(1);
            name.setEllipsize(android.text.TextUtils.TruncateAt.END);
            tile.addView(icon);
            tile.addView(name);
            tile.setOnClickListener(v -> {
                if (!browserUp) {
                    Toast.makeText(this, "Chrome belum jalan di laptop — jalankan dulu",
                            Toast.LENGTH_LONG).show();
                    return;
                }
                relay.sendTabNew(s[1]);
                closeSheet();
            });
            grid.addView(tile);
        }
    }

    private void openSheet() {
        if (sheetOpen) return;
        sheetOpen = true;
        handler.removeCallbacks(roundRunnable);
        handler.removeCallbacks(idleRunnable);
        hstate = HState.OPEN;
        handle.setState(EdgeHandleView.State.OPEN);
        handle.setAlpha(1f);
        setHandleSize(dp(50), dp(50));
        backdrop.setVisibility(View.VISIBLE);
        backdrop.setAlpha(0f);
        backdrop.animate().cancel();
        backdrop.animate().alpha(1f).setDuration(200).start();
        sheet.setVisibility(View.VISIBLE);
        sheet.animate().cancel();
        sheet.animate().translationY(0).setDuration(260).start();
        // Remember where the user parked the handle so close restores it.
        preSheetHandleY = handle.getY();
        // Park the × above the sheet so it never covers the tool grid.
        sheet.post(() -> {
            int sheetH = sheet.getHeight();
            handle.setX(rootW - dp(16) - dp(50));
            handle.setY(rootH - sheetH - dp(16) - dp(50));
        });
    }

    private void closeSheet() {
        if (!sheetOpen) return;
        sheetOpen = false;
        backdrop.animate().cancel();
        backdrop.animate().alpha(0f).setDuration(200)
                .withEndAction(() -> backdrop.setVisibility(View.GONE)).start();
        sheet.animate().cancel();
        sheet.animate().translationY(sheet.getHeight()).setDuration(240)
                .withEndAction(() -> sheet.setVisibility(View.INVISIBLE)).start();
        // land back as a docked handle, restoring the user's position
        float restoreY = preSheetHandleY >= 0 ? preSheetHandleY : handle.getY();
        preSheetHandleY = -1f;
        placeDocked(clamp(restoreY, dp(56), rootH - dp(130)));
        armIdle();
        hideKeyboard();
    }

    private void soon(String what) {
        Toast.makeText(this, what + " — segera hadir", Toast.LENGTH_SHORT).show();
    }

    // ---- tabs ----

    private void renderTabs(List<RelayClient.TabInfo> tabs) {
        tabRow.removeAllViews();
        String activeUrl = null;
        for (RelayClient.TabInfo t : tabs) {
            if (t.active) activeUrl = t.url;
            tabRow.addView(makeTabPill(t));
        }
        // "+" new-tab button
        TextView plus = new TextView(this);
        plus.setText("+");
        plus.setTextColor(0xFF9A9A9A);
        plus.setTextSize(17);
        plus.setGravity(Gravity.CENTER);
        plus.setBackgroundResource(R.drawable.circle_btn_bg);
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                (int) dp(32), (int) dp(32));
        lp.setMarginStart((int) dp(7));
        plus.setLayoutParams(lp);
        plus.setOnClickListener(v -> askUrl());
        tabRow.addView(plus);
        urlText.setText(activeUrl != null && !activeUrl.isEmpty() ? activeUrl : "—");
    }

    private View makeTabPill(RelayClient.TabInfo t) {
        LinearLayout pill = new LinearLayout(this);
        pill.setOrientation(LinearLayout.HORIZONTAL);
        pill.setGravity(Gravity.CENTER_VERTICAL);
        pill.setBackgroundResource(t.active
                ? R.drawable.tab_pill_active : R.drawable.tab_pill_bg);
        int pad = (int) dp(11);
        pill.setPadding(pad, (int) dp(7), (int) dp(7), (int) dp(7));

        View dot = new View(this);
        dot.setBackgroundResource(R.drawable.dot);
        dot.setBackgroundTintList(ColorStateList.valueOf(0xFF8B72F0));
        LinearLayout.LayoutParams dotLp = new LinearLayout.LayoutParams(
                (int) dp(8), (int) dp(8));
        dotLp.setMarginEnd((int) dp(7));
        dot.setLayoutParams(dotLp);
        pill.addView(dot);

        TextView title = new TextView(this);
        String label = t.title != null && !t.title.isEmpty() ? t.title : t.url;
        title.setText(label);
        title.setTextColor(t.active ? 0xFFDADADA : 0xFF9A9A9A);
        title.setTextSize(12.5f);
        title.setSingleLine(true);
        title.setEllipsize(android.text.TextUtils.TruncateAt.END);
        title.setMaxWidth((int) dp(148));
        pill.addView(title);

        TextView x = new TextView(this);
        x.setText("×");
        x.setTextColor(0xFF9A9A9A);
        x.setTextSize(14);
        x.setPadding((int) dp(7), 0, (int) dp(2), 0);
        final String id = t.id;
        x.setOnClickListener(v -> relay.sendTabClose(id));
        pill.addView(x);

        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.setMarginEnd((int) dp(7));
        pill.setLayoutParams(lp);
        pill.setOnClickListener(v -> relay.sendTabActivate(t.id));
        return pill;
    }

    // ============================== home (beranda) ==============================

    private void setupHome() {
        android.widget.ScrollView scroll = new android.widget.ScrollView(this);
        scroll.setBackgroundColor(0xFF161616);
        scroll.setVisibility(View.GONE);
        FrameLayout.LayoutParams slp = new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT);
        scroll.setLayoutParams(slp);

        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        scroll.addView(col, new ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        // header
        LinearLayout header = new LinearLayout(this);
        header.setOrientation(LinearLayout.HORIZONTAL);
        header.setGravity(Gravity.CENTER_VERTICAL);
        int hp = (int) dp(16);
        header.setPadding(hp, (int) dp(14), hp, (int) dp(12));
        LinearLayout titles = new LinearLayout(this);
        titles.setOrientation(LinearLayout.VERTICAL);
        LinearLayout.LayoutParams tlp = new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        titles.setLayoutParams(tlp);
        TextView h1 = new TextView(this);
        h1.setText("Beranda");
        h1.setTextSize(20);
        h1.setTextColor(0xFFDADADA);
        h1.setTypeface(null, android.graphics.Typeface.BOLD);
        titles.addView(h1);
        header.addView(titles);
        col.addView(header);

        // ---- pairing card (visible when unpaired) ----
        pairingCard = new LinearLayout(this);
        pairingCard.setOrientation(LinearLayout.VERTICAL);
        android.graphics.drawable.GradientDrawable pbg =
                new android.graphics.drawable.GradientDrawable();
        pbg.setColor(0xFF1E1E1E);
        pbg.setCornerRadius(dp(14));
        pbg.setStroke((int) dp(1), 0xFF8B72F0);
        pairingCard.setBackground(pbg);
        LinearLayout.LayoutParams plp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        plp.setMargins(hp, (int) dp(4), hp, (int) dp(4));
        pairingCard.setLayoutParams(plp);
        pairingCard.setPadding(hp, (int) dp(16), hp, (int) dp(16));

        TextView pt = new TextView(this);
        pt.setText("Hubungkan ke PC");
        pt.setTextSize(15);
        pt.setTextColor(0xFFDADADA);
        pt.setTypeface(null, android.graphics.Typeface.BOLD);
        pairingCard.addView(pt);
        // 3-gate stepper: Server -> Chrome -> Siap.
        gateIndicator = new TextView(this);
        gateIndicator.setTextSize(11);
        gateIndicator.setTextColor(0xFF9A9A9A);
        gateIndicator.setPadding(0, (int) dp(4), 0, (int) dp(8));
        pairingCard.addView(gateIndicator);
        updateGateIndicator();
        TextView ps = new TextView(this);
        ps.setText("Pilih cara pairing:");
        ps.setTextSize(12);
        ps.setTextColor(0xFF9A9A9A);
        ps.setPadding(0, (int) dp(2), 0, (int) dp(12));
        pairingCard.addView(ps);

        TextView hl = new TextView(this);
        hl.setText("Server (host:port)");
        hl.setTextSize(12);
        hl.setTextColor(0xFF9A9A9A);
        pairingCard.addView(hl);
        pairHostInput = new android.widget.EditText(this);
        pairHostInput.setHint("192.168.1.10:8765");
        pairHostInput.setTextSize(14);
        pairHostInput.setTextColor(0xFFDADADA);
        pairHostInput.setHintTextColor(0xFF6B6B6B);
        pairHostInput.setBackground(null);
        android.graphics.drawable.GradientDrawable hbg =
                new android.graphics.drawable.GradientDrawable();
        hbg.setColor(0xFF141414);
        hbg.setCornerRadius(dp(8));
        hbg.setStroke((int) dp(1), 0xFF333333);
        pairHostInput.setBackground(hbg);
        pairHostInput.setPadding((int) dp(12), (int) dp(11), (int) dp(12), (int) dp(11));
        LinearLayout.LayoutParams hlp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        hlp.setMargins(0, (int) dp(4), 0, (int) dp(12));
        pairHostInput.setLayoutParams(hlp);
        pairingCard.addView(pairHostInput);

        pairRequestBtn = new android.widget.Button(this);
        pairRequestBtn.setText("Minta Kode Pairing");
        pairRequestBtn.setTextColor(0xFFFFFFFF);
        pairRequestBtn.setCompoundDrawablesWithIntrinsicBounds(R.drawable.ic_link, 0, 0, 0);
        pairRequestBtn.setCompoundDrawablePadding((int) dp(8));
        android.graphics.drawable.GradientDrawable rbg =
                new android.graphics.drawable.GradientDrawable();
        rbg.setColor(0xFF8B72F0);
        rbg.setCornerRadius(dp(10));
        pairRequestBtn.setBackground(rbg);
        pairRequestBtn.setPadding(0, (int) dp(13), 0, (int) dp(13));
        pairRequestBtn.setOnClickListener(v -> onPairRequest());
        pairingCard.addView(pairRequestBtn);

        TextView cl = new TextView(this);
        cl.setText("Kode dari panel PC (6 digit)");
        cl.setTextSize(12);
        cl.setTextColor(0xFF9A9A9A);
        cl.setPadding(0, (int) dp(4), 0, 0);
        pairingCard.addView(cl);
        pairCodeInput = new android.widget.EditText(this);
        pairCodeInput.setHint("••••••");
        pairCodeInput.setTextSize(20);
        pairCodeInput.setTextColor(0xFFDADADA);
        pairCodeInput.setHintTextColor(0xFF6B6B6B);
        pairCodeInput.setGravity(Gravity.CENTER);
        pairCodeInput.setInputType(android.text.InputType.TYPE_CLASS_NUMBER);
        pairCodeInput.setBackground(hbg);
        pairCodeInput.setPadding((int) dp(12), (int) dp(11), (int) dp(12), (int) dp(11));
        LinearLayout.LayoutParams clp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        clp.setMargins(0, (int) dp(4), 0, (int) dp(12));
        pairCodeInput.setLayoutParams(clp);
        pairingCard.addView(pairCodeInput);

        pairConnectBtn = new android.widget.Button(this);
        pairConnectBtn.setText("Hubungkan");
        pairConnectBtn.setTextColor(0xFFDADADA);
        pairConnectBtn.setCompoundDrawablesWithIntrinsicBounds(R.drawable.ic_link, 0, 0, 0);
        pairConnectBtn.setCompoundDrawablePadding((int) dp(8));
        android.graphics.drawable.GradientDrawable cbg =
                new android.graphics.drawable.GradientDrawable();
        cbg.setColor(0xFF262626);
        cbg.setCornerRadius(dp(10));
        cbg.setStroke((int) dp(1), 0xFF333333);
        pairConnectBtn.setBackground(cbg);
        pairConnectBtn.setPadding(0, (int) dp(13), 0, (int) dp(13));
        pairConnectBtn.setOnClickListener(v -> onPairConnect());
        pairingCard.addView(pairConnectBtn);
        // QR note (mockup).
        TextView qrNote = new TextView(this);
        qrNote.setText("atau scan QR dari panel PC — otomatis kebuka");
        qrNote.setTextSize(11);
        qrNote.setTextColor(0xFF6B6B6B);
        qrNote.setGravity(Gravity.CENTER);
        qrNote.setPadding(0, (int) dp(10), 0, 0);
        pairingCard.addView(qrNote);
        col.addView(pairingCard);

        // ---- status row (visible when paired) ----
        statusRow = new LinearLayout(this);
        statusRow.setOrientation(LinearLayout.HORIZONTAL);
        statusRow.setGravity(Gravity.CENTER_VERTICAL);
        android.graphics.drawable.GradientDrawable sbg2 =
                new android.graphics.drawable.GradientDrawable();
        sbg2.setColor(0xFF1E1E1E);
        sbg2.setCornerRadius(dp(12));
        sbg2.setStroke((int) dp(1), 0xFF333333);
        statusRow.setBackground(sbg2);
        LinearLayout.LayoutParams slp2 = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        slp2.setMargins(hp, (int) dp(4), hp, (int) dp(4));
        statusRow.setLayoutParams(slp2);
        statusRow.setPadding(hp, (int) dp(12), hp, (int) dp(12));
        View dot = new View(this);
        android.graphics.drawable.GradientDrawable dbg =
                new android.graphics.drawable.GradientDrawable();
        dbg.setShape(android.graphics.drawable.GradientDrawable.OVAL);
        dbg.setColor(0xFF4CAF50);
        dot.setBackground(dbg);
        LinearLayout.LayoutParams dlp = new LinearLayout.LayoutParams((int) dp(10), (int) dp(10));
        dlp.setMargins(0, 0, (int) dp(10), 0);
        dot.setLayoutParams(dlp);
        statusRow.addView(dot);
        LinearLayout st = new LinearLayout(this);
        st.setOrientation(LinearLayout.VERTICAL);
        LinearLayout.LayoutParams stlp = new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        st.setLayoutParams(stlp);
        TextView st1 = new TextView(this);
        st1.setText("Terhubung ke PC");
        st1.setTextSize(13);
        st1.setTextColor(0xFFDADADA);
        TextView st2 = new TextView(this);
        st2.setTextSize(11);
        st2.setTextColor(0xFF9A9A9A);
        st2.setTag("status_host");
        st.addView(st1);
        st.addView(st2);
        statusRow.addView(st);
        statusHostText = st2;
        android.widget.Button disBtn = new android.widget.Button(this);
        disBtn.setText("Putus");
        disBtn.setTextSize(12);
        disBtn.setTextColor(0xFFFF8A8A);
        android.graphics.drawable.GradientDrawable dbgb =
                new android.graphics.drawable.GradientDrawable();
        dbgb.setColor(0xFF262626);
        dbgb.setCornerRadius(dp(8));
        dbgb.setStroke((int) dp(1), 0xFF4A2A2A);
        disBtn.setBackground(dbgb);
        disBtn.setOnClickListener(v -> {
            cancelReconnect();
            relay.disconnect();
            authed = false;
            updatePairingCard(); updateGateIndicator();
        });
        statusRow.addView(disBtn);
        col.addView(statusRow);

        // search box (mockup: 10px radius, not pill).
        TextView search = new TextView(this);
        search.setText("\uD83D\uDD0D  Cari atau ketik URL…");
        search.setTextSize(13);
        search.setTextColor(0xFF6B6B6B);
        search.setPadding((int) dp(12), (int) dp(12), (int) dp(12), (int) dp(12));
        android.graphics.drawable.GradientDrawable sbg =
                new android.graphics.drawable.GradientDrawable();
        sbg.setColor(0xFF1E1E1E);
        sbg.setCornerRadius(dp(10));
        sbg.setStroke((int) dp(1), 0xFF333333);
        search.setBackground(sbg);
        LinearLayout.LayoutParams sp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        sp.setMargins(hp, (int) dp(6), hp, (int) dp(12));
        search.setLayoutParams(sp);
        search.setOnClickListener(v -> askUrl());
        col.addView(search);

        // APLIKASI row: label left, edit button right (inline).
        LinearLayout secRow = new LinearLayout(this);
        secRow.setOrientation(LinearLayout.HORIZONTAL);
        secRow.setGravity(Gravity.CENTER_VERTICAL);
        secRow.setPadding(hp, (int) dp(8), hp, 0);
        TextView sec = makeSectionLabel("APLIKASI");
        LinearLayout.LayoutParams secLp = new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        sec.setLayoutParams(secLp);
        secRow.addView(sec);
        final android.widget.ImageButton editBtn = new android.widget.ImageButton(this);
        editBtn.setImageResource(R.drawable.ic_pencil);
        editBtn.setScaleType(android.widget.ImageView.ScaleType.CENTER_INSIDE);
        int ebs = (int) dp(32);
        editBtn.setLayoutParams(new LinearLayout.LayoutParams(ebs, ebs));
        editBtn.setBackground(null);
        editBtn.setOnClickListener(v -> {
            homeEditing = !homeEditing;
            editBtn.setImageResource(homeEditing ? R.drawable.ic_check : R.drawable.ic_pencil);
            renderHome();
        });
        secRow.addView(editBtn);
        col.addView(secRow);

        homeGrid = new GridLayout(this);
        homeGrid.setColumnCount(4);
        LinearLayout.LayoutParams glp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        glp.setMargins(hp, (int) dp(12), hp, (int) dp(24));
        homeGrid.setLayoutParams(glp);
        col.addView(homeGrid);

        homeView = scroll;
        root.addView(homeView);
        renderHome();
    }

    private java.util.List<String[]> loadHome() {
        java.util.List<String[]> out = new java.util.ArrayList<>();
        String raw = prefs.getString(HOME_PREFS, null);
        if (raw != null) {
            try {
                org.json.JSONArray arr = new org.json.JSONArray(raw);
                for (int i = 0; i < arr.length(); i++) {
                    org.json.JSONObject o = arr.getJSONObject(i);
                    out.add(new String[]{o.optString("name"), o.optString("url"),
                            o.optString("color", "#8B72F0")});
                }
                return out;
            } catch (org.json.JSONException e) {
                Log.w(TAG, "home load: " + e);
            }
        }
        for (String[] d : HOME_DEFAULTS) out.add(d.clone());
        return out;
    }

    private void saveHome(java.util.List<String[]> list) {
        try {
            org.json.JSONArray arr = new org.json.JSONArray();
            for (String[] s : list) {
                arr.put(new org.json.JSONObject().put("name", s[0])
                        .put("url", s[1]).put("color", s[2]));
            }
            prefs.edit().putString(HOME_PREFS, arr.toString()).apply();
        } catch (org.json.JSONException e) {
            Log.w(TAG, "home save: " + e);
        }
    }

    /** Update the 3-gate stepper: Server -> Chrome -> Siap. */
    private void updateGateIndicator() {
        if (gateIndicator == null) return;
        runOnUiThread(() -> {
            String s;
            if (authed && browserUp) {
                s = "✓ Server  →  ✓ Chrome  →  ✓ Siap";
            } else if (authed) {
                s = "✓ Server  →  ◌ Chrome  →  ○ Siap";
            } else if (reconnectRunnable != null) {
                // Reconnecting after unexpected disconnect.
                s = "◌ Server  →  ○ Chrome  →  ○ Siap  (menghubungkan ulang…)";
            } else {
                s = "○ Server  →  ○ Chrome  →  ○ Siap";
            }
            gateIndicator.setText(s);
        });
    }

    /** Toggle pairing card vs status row based on auth state. */
    private void updatePairingCard() {
        runOnUiThread(() -> {
            if (pairingCard == null) return;
            // During auto-reconnect, keep the status row (not the pairing card).
            boolean showStatus = authed || reconnectRunnable != null;
            pairingCard.setVisibility(showStatus ? View.GONE : View.VISIBLE);
            statusRow.setVisibility(showStatus ? View.VISIBLE : View.GONE);
            if (authed) {
                String host = prefs.getString("br_host", "");
                if (statusHostText != null) statusHostText.setText(host);
            } else {
                // Pre-fill saved host.
                String saved = prefs.getString("br_host", "");
                if (pairHostInput != null && pairHostInput.getText().length() == 0) {
                    pairHostInput.setText(saved);
                }
            }
            // Dim shortcuts when unpaired.
            if (homeGrid != null) homeGrid.setAlpha(authed ? 1f : 0.35f);
        });
    }

    private String pairCleanHost() {
        return pairHostInput.getText().toString().trim()
                .replaceFirst("^https?://", "").replaceAll("/+$", "");
    }

    /** Request-code flow: ask the PC panel to show a single-use code. */
    private void onPairRequest() {
        String host = pairCleanHost();
        if (host.isEmpty()) {
            Toast.makeText(this, "Isi server dulu (host:port)", Toast.LENGTH_SHORT).show();
            return;
        }
        prefs.edit().putString("br_host", host).apply();
        pairRequestBtn.setEnabled(false);
        pairRequestBtn.setText("Menunggu panel PC…");
        pairRequested = true;
        if (pairWs != null) pairWs.close();
        pairWs = new WsClient();
        final String fp = fingerprint();
        final String name = deviceName();
        pairWs.connect("ws://" + host + "/ws", new WsClient.Listener() {
            @Override public void onOpen() {
                try {
                    org.json.JSONObject m = new org.json.JSONObject();
                    m.put("type", "pair_request");
                    m.put("fingerprint", fp);
                    m.put("name", name);
                    pairWs.send(m.toString());
                } catch (Exception e) {
                    onPairError(e.getMessage());
                }
            }
            @Override public void onMessage(String text) {
                try {
                    org.json.JSONObject m = new org.json.JSONObject(text);
                    if ("pair_wait".equals(m.optString("type"))) {
                        runOnUiThread(() -> Toast.makeText(MainActivity.this,
                                "Cek panel di PC — ketik kode yang muncul",
                                Toast.LENGTH_LONG).show());
                    }
                } catch (Exception ignored) {}
            }
            @Override public void onError(String msg) { onPairError(msg); }
            @Override public void onClose() {}
        });
    }

    private void onPairError(String msg) {
        runOnUiThread(() -> {
            pairRequested = false;
            pairRequestBtn.setEnabled(true);
            pairRequestBtn.setText("Minta Kode Pairing");
            Toast.makeText(this, "Gagal minta kode: " + friendlyError(msg),
                    Toast.LENGTH_LONG).show();
        });
    }

    /** Submit the 6-digit code to pair. */
    private void onPairConnect() {
        String host = pairCleanHost();
        String code = pairCodeInput.getText().toString().trim();
        if (host.isEmpty()) {
            Toast.makeText(this, "Isi server dulu (host:port)", Toast.LENGTH_SHORT).show();
            return;
        }
        if (code.length() != 6) {
            Toast.makeText(this, "Kode harus 6 digit", Toast.LENGTH_SHORT).show();
            return;
        }
        prefs.edit().putString("br_host", host).putString("br_token", code).apply();
        if (pairWs != null) { pairWs.close(); pairWs = null; }
        pairRequested = false;
        // Connect via the main relay client; onAuthOk flips the card.
        manualConnect = true;
        connect();
        Toast.makeText(this, "Menghubungkan…", Toast.LENGTH_SHORT).show();
    }

    /** Find the tab ID matching a shortcut URL, or null. */
    private String findTabId(String shortcutUrl) {
        if (shortcutUrl == null) return null;
        String host = shortcutUrl.replaceFirst("^https?://", "").split("/")[0].toLowerCase();
        for (RelayClient.TabInfo t : openTabs) {
            if (t.url == null) continue;
            String thost = t.url.replaceFirst("^https?://", "").split("/")[0].toLowerCase();
            if (thost.equals(host) || thost.endsWith("." + host) || host.endsWith("." + thost)) {
                return t.id;
            }
        }
        return null;
    }

    /** Check if a shortcut URL matches any open tab. */
    private boolean isTabOpen(String shortcutUrl) {
        if (shortcutUrl == null) return false;
        String host = shortcutUrl.replaceFirst("^https?://", "").split("/")[0].toLowerCase();
        for (RelayClient.TabInfo t : openTabs) {
            if (t.url == null) continue;
            String thost = t.url.replaceFirst("^https?://", "").split("/")[0].toLowerCase();
            if (thost.equals(host) || thost.endsWith("." + host) || host.endsWith("." + thost)) {
                return true;
            }
        }
        return false;
    }

    private void renderHome() {
        homeGrid.removeAllViews();
        final java.util.List<String[]> list = loadHome();
        for (int i = 0; i < list.size(); i++) {
            final int idx = i;
            final String[] s = list.get(i);
            LinearLayout tile = new LinearLayout(this);
            tile.setOrientation(LinearLayout.VERTICAL);
            tile.setGravity(Gravity.CENTER_HORIZONTAL);
            tile.setPadding(0, (int) dp(9), 0, (int) dp(9));
            GridLayout.LayoutParams lp = new GridLayout.LayoutParams(
                    GridLayout.spec(i / 4), GridLayout.spec(i % 4, 1f));
            lp.width = 0;
            tile.setLayoutParams(lp);

            TextView icon = new TextView(this);
            icon.setText(s[0].isEmpty() ? "?" : s[0].substring(0, 1).toUpperCase());
            icon.setTextSize(24);
            icon.setTextColor(0xFFFFFFFF);
            icon.setGravity(Gravity.CENTER);
            android.graphics.drawable.GradientDrawable bg =
                    new android.graphics.drawable.GradientDrawable();
            try {
                bg.setColor(Color.parseColor(s[2]));
            } catch (Exception e) {
                bg.setColor(0xFF8B72F0);
            }
            bg.setCornerRadius(dp(14));
            // Highlight if this shortcut's tab is open in Chrome.
            boolean tabOpen = isTabOpen(s[1]);
            if (tabOpen) {
                bg.setStroke((int) dp(2), 0xFF8B72F0);
            }
            icon.setBackground(bg);
            LinearLayout.LayoutParams ilp = new LinearLayout.LayoutParams(
                    (int) dp(52), (int) dp(52));
            icon.setLayoutParams(ilp);
            // Wrap icon in FrameLayout for the green dot overlay.
            FrameLayout iconWrap = new FrameLayout(this);
            iconWrap.addView(icon, new FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                    Gravity.CENTER));
            if (tabOpen) {
                View dot = new View(this);
                android.graphics.drawable.GradientDrawable dotBg =
                        new android.graphics.drawable.GradientDrawable();
                dotBg.setShape(android.graphics.drawable.GradientDrawable.OVAL);
                dotBg.setColor(0xFF4CAF50);
                dotBg.setStroke((int) dp(1), 0xFF161616);
                dot.setBackground(dotBg);
                FrameLayout.LayoutParams dotLp = new FrameLayout.LayoutParams(
                        (int) dp(12), (int) dp(12), Gravity.TOP | Gravity.END);
                iconWrap.addView(dot, dotLp);
            }
            tile.addView(iconWrap);

            TextView label = new TextView(this);
            label.setText(s[0]);
            label.setTextSize(11);
            label.setTextColor(0xFFDADADA);
            label.setGravity(Gravity.CENTER);
            label.setSingleLine(true);
            label.setEllipsize(android.text.TextUtils.TruncateAt.END);
            LinearLayout.LayoutParams llp = new LinearLayout.LayoutParams(
                    (int) dp(76), ViewGroup.LayoutParams.WRAP_CONTENT);
            llp.topMargin = (int) dp(6);
            label.setLayoutParams(llp);
            tile.addView(label);
            // "terbuka" label for open tabs.
            if (tabOpen) {
                TextView openLabel = new TextView(this);
                openLabel.setText("● terbuka");
                openLabel.setTextSize(9);
                openLabel.setTextColor(0xFF4CAF50);
                openLabel.setGravity(Gravity.CENTER);
                tile.addView(openLabel);
            }

            if (homeEditing) {
                TextView del = new TextView(this);
                del.setText("×");
                del.setTextSize(15);
                del.setTextColor(0xFFFFFFFF);
                del.setGravity(Gravity.CENTER);
                android.graphics.drawable.GradientDrawable dbg =
                        new android.graphics.drawable.GradientDrawable();
                dbg.setColor(0xFFF85149);
                dbg.setCornerRadius(dp(12));
                del.setBackground(dbg);
                FrameLayout wrapper = new FrameLayout(this);
                wrapper.addView(tile, new FrameLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.WRAP_CONTENT));
                FrameLayout.LayoutParams dlp = new FrameLayout.LayoutParams(
                        (int) dp(24), (int) dp(24), Gravity.TOP | Gravity.END);
                wrapper.addView(del, dlp);
                del.setOnClickListener(v -> {
                    java.util.List<String[]> l2 = loadHome();
                    l2.remove(idx);
                    saveHome(l2);
                    renderHome();
                    Toast.makeText(this, "Shortcut dihapus", Toast.LENGTH_SHORT).show();
                });
                tile.setOnClickListener(null);
                GridLayout.LayoutParams wlp = new GridLayout.LayoutParams(
                        GridLayout.spec(i / 4), GridLayout.spec(i % 4, 1f));
                wlp.width = 0;
                wrapper.setLayoutParams(wlp);
                homeGrid.addView(wrapper);
            } else {
                tile.setOnClickListener(v -> {
                    // Gate 2: Chrome must be running.
                    if (!browserUp) {
                        updateBrowserDown();
                        Toast.makeText(this,
                                "Chrome belum jalan di laptop — jalankan dulu",
                                Toast.LENGTH_LONG).show();
                        return;
                    }
                    // If the tab is already open, switch to it instead of
                    // opening a duplicate.
                    String tabId = findTabId(s[1]);
                    if (tabId != null) {
                        relay.sendTabActivate(tabId);
                    } else {
                        relay.sendTabNew(s[1]);
                    }
                    closeHome();
                });
                homeGrid.addView(tile);
            }
        }
        if (!homeEditing) {
            LinearLayout add = new LinearLayout(this);
            add.setOrientation(LinearLayout.VERTICAL);
            add.setGravity(Gravity.CENTER_HORIZONTAL);
            add.setPadding(0, (int) dp(9), 0, (int) dp(9));
            GridLayout.LayoutParams alp = new GridLayout.LayoutParams(
                    GridLayout.spec(list.size() / 4),
                    GridLayout.spec(list.size() % 4, 1f));
            alp.width = 0;
            add.setLayoutParams(alp);
            TextView plus = new TextView(this);
            plus.setText("+");
            plus.setTextSize(28);
            plus.setTextColor(0xFF9A9A9A);
            plus.setGravity(Gravity.CENTER);
            android.graphics.drawable.GradientDrawable pbg =
                    new android.graphics.drawable.GradientDrawable();
            pbg.setStroke((int) dp(1.5f), 0xFF454545);
            pbg.setCornerRadius(dp(16));
            plus.setBackground(pbg);
            plus.setLayoutParams(new LinearLayout.LayoutParams((int) dp(60), (int) dp(60)));
            add.addView(plus);
            TextView alabel = new TextView(this);
            alabel.setText("Tambah");
            alabel.setTextSize(11.5f);
            alabel.setTextColor(0xFFDADADA);
            alabel.setGravity(Gravity.CENTER);
            LinearLayout.LayoutParams allp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            allp.topMargin = (int) dp(8);
            alabel.setLayoutParams(allp);
            add.addView(alabel);
            add.setOnClickListener(v -> askAddShortcut());
            homeGrid.addView(add);
        }
    }

    private void askAddShortcut() {
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setPadding((int) dp(4), 0, (int) dp(4), 0);
        final EditText nameIn = new EditText(this);
        nameIn.setHint("Nama (cth: Facebook)");
        final EditText urlIn = new EditText(this);
        urlIn.setHint("URL (cth: facebook.com)");
        urlIn.setInputType(android.text.InputType.TYPE_TEXT_VARIATION_URI);
        col.addView(nameIn);
        col.addView(urlIn);
        new AlertDialog.Builder(this)
                .setTitle("Tambah shortcut")
                .setView(col)
                .setPositiveButton("Simpan", (d, w) -> {
                    String name = nameIn.getText().toString().trim();
                    String url = urlIn.getText().toString().trim()
                            .replaceFirst("^https?://", "").replaceAll("/$", "");
                    if (name.isEmpty()) {
                        Toast.makeText(this, "Isi namanya dulu", Toast.LENGTH_SHORT).show();
                        return;
                    }
                    if (url.isEmpty() || !url.contains(".")) {
                        Toast.makeText(this, "URL-nya belum valid", Toast.LENGTH_SHORT).show();
                        return;
                    }
                    java.util.List<String[]> l = loadHome();
                    l.add(new String[]{name, url, "#8B72F0"});
                    saveHome(l);
                    renderHome();
                    Toast.makeText(this, "Shortcut tersimpan", Toast.LENGTH_SHORT).show();
                })
                .setNegativeButton("Batal", null)
                .show();
    }

    private void openHome() {
        homeEditing = false;
        renderHome();
        updatePairingCard(); updateGateIndicator();
        homeView.setVisibility(View.VISIBLE);
        homeView.bringToFront();
    }

    private void closeHome() {
        homeEditing = false;
        if (homeView != null) homeView.setVisibility(View.GONE);
    }

    private void askUrl() {
        // Gate 2: Chrome must be running before opening URLs.
        if (!browserUp) {
            updateBrowserDown();
            Toast.makeText(this, "Chrome belum jalan di laptop — jalankan dulu",
                    Toast.LENGTH_LONG).show();
            return;
        }
        final EditText input = new EditText(this);
        input.setInputType(android.text.InputType.TYPE_TEXT_VARIATION_URI);
        input.setText("https://");
        input.setSelection(input.getText().length());
        new AlertDialog.Builder(this)
                .setTitle("Buka URL")
                .setView(input)
                .setPositiveButton("Buka", (d, w) -> {
                    String u = input.getText().toString().trim();
                    if (!u.isEmpty()) relay.sendTabNew(u);
                })
                .setNegativeButton("Batal", null)
                .show();
    }

    // ---- tool grid ----

    private void buildToolGrid() {
        toolGrid.removeAllViews();
        for (int i = 0; i < TOOL_ACTIONS.length; i++) {
            final String action = TOOL_ACTIONS[i];
            LinearLayout item = new LinearLayout(this);
            item.setOrientation(LinearLayout.VERTICAL);
            item.setGravity(Gravity.CENTER_HORIZONTAL);
            item.setPadding(0, (int) dp(6), 0, (int) dp(6));

            // Icon container: 48dp, #2a2a2a, 12dp radius (mockup).
            FrameLayout iconBox = new FrameLayout(this);
            android.graphics.drawable.GradientDrawable ibg =
                    new android.graphics.drawable.GradientDrawable();
            ibg.setColor(0xFF2A2A2A);
            ibg.setCornerRadius(dp(12));
            iconBox.setBackground(ibg);
            int boxSize = (int) dp(48);
            iconBox.setLayoutParams(new LinearLayout.LayoutParams(boxSize, boxSize));

            ImageView icon = new ImageView(this);
            icon.setImageResource(TOOL_ICONS[i]);
            int iconSize = (int) dp(22);
            FrameLayout.LayoutParams iconLp = new FrameLayout.LayoutParams(
                    iconSize, iconSize, Gravity.CENTER);
            icon.setLayoutParams(iconLp);
            iconBox.addView(icon);
            item.addView(iconBox);
            toolIcons.put(action, icon);

            TextView label = new TextView(this);
            label.setText(TOOL_LABELS[i]);
            label.setTextColor(0xFF9A9A9A);
            label.setTextSize(10);
            label.setPadding(0, (int) dp(6), 0, 0);
            label.setGravity(Gravity.CENTER);
            item.addView(label);

            GridLayout.LayoutParams lp = new GridLayout.LayoutParams(
                    GridLayout.spec(i / 4), GridLayout.spec(i % 4, 1f));
            lp.width = 0;
            item.setLayoutParams(lp);
            item.setOnClickListener(v -> onTool(action));
            toolGrid.addView(item);
        }
    }

    private void onTool(String action) {
        switch (action) {
            case "back":
                relay.sendHistory("back");
                break;
            case "forward":
                relay.sendHistory("forward");
                break;
            case "keyboard":
                openKeyboard();
                break;
            case "paste":
                pasteFromPhone();
                break;
            case "desktop":
                relay.sendEmulation(!mobile);
                break;
            case "zoom":
                soon("Zoom");
                break;
            case "find":
                askFind();
                break;
            case "settings":
                soon("Pengaturan");
                break;
            default:
                break;
        }
    }

    private void askFind() {
        final EditText input = new EditText(this);
        input.setHint("Cari di halaman…");
        new AlertDialog.Builder(this)
                .setTitle("Find in page")
                .setView(input)
                .setPositiveButton("Cari", (d, w) -> {
                    String q = input.getText().toString().trim();
                    if (!q.isEmpty()) relay.sendFind(q);
                })
                .setNegativeButton("Batal", null)
                .show();
    }

    private void updateDesktopTool() {
        ImageView icon = toolIcons.get("desktop");
        if (icon != null) {
            if (mobile) icon.setColorFilter(0xFF8B72F0);
            else icon.clearColorFilter();
        }
    }

    // ---- soft keyboard path (mirrors the web client's hidden input) ----

    private void setupKeyboard() {
        kbd.addTextChangedListener(new TextWatcher() {
            @Override
            public void beforeTextChanged(CharSequence s, int st, int c, int a) {
            }

            @Override
            public void onTextChanged(CharSequence s, int st, int b, int c) {
            }

            @Override
            public void afterTextChanged(Editable s) {
                if (kbdGuard) return;
                String t = s.toString();
                if (!t.isEmpty()) {
                    kbdGuard = true;
                    s.clear();
                    kbdGuard = false;
                    relay.sendKeyText(t);
                }
            }
        });
        kbd.setOnKeyListener((v, keyCode, event) -> {
            if (event.getAction() == KeyEvent.ACTION_DOWN) {
                if (keyCode == KeyEvent.KEYCODE_DEL) {
                    relay.sendKeyCode(8, true);
                    relay.sendKeyCode(8, false);
                    return true;
                }
                if (keyCode == KeyEvent.KEYCODE_ENTER) {
                    relay.sendKeyCode(13, true);
                    relay.sendKeyCode(13, false);
                    return true;
                }
            }
            return false;
        });
    }

    private void openKeyboard() {
        kbd.requestFocus();
        InputMethodManager imm = (InputMethodManager)
                getSystemService(Context.INPUT_METHOD_SERVICE);
        if (imm != null) imm.showSoftInput(kbd, InputMethodManager.SHOW_IMPLICIT);
    }

    private void hideKeyboard() {
        InputMethodManager imm = (InputMethodManager)
                getSystemService(Context.INPUT_METHOD_SERVICE);
        if (imm != null) imm.hideSoftInputFromWindow(kbd.getWindowToken(), 0);
        kbd.clearFocus();
    }

    private void pasteFromPhone() {
        ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        CharSequence text = cm != null && cm.getPrimaryClip() != null
                && cm.getPrimaryClip().getItemCount() > 0
                ? cm.getPrimaryClip().getItemAt(0).coerceToText(this) : null;
        if (text == null || text.length() == 0) {
            Toast.makeText(this, "Clipboard HP kosong", Toast.LENGTH_SHORT).show();
            return;
        }
        relay.sendClipboardSet(text.toString());
        Toast.makeText(this, "Terkirim ke laptop", Toast.LENGTH_SHORT).show();
    }
}
