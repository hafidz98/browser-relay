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
    private TextView urlText;
    private ImageButton reloadBtn;
    private EditText kbd;
    private View takeoverBtn;
    private TextView statusBanner;

    // ---- the three states (exactly one at a time) ----
    /** Not connected: the address + code + Connect card. */
    private View connectCard;
    private EditText serverInput;
    private EditText codeInput;
    private View connectBtn;
    private TextView connectError;
    /** Connected: the slim bar with the state and the round-trip time. */
    private View statusBar;
    private TextView statusText;
    private View statusDot;
    /** Connection lost: the banner with the Reconnect button. */
    private View lostBanner;
    private TextView lostText;
    private View reconnectBtn;

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
    /** The relay said hello: we have a session. */
    private boolean connected;
    /** "Driving Chrome" or "View only" - the status bar shows it. */
    private String roleLabel = "";
    /** The session ended on its own: the Reconnect banner. */
    private boolean lost;
    private boolean browserUp = true;
    private boolean mobile;
    private boolean firstWelcome = true;
    /** The page is in the background: the picture is frozen. */
    private boolean paused = false;
    /** Chrome runs without the flags that keep it painting. */
    private boolean flagsOk = true;
    /** The last thing the relay complained about (shown in the banner). */
    private String serverNote = "";

    // ---- frame decode (latest-only, off the main thread) ----
    private final ExecutorService frameWorker = Executors.newSingleThreadExecutor();
    private final AtomicReference<byte[]> pendingFrame = new AtomicReference<>();
    private final AtomicBoolean decoding = new AtomicBoolean(false);
    private int framesLogged;

    private final Map<String, ImageView> toolIcons = new HashMap<>();
    private boolean kbdGuard;

    // ---- design helpers (match mockup) ----

    // The shortcut grid of the beranda (the sites the user saved).
    private boolean homeEditing;
    // Open tabs cache (for shortcut highlight).
    private java.util.List<RelayClient.TabInfo> openTabs =
            new java.util.ArrayList<>();

    /** Section label: small caps, muted, letter-spaced. */
    private TextView makeSectionLabel(String text) {
        TextView tv = new TextView(this);
        tv.setText(text);
        tv.setTextSize(11);
        tv.setTextColor(getColor(R.color.relay_muted));
        tv.setTypeface(null, android.graphics.Typeface.BOLD);
        tv.setLetterSpacing(0.08f);
        return tv;
    }

    /** Card background: dark, rounded, subtle border. */
    private android.graphics.drawable.GradientDrawable makeCardBg(int borderColor) {
        android.graphics.drawable.GradientDrawable bg =
                new android.graphics.drawable.GradientDrawable();
        bg.setColor(getColor(R.color.relay_panel));
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

    // ---- the beranda (the home screen) ----
    /** The beranda layer: the home screen with the state and the tools. */
    private View berandaView;
    private GridLayout shortcutGrid;
    private TextView berandaState;
    private TextView berandaLatency;
    private TextView berandaDevice;
    private TextView berandaPcName;
    private TextView berandaAddress;
    private TextView berandaRole;
    private TextView berandaError;
    private View berandaDot;
    private View berandaReconnect;
    private View berandaTakeover;
    /** The remote view is in front of the beranda (the user opened it). */
    private boolean remoteOpen = false;
    /**
     * The relay does not know this device (or the code was refused):
     * the connect card is the screen until the pairing worked.
     */
    private boolean pairingRequired = false;
    /**
     * The user opened the connect card on purpose (to fix the
     * address). It closes again when the pairing worked.
     */
    private boolean connectCardOpen = false;
    /** The name of the PC we drive (the relay sends it with the welcome). */
    private String peerHost = "";

    // ---- home (beranda) ----
    /** The last connection failure, shown next to the Connect button. */
    private String lastConnError = "";
    /** The kind of the last failure (WsClient.Failure.*). */
    private int lastConnErrorKind = WsClient.Failure.UNKNOWN;
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

    // The tools of the beranda: icon, label, action. The first tile
    // opens the remote view - that is the way into the browser.
    private static final String[] BERANDA_TOOL_LABELS = {
            "Remote", "Back", "Forward", "Reload",
            "Home", "Find", "Keyboard", "Paste",
            "Copy link", "Zoom", "Settings", "Disconnect"
    };
    private static final int[] BERANDA_TOOL_ICONS = {
            R.drawable.ic_desktop, R.drawable.ic_back, R.drawable.ic_forward,
            R.drawable.ic_reload, R.drawable.ic_home, R.drawable.ic_find,
            R.drawable.ic_keyboard, R.drawable.ic_paste, R.drawable.ic_link,
            R.drawable.ic_zoom, R.drawable.ic_settings, R.drawable.ic_link_off
    };
    private static final String[] BERANDA_TOOL_ACTIONS = {
            "remote", "back", "forward", "reload",
            "home", "find", "keyboard", "paste",
            "copylink", "zoom", "settings", "disconnect"
    };

    /** The status row at the top of the beranda. */
    private void buildBerandaStatus(LinearLayout col) {
        LinearLayout card = new LinearLayout(this);
        card.setOrientation(LinearLayout.HORIZONTAL);
        card.setGravity(Gravity.CENTER_VERTICAL);
        card.setPadding((int) dp(14), (int) dp(12), (int) dp(14), (int) dp(12));
        android.graphics.drawable.GradientDrawable bg =
                new android.graphics.drawable.GradientDrawable();
        bg.setColor(getColor(R.color.relay_panel));
        bg.setCornerRadius(dp(14));
        bg.setStroke((int) dp(1), getColor(R.color.relay_border));
        card.setBackground(bg);

        berandaDot = makeDot(getColor(R.color.relay_faint), 8);
        card.addView(berandaDot);

        LinearLayout texts = new LinearLayout(this);
        texts.setOrientation(LinearLayout.VERTICAL);
        LinearLayout.LayoutParams tlp = new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        tlp.setMarginStart((int) dp(10));
        texts.setLayoutParams(tlp);

        berandaState = new TextView(this);
        berandaState.setText("Not connected");
        berandaState.setTextSize(14);
        berandaState.setTextColor(getColor(R.color.relay_text));
        berandaState.setSingleLine(true);
        berandaState.setEllipsize(android.text.TextUtils.TruncateAt.END);
        texts.addView(berandaState);

        berandaDevice = new TextView(this);
        berandaDevice.setText(deviceName());
        berandaDevice.setTextSize(11.5f);
        berandaDevice.setTextColor(getColor(R.color.relay_muted));
        berandaDevice.setSingleLine(true);
        LinearLayout.LayoutParams dlp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        dlp.topMargin = (int) dp(2);
        berandaDevice.setLayoutParams(dlp);
        texts.addView(berandaDevice);

        card.addView(texts);

        berandaLatency = new TextView(this);
        berandaLatency.setText("— ms");
        berandaLatency.setTextSize(12);
        berandaLatency.setTextColor(getColor(R.color.relay_faint));
        card.addView(berandaLatency);

        LinearLayout.LayoutParams clp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        col.addView(card, clp);
    }

    /** The tile that opens the remote view (the beranda's main action). */
    private View buildRemoteTile() {
        TextView tile = new TextView(this);
        tile.setText("Open the remote view");
        tile.setTextSize(14);
        tile.setTextColor(getColor(R.color.relay_text));
        tile.setGravity(Gravity.CENTER);
        tile.setPadding((int) dp(14), (int) dp(16), (int) dp(14), (int) dp(16));
        android.graphics.drawable.GradientDrawable bg =
                new android.graphics.drawable.GradientDrawable();
        bg.setColor(getColor(R.color.relay_accent));
        bg.setCornerRadius(dp(12));
        tile.setBackground(bg);
        tile.setOnClickListener(v -> openRemote());
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.setMargins((int) dp(16), (int) dp(12), (int) dp(16), (int) dp(4));
        tile.setLayoutParams(lp);
        return tile;
    }

    /** One line of the connection info: a muted key and its value. */
    private void addInfoRow(LinearLayout card, String key, TextView value) {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(0, (int) dp(6), 0, (int) dp(6));
        TextView k = new TextView(this);
        k.setText(key);
        k.setTextSize(12);
        k.setTextColor(getColor(R.color.relay_faint));
        row.addView(k, new LinearLayout.LayoutParams(
                (int) dp(76), ViewGroup.LayoutParams.WRAP_CONTENT));
        value.setTextSize(13);
        value.setTextColor(getColor(R.color.relay_text));
        value.setSingleLine(true);
        value.setEllipsize(android.text.TextUtils.TruncateAt.END);
        row.addView(value, new LinearLayout.LayoutParams(
                0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        card.addView(row);
    }

    /** A small pill button (the web client's accent pill). */
    private View makePillButton(String label, int accent, View.OnClickListener onClick) {
        TextView btn = new TextView(this);
        btn.setText(label);
        btn.setTextSize(13);
        btn.setTextColor(accent);
        btn.setGravity(Gravity.CENTER);
        btn.setPadding((int) dp(14), (int) dp(11), (int) dp(14), (int) dp(11));
        android.graphics.drawable.GradientDrawable bg =
                new android.graphics.drawable.GradientDrawable();
        bg.setColor(getColor(R.color.relay_field));
        bg.setCornerRadius(dp(999));
        bg.setStroke((int) dp(1), accent);
        btn.setBackground(bg);
        btn.setOnClickListener(onClick);
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        lp.topMargin = (int) dp(6);
        btn.setLayoutParams(lp);
        return btn;
    }

    /** One tool tile of the beranda: a 56dp icon with a label under it. */
    private View makeToolTile(int iconRes, String label, String action) {
        LinearLayout tile = new LinearLayout(this);
        tile.setOrientation(LinearLayout.VERTICAL);
        tile.setGravity(Gravity.CENTER_HORIZONTAL);
        FrameLayout box = new FrameLayout(this);
        box.setBackgroundResource(R.drawable.tool_tile_bg);
        int size = (int) dp(56);
        box.setLayoutParams(new LinearLayout.LayoutParams(size, size));
        ImageView icon = new ImageView(this);
        icon.setImageResource(iconRes);
        icon.setLayoutParams(new FrameLayout.LayoutParams(
                (int) dp(26), (int) dp(26), Gravity.CENTER));
        box.addView(icon);
        tile.addView(box);
        TextView name = new TextView(this);
        name.setText(label);
        name.setTextSize(11);
        name.setTextColor(getColor(R.color.relay_muted));
        name.setGravity(Gravity.CENTER);
        name.setSingleLine(true);
        name.setEllipsize(android.text.TextUtils.TruncateAt.END);
        LinearLayout.LayoutParams nameLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        nameLp.topMargin = (int) dp(6);
        name.setLayoutParams(nameLp);
        tile.addView(name);
        tile.setOnClickListener(v -> onBerandaTool(action));
        return tile;
    }

    /** What a tool tile of the beranda does. */
    private void onBerandaTool(String action) {
        switch (action) {
            case "remote":
                openRemote();
                break;
            case "back":
                relay.sendHistory("back");
                openRemote();
                break;
            case "forward":
                relay.sendHistory("forward");
                openRemote();
                break;
            case "reload":
                relay.sendTabReload();
                openRemote();
                break;
            case "home":
                askUrl();
                break;
            case "find":
                askFind();
                break;
            case "keyboard":
                openKeyboard();
                break;
            case "paste":
                pasteFromPhone();
                break;
            case "copylink":
                copyActiveLink();
                break;
            case "zoom":
                soon("Zoom");
                break;
            case "settings":
                soon("Settings");
                break;
            case "disconnect":
                endSession();
                break;
            default:
                break;
        }
    }

    /** Copy the address of the open tab to the phone's clipboard. */
    private void copyActiveLink() {
        String url = "";
        for (RelayClient.TabInfo t : openTabs) {
            if (t.active && t.url != null && !t.url.isEmpty()) {
                url = t.url;
                break;
            }
        }
        if (url.isEmpty()) {
            Toast.makeText(this, "No page open yet", Toast.LENGTH_SHORT).show();
            return;
        }
        ClipboardManager cm = (ClipboardManager)
                getSystemService(Context.CLIPBOARD_SERVICE);
        if (cm != null) cm.setPrimaryClip(ClipData.newPlainText("url", url));
        Toast.makeText(this, "Link copied", Toast.LENGTH_SHORT).show();
    }

    /** End the session: the app goes back to the connect card. */
    private void endSession() {
        relay.disconnect();
        connected = false;
        lost = false;
        lastConnError = "";
        updateConnectError();
        closeSheet();
        pairingRequired = true;
        updateState();
    }

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
        urlText = findViewById(R.id.urlText);
        reloadBtn = findViewById(R.id.reloadBtn);
        kbd = findViewById(R.id.kbd);
        takeoverBtn = findViewById(R.id.takeoverBtn);
        statusBanner = findViewById(R.id.statusBanner);

        // The three states.
        connectCard = findViewById(R.id.connectCard);
        serverInput = findViewById(R.id.serverInput);
        codeInput = findViewById(R.id.codeInput);
        connectBtn = findViewById(R.id.connectBtn);
        connectError = findViewById(R.id.connectError);
        statusBar = findViewById(R.id.statusBar);
        statusText = findViewById(R.id.statusText);
        statusDot = findViewById(R.id.statusDot);
        lostBanner = findViewById(R.id.lostBanner);
        lostText = findViewById(R.id.lostText);
        reconnectBtn = findViewById(R.id.reconnectBtn);

        frameView.setInputSink(this);
        relay = new RelayClient(this);
        // View-only clients can ask for the wheel.
        takeoverBtn.setOnClickListener(v -> {
            relay.sendTakeover();
            Toast.makeText(this, "Taking over the remote view",
                    Toast.LENGTH_SHORT).show();
        });
        // One Connect button, one Reconnect button.
        connectBtn.setOnClickListener(v -> onConnectPressed());
        reconnectBtn.setOnClickListener(v -> {
            lastConnError = "";
            updateConnectError();
            userReconnect();
        });
        // The PC renders the page for the size we show: tell it when
        // that size changes.
        frameView.addOnLayoutChangeListener((v, l, t, r, b, ol, ot, or, ob) -> {
            if (r - l != or - ol || b - t != ob - ot) updateViewSize();
        });

        buildToolGrid();
        setupHandle();
        setupSheet();
        setupKeyboard();
        setupHome();

        handlePairIntent(getIntent());
        updateBrowserDown();
        // The beranda is the default screen. The connect card takes
        // over when this device is not paired yet.
        if (prefs.getString("br_host", null) == null) {
            pairingRequired = true;
        }
        showConnectState();
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
        // The PC has to know how big our view is; the layout listener
        // covers later changes.
        updateViewSize();
    }

    @Override
    protected void onPause() {
        super.onPause();
        // Going to the background is not a lost connection: the app
        // reconnects when it comes back, and the user must not be shown
        // the "connection lost" banner for it.
        relay.disconnect();
        handler.removeCallbacks(idleRunnable);
        handler.removeCallbacks(roundRunnable);
    }

    @Override
    protected void onDestroy() {
        // The websocket must not outlive the activity: a live socket
        // keeps the reader thread (and the phone's battery) awake.
        relay.disconnect();
        handler.removeCallbacks(idleRunnable);
        handler.removeCallbacks(roundRunnable);
        super.onDestroy();
    }

    @Override
    public void onBackPressed() {
        if (sheetOpen) {
            closeSheet();
        } else if (remoteOpen) {
            // The back button brings the beranda back.
            closeRemote();
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
        String host = data.getQueryParameter("host"); // e.g. http://192.168.1.5:8765
        if (code == null || !code.matches("\\d{6}")) return false;
        ServerAddress addr = ServerAddress.parse(host);
        if (!addr.isValid()) {
            // The link carried an address we cannot use. Say so instead
            // of failing later with a bare "EOFException".
            lastConnError = addr.error;
            lastConnErrorKind = WsClient.Failure.UNKNOWN;
            updateConnectError();
            return false;
        }
        prefs.edit().putString("br_host", addr.display())
                .putString("br_token", code).apply();
        // Fill the card, so the user can see what the link carried.
        if (serverInput != null) serverInput.setText(addr.display());
        if (codeInput != null) codeInput.setText(code);
        return true;
    }

    private void connect() {
        String hostPort = prefs.getString("br_host", null);
        if (hostPort == null) {
            // No saved host: show the connect card and wait for the user.
            pairingRequired = true;
            showConnectState();
            return;
        }
        ServerAddress addr = ServerAddress.parse(hostPort);
        if (!addr.isValid()) {
            // A saved address we cannot use: say why, and show the card
            // so the user can fix it.
            lastConnError = addr.error;
            lastConnErrorKind = WsClient.Failure.UNKNOWN;
            updateConnectError();
            pairingRequired = true;
            showConnectState();
            return;
        }
        // The pairing code is single use: the relay burns it. It is sent
        // with the first auth only, so a spent code is never sent twice.
        String token = prefs.getString("br_token", "");
        String fp = getFingerprint();
        Log.i(TAG, "connecting to " + addr.wsUrl());
        relay.connect(addr.wsUrl(), token, fp, deviceName());
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

    // ============================== RelayClient.Listener ==============================

    @Override
    public void onWelcome(boolean driver, boolean browserUp, boolean mobile,
                          String host) {
        connected = true;
        lost = false;
        // The pairing worked: the beranda is the screen now.
        boolean wasPairing = pairingRequired;
        pairingRequired = false;
        connectCardOpen = false;
        // The pairing lands on the beranda, not on the remote view.
        // A later reconnect keeps the screen the user was on.
        if (wasPairing) remoteOpen = false;
        this.browserUp = browserUp;
        this.mobile = mobile;
        if (host != null && !host.isEmpty()) peerHost = host;
        // The pairing code is burned by the relay: forget it, so the next
        // connect authenticates with the fingerprint alone.
        prefs.edit().remove("br_token").apply();
        serverNote = "";
        lastServerNote = "";
        roleLabel = driver ? "Driving Chrome" : "View only";
        updateBrowserDown();
        updateState();
        updateDesktopTool();
        updateTakeover();
        updateStatusBanner();
        Log.i(TAG, "welcome driver=" + driver + " browserUp=" + browserUp);
        // The PC renders the page for whoever drives: tell it our size.
        updateViewSize();
    }

    @Override
    public void onStatus(boolean browserUp) {
        this.browserUp = browserUp;
        updateBrowserDown();
        updateStatusBanner();
    }

    @Override
    public void onPaused(boolean paused) {
        this.paused = paused;
        updateStatusBanner();
        updateState();
    }

    @Override
    public void onFlagsOk(boolean flagsOk) {
        this.flagsOk = flagsOk;
        updateStatusBanner();
        updateState();
    }

    private long lastServerNoteAt = 0;
    private String lastServerNote = "";

    @Override
    public void onServerError(String msg) {
        // The relay refused something we asked for (for example
        // "view-only: another client is driving"). Say it out loud
        // instead of dropping it into the log. The same message can
        // arrive over and over, so only say it once in a while.
        this.serverNote = friendlyServerError(msg);
        updateStatusBanner();
        long now = System.currentTimeMillis();
        if (!this.serverNote.equals(lastServerNote)
                || now - lastServerNoteAt > 15000) {
            lastServerNote = this.serverNote;
            lastServerNoteAt = now;
            Toast.makeText(this, this.serverNote, Toast.LENGTH_LONG).show();
        }
    }

    @Override
    public void onBadToken() {
        // The pairing code was spent or wrong. The relay retries with
        // the fingerprint alone (this device may already be paired).
        prefs.edit().remove("br_token").apply();
        updateState();
        Toast.makeText(this, "That pairing code was already used — "
                + "reconnecting with this device", Toast.LENGTH_LONG).show();
    }

    /** What the relay told us about the page, in one line. */
    private String statusNote() {
        if (!flagsOk) {
            return " — Chrome runs without the flags that keep it painting";
        }
        if (paused) {
            return " — the page is in the background";
        }
        return "";
    }

    /** The status strip: what the relay told us about the page. */
    private void updateStatusBanner() {
        if (statusBanner == null) return;
        String msg = "";
        if (!flagsOk) {
            msg = "Chrome runs without the flags that keep it painting — "
                    + "the screen may freeze.";
        } else if (paused) {
            msg = "The page is in the background — the picture is frozen.";
        }
        if (!serverNote.isEmpty()) {
            msg = msg.isEmpty() ? serverNote : msg + "  " + serverNote;
        }
        statusBanner.setText(msg);
        statusBanner.setVisibility(msg.isEmpty() ? View.GONE : View.VISIBLE);
    }

    /** The take-over button only makes sense while we are watching. */
    private void updateTakeover() {
        if (takeoverBtn == null) return;
        boolean show = connected && !relay.isDriver();
        takeoverBtn.setVisibility(show ? View.VISIBLE : View.GONE);
    }

    /** A relay message the user can do something about. */
    static String friendlyServerError(String msg) {
        if (msg == null || msg.isEmpty()) return "The relay refused the request";
        String m = msg.toLowerCase();
        if (m.contains("view-only")) {
            return "Another device is driving — tap Take over to switch";
        }
        if (m.contains("browser unavailable")) {
            return "Chrome is not running on the PC";
        }
        if (m.contains("mouse failed") || m.contains("wheel failed")
                || m.contains("key failed")) {
            return "The PC did not accept that input";
        }
        return msg;
    }

    /**
     * Tell the PC how big our view is, so the picture arrives 1:1.
     * The size is in CSS pixels (dp on Android) and the zoom of our own
     * picture is not implemented yet, so z is 1.
     */
    private void updateViewSize() {
        if (relay == null || frameView == null) return;
        int w = frameView.getWidth();
        int h = frameView.getHeight();
        if (w <= 0 || h <= 0) return;
        float density = getResources().getDisplayMetrics().density;
        if (density <= 0) density = 1f;
        int cssW = Math.max(1, Math.round(w / density));
        int cssH = Math.max(1, Math.round(h / density));
        relay.setViewSize(cssW, cssH, density, 1f);
    }

    @Override
    public void onLatency(long ms) {
        latencyText.setText(ms + " ms");
        if (berandaLatency != null) berandaLatency.setText(ms + " ms");
        int color;
        if (ms < 120) {
            color = getColor(R.color.relay_green);
        } else if (ms < 350) {
            color = getColor(R.color.relay_yellow);
        } else {
            color = getColor(R.color.relay_red);
        }
        statusDot.setBackgroundTintList(ColorStateList.valueOf(color));
    }

    @Override
    public void onRole(boolean driver) {
        roleLabel = driver ? "Driving Chrome" : "View only";
        updateTakeover();
        updateState();
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
            if (shortcutGrid != null) renderHome();
            renderSheetShortcuts();
        });
    }

    @Override
    public void onClipboard(String text) {
        ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        if (cm != null) {
            cm.setPrimaryClip(ClipData.newPlainText("relay", text));
            Toast.makeText(this, "Laptop clipboard copied", Toast.LENGTH_SHORT).show();
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
                count + (count == 1 ? " match" : " matches"), Toast.LENGTH_SHORT).show());
    }

    @Override
    public void onChrome(String action) {
        // The welcome/status broadcast hides the browserDown overlay
        // once Chrome is up; nothing else to do here.
    }

    @Override
    public void onAuthError(String msg) {
        // The reason goes next to the Connect button, where it stays
        // readable. A toast is gone before the user can act on it.
        setAuthError(msg);
        if (msg != null && msg.contains("revoked")) {
            // The PC forgot this device: the pairing code is worthless.
            prefs.edit().remove("br_token").apply();
        }
        runOnUiThread(() -> {
            connected = false;
            lost = false;
            // A new code is needed: the field is cleared so the user
            // types the one the panel shows now.
            if (lastConnErrorKind == WsClient.Failure.NOT_PAIRED
                    && codeInput != null) {
                codeInput.setText("");
            }
            updateState();
        });
    }

    @Override
    public void onDisconnected() {
        boolean hadSession = connected;
        connected = false;
        // A session that existed and is gone is "lost": the user gets
        // the banner with a Reconnect button. A connection that never
        // came up is "not connected": the user gets the card with the
        // reason on it.
        lost = hadSession;
        // A lost session brings the remote view forward: the lost
        // banner lives there.
        if (lost) remoteOpen = true;
        runOnUiThread(() -> {
            latencyText.setText("— ms");
            statusDot.setBackgroundTintList(
                    ColorStateList.valueOf(getColor(R.color.relay_faint)));
            updateBrowserDown();
            updateTakeover();
            updateState();
        });
    }

    @Override
    public void onConnError(String msg, int kind) {
        // The reason is shown next to the Connect button, not in a
        // toast that disappears before it can be read.
        lastConnError = msg == null ? "" : msg.trim();
        lastConnErrorKind = kind;
        Log.w(TAG, "conn error: " + lastConnError);
        runOnUiThread(() -> {
            updateConnectError();
            updateState();
        });
    }

    @Override
    public void onRetriesExhausted() {
        // The automatic reconnect gave up: the banner with the
        // Reconnect button is the way out. No silent endless retry.
        runOnUiThread(() -> {
            updateStatusBanner();
            updateState();
        });
    }

    /** Manual reconnect (the Reconnect button in the banner). */
    private void userReconnect() {
        lost = false;
        runOnUiThread(() -> {
            latencyText.setText("…");
            lastConnError = "";
            updateConnectError();
            updateState();
        });
        connect();
    }

    private void updateBrowserDown() {
        browserDown.setVisibility(connected && !browserUp ? View.VISIBLE : View.GONE);
    }

    // ============================== FrameView.InputSink ==============================

    @Override
    public boolean canDrive() {
        return relay.isDriver();
    }

    @Override
    public void mouse(String action, float x, float y, String button, int iw, int ih) {
        relay.sendMouse(action, x, y, button, iw, ih);
    }

    @Override
    public void wheel(float x, float y, float dx, float dy, int iw, int ih) {
        relay.sendWheel(x, y, dx, dy, iw, ih);
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
            // The Home button brings the beranda back (the web client
            // does the same: the sheet closes and the home screen opens).
            closeSheet();
            closeRemote();
        });
        findViewById(R.id.launchChromeBtn).setOnClickListener(v -> {
            relay.sendChrome("launch");
            Toast.makeText(this, "Chrome launching…", Toast.LENGTH_SHORT).show();
        });
        // End the session: the app goes back to the connect card.
        findViewById(R.id.disconnectBtn).setOnClickListener(v -> endSession());
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
                    Toast.makeText(this, "Chrome isn't running on the laptop — start it first",
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
        Toast.makeText(this, what + " — coming soon", Toast.LENGTH_SHORT).show();
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

    // ============================== the beranda ==============================

    /**
     * The beranda is the home screen of the phone: the state of the
     * connection, the tools and the shortcuts. The remote view is one
     * tap away (the "Remote" tile).
     */
    private void setupHome() {
        android.widget.ScrollView scroll = new android.widget.ScrollView(this);
        scroll.setBackgroundColor(getColor(R.color.relay_bg));
        scroll.setVisibility(View.GONE);
        FrameLayout.LayoutParams slp = new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT);
        scroll.setLayoutParams(slp);

        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        scroll.addView(col, new ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        int hp = (int) dp(16);

        // ---- the header (the web client's home header) ----
        LinearLayout header = new LinearLayout(this);
        header.setOrientation(LinearLayout.HORIZONTAL);
        header.setGravity(Gravity.CENTER_VERTICAL);
        header.setPadding(hp, (int) dp(14), hp, (int) dp(12));
        LinearLayout titles = new LinearLayout(this);
        titles.setOrientation(LinearLayout.VERTICAL);
        LinearLayout.LayoutParams tlp = new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        titles.setLayoutParams(tlp);
        TextView h1 = new TextView(this);
        h1.setText("Home");
        h1.setTextSize(20);
        h1.setTextColor(getColor(R.color.relay_text));
        h1.setTypeface(null, android.graphics.Typeface.BOLD);
        titles.addView(h1);
        TextView h2 = new TextView(this);
        h2.setText("Tools and the PC connection");
        h2.setTextSize(12);
        h2.setTextColor(getColor(R.color.relay_muted));
        titles.addView(h2);
        header.addView(titles);
        col.addView(header);

        // ---- the state of the connection ----
        buildBerandaStatus(col);

        // ---- the tools ----
        col.addView(makeSectionLabel("TOOLS"));
        GridLayout tools = new GridLayout(this);
        tools.setColumnCount(4);
        for (int i = 0; i < BERANDA_TOOL_ACTIONS.length; i++) {
            View tile = makeToolTile(BERANDA_TOOL_ICONS[i], BERANDA_TOOL_LABELS[i],
                    BERANDA_TOOL_ACTIONS[i]);
            GridLayout.LayoutParams lp = new GridLayout.LayoutParams(
                    GridLayout.spec(i / 4), GridLayout.spec(i % 4, 1f));
            lp.width = 0;
            lp.setMargins((int) dp(3), (int) dp(3), (int) dp(3), (int) dp(3));
            tile.setLayoutParams(lp);
            tools.addView(tile);
        }
        LinearLayout.LayoutParams toolsLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        toolsLp.setMargins(hp, (int) dp(8), hp, (int) dp(16));
        tools.setLayoutParams(toolsLp);
        col.addView(tools);

        // ---- the connection info ----
        col.addView(makeSectionLabel("CONNECTION"));
        LinearLayout card = new LinearLayout(this);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setPadding((int) dp(14), (int) dp(6), (int) dp(14), (int) dp(6));
        card.setBackground(makeCardBg(getColor(R.color.relay_border)));
        berandaPcName = new TextView(this);
        berandaAddress = new TextView(this);
        berandaRole = new TextView(this);
        addInfoRow(card, "PC", berandaPcName);
        addInfoRow(card, "Address", berandaAddress);
        addInfoRow(card, "Role", berandaRole);
        berandaError = new TextView(this);
        berandaError.setTextSize(12);
        berandaError.setTextColor(getColor(R.color.relay_red));
        berandaError.setVisibility(View.GONE);
        LinearLayout.LayoutParams errLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        errLp.topMargin = (int) dp(6);
        berandaError.setLayoutParams(errLp);
        card.addView(berandaError);

        // Take over (when we only watch) and Reconnect (when the link
        // died) sit at the bottom of the card: one thumb reaches them.
        berandaTakeover = makePillButton("Take over", getColor(R.color.relay_accent),
                v -> {
                    relay.sendTakeover();
                    Toast.makeText(this, "Taking over the remote view",
                            Toast.LENGTH_SHORT).show();
                });
        berandaReconnect = makePillButton("Reconnect", getColor(R.color.relay_accent),
                v -> {
                    lastConnError = "";
                    updateConnectError();
                    userReconnect();
                });
        card.addView(berandaTakeover);
        card.addView(berandaReconnect);
        LinearLayout.LayoutParams cardLp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        cardLp.setMargins(hp, (int) dp(8), hp, (int) dp(20));
        card.setLayoutParams(cardLp);
        col.addView(card);

        // ---- the shortcuts the user saved ----
        LinearLayout secRow = new LinearLayout(this);
        secRow.setOrientation(LinearLayout.HORIZONTAL);
        secRow.setGravity(Gravity.CENTER_VERTICAL);
        secRow.setPadding(hp, 0, hp, 0);
        TextView sec = makeSectionLabel("SHORTCUTS");
        LinearLayout.LayoutParams secLp = new LinearLayout.LayoutParams(0,
                ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        sec.setLayoutParams(secLp);
        secRow.addView(sec);
        final android.widget.ImageButton editBtn = new android.widget.ImageButton(this);
        editBtn.setImageResource(R.drawable.ic_pencil);
        editBtn.setScaleType(android.widget.ImageView.ScaleType.CENTER_INSIDE);
        editBtn.setBackground(null);
        editBtn.setContentDescription("Edit shortcuts");
        int ebs = (int) dp(44);
        editBtn.setLayoutParams(new LinearLayout.LayoutParams(ebs, ebs));
        editBtn.setOnClickListener(v -> {
            homeEditing = !homeEditing;
            editBtn.setImageResource(homeEditing ? R.drawable.ic_check : R.drawable.ic_pencil);
            renderHome();
        });
        secRow.addView(editBtn);
        col.addView(secRow);

        shortcutGrid = new GridLayout(this);
        shortcutGrid.setColumnCount(4);
        LinearLayout.LayoutParams glp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        glp.setMargins(hp, (int) dp(8), hp, (int) dp(24));
        shortcutGrid.setLayoutParams(glp);
        col.addView(shortcutGrid);

        berandaView = scroll;
        // Above the remote screen, below the connect card.
        root.addView(berandaView, 1);
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

    /**
     * The one state on screen: the connect card (the device is not
     * paired yet), the beranda (the default screen) or the remote view
     * (the user opened it from the beranda).
     *
     * <p>Exactly one of them is in front. The status information lives
     * in one place - the status bar of the remote view, and the status
     * area of the beranda.
     */
    private void updateState() {
        runOnUiThread(() -> {
            boolean wasConnectCard = connectCard != null
                    && connectCard.getVisibility() == View.VISIBLE;
            // The connect card is the screen while the device is not
            // paired, or while the user asked for it.
            boolean showConnect = pairingRequired || connectCardOpen;
            // The remote view is alive as soon as there is a session.
            boolean showRemote = connected || lost;

            if (connectCard != null) {
                connectCard.setVisibility(showConnect ? View.VISIBLE : View.GONE);
            }
            if (statusBar != null) {
                statusBar.setVisibility(showRemote ? View.VISIBLE : View.GONE);
            }
            if (lostBanner != null) {
                lostBanner.setVisibility(lost ? View.VISIBLE : View.GONE);
            }
            // The remote view only shows once there is a session.
            if (frameView != null) {
                frameView.setVisibility(showRemote ? View.VISIBLE : View.INVISIBLE);
            }
            // The beranda is the default screen: it covers the remote
            // view until the user opens the remote view from it.
            if (berandaView != null) {
                boolean berandaOn = !showConnect && !remoteOpen;
                berandaView.setVisibility(berandaOn ? View.VISIBLE : View.GONE);
            }
            // The edge handle belongs to the remote view (the web
            // client hides it on the home screen too).
            if (handle != null) {
                boolean handleOn = remoteOpen && showRemote;
                handle.setVisibility(handleOn ? View.VISIBLE : View.GONE);
            }
            updateBeranda();

            if (statusText != null) {
                if (connected) {
                    String host = prefs.getString("br_host", "");
                    String t = roleLabel.isEmpty() ? "Connected" : roleLabel;
                    if (!host.isEmpty()) t += " · " + host;
                    statusText.setText(t + statusNote());
                } else if (lost) {
                    statusText.setText("Not connected");
                }
            }
            if (statusDot != null) {
                int color = connected ? 0xFF3FB950 : 0xFF8A8A8A;
                statusDot.setBackgroundTintList(ColorStateList.valueOf(color));
            }
            if (lostText != null) {
                lostText.setText(lastConnError.isEmpty()
                        ? "The connection to the PC was lost."
                        : lastConnError);
            }
            // The connect card is pre-filled with the address that
            // worked last.
            if (showConnect && serverInput != null
                    && serverInput.getText().length() == 0) {
                serverInput.setText(prefs.getString("br_host", ""));
            }
            // Dim shortcuts when there is no session to drive.
            if (shortcutGrid != null) {
                shortcutGrid.setAlpha(connected ? 1f : 0.35f);
            }
            if (wasConnectCard && showConnect) {
                // The card just came back: the keyboard may be in the way.
                serverInput.requestFocus();
            }
        });
    }

    /** The not-connected state: the connect card is the screen. */
    private void showConnectState() {
        connected = false;
        lost = false;
        updateState();
    }

    /**
     * Open the remote view: the user drives the browser from here.
     * The back button brings the beranda back.
     */
    private void openRemote() {
        if (!connected) {
            // Nothing to drive yet: the connect card says why.
            updateState();
            return;
        }
        remoteOpen = true;
        updateState();
        if (firstWelcome) {
            // The handle on the edge is how the tools open: say so once,
            // the first time the user is in the remote view.
            firstWelcome = false;
            hint.setVisibility(View.VISIBLE);
            hint.setAlpha(1f);
            hint.setOnClickListener(v -> hint.setVisibility(View.GONE));
            handler.postDelayed(() -> hint.animate().alpha(0f).setDuration(400)
                    .withEndAction(() -> hint.setVisibility(View.GONE)), 6000);
        }
    }

    /** Back to the beranda. */
    private void closeRemote() {
        remoteOpen = false;
        updateState();
    }

    /**
     * The beranda shows the state of the connection: the state, the
     * round trip time, the device name and the connection info.
     */
    private void updateBeranda() {
        if (berandaView == null) return;
        String host = prefs.getString("br_host", "");
        if (berandaState != null) {
            String t;
            if (connected) {
                t = roleLabel.isEmpty() ? "Connected" : roleLabel;
                if (!host.isEmpty()) t += " · " + host;
            } else if (lost) {
                t = "Connection lost";
            } else if (pairingRequired) {
                t = "Not paired";
            } else {
                t = "Connecting…";
            }
            berandaState.setText(t);
        }
        if (berandaDot != null) {
            int color;
            if (connected) {
                color = getColor(R.color.relay_green);
            } else if (lost) {
                color = getColor(R.color.relay_red);
            } else {
                color = getColor(R.color.relay_faint);
            }
            berandaDot.setBackgroundTintList(ColorStateList.valueOf(color));
        }
        if (berandaDevice != null) {
            // The name of the PC we drive, and this phone below it.
            berandaDevice.setText(peerHost.isEmpty() ? deviceName() : peerHost);
        }
        if (berandaPcName != null) {
            berandaPcName.setText(peerHost.isEmpty() ? "—" : peerHost);
        }
        if (berandaAddress != null) {
            berandaAddress.setText(host.isEmpty() ? "—" : host);
        }
        if (berandaRole != null) {
            berandaRole.setText(roleLabel.isEmpty() ? "—" : roleLabel);
        }
        if (berandaError != null) {
            boolean show = !connected && !lastConnError.isEmpty();
            berandaError.setVisibility(show ? View.VISIBLE : View.GONE);
            berandaError.setText(lastConnError);
        }
        if (berandaTakeover != null) {
            // Take over only makes sense while we only watch.
            berandaTakeover.setVisibility(
                    connected && !relay.isDriver() ? View.VISIBLE : View.GONE);
        }
        if (berandaReconnect != null) {
            berandaReconnect.setVisibility(
                    !connected ? View.VISIBLE : View.GONE);
        }
    }

    /** The line under the Connect button: what is wrong, or what to do. */
    private void updateConnectError() {
        if (connectError == null) return;
        if (lastConnError == null || lastConnError.isEmpty()) {
            connectError.setVisibility(View.GONE);
            connectError.setText("");
        } else {
            connectError.setText(lastConnError);
            connectError.setVisibility(View.VISIBLE);
        }
    }

    /**
     * The relay refused us. Turn its answer into one line the user
     * can act on: the code was wrong, or this phone is not paired yet.
     */
    private void setAuthError(String msg) {
        String m = msg == null ? "" : msg.toLowerCase();
        if (m.contains("unknown device")) {
            lastConnError = "this phone is not paired yet - enter the "
                    + "6 digit code from the PC panel";
            lastConnErrorKind = WsClient.Failure.NOT_PAIRED;
            // The device is not paired: the connect card is the screen.
            pairingRequired = true;
        } else if (m.contains("bad token") || m.contains("invalid token")
                || m.contains("expired")) {
            lastConnError = "that code was refused - enter the current "
                    + "6 digit code from the PC panel";
            lastConnErrorKind = WsClient.Failure.BAD_CODE;
            pairingRequired = true;
        } else {
            lastConnError = msg == null ? "the relay refused the connection" : msg;
            lastConnErrorKind = WsClient.Failure.UNKNOWN;
            pairingRequired = true;
        }
        updateConnectError();
    }

    /** The Connect button on the card. */
    private void onConnectPressed() {
        ServerAddress addr = ServerAddress.parse(serverInput.getText().toString());
        String code = codeInput.getText().toString().trim();
        if (!addr.isValid()) {
            // The address is wrong: say why, next to the field.
            lastConnError = addr.error;
            lastConnErrorKind = WsClient.Failure.UNKNOWN;
            updateConnectError();
            return;
        }
        if (!code.matches("\\d{6}")) {
            lastConnError = "the pairing code is 6 digits - read it from the PC panel";
            lastConnErrorKind = WsClient.Failure.BAD_CODE;
            updateConnectError();
            return;
        }
        prefs.edit().putString("br_host", addr.display())
                .putString("br_token", code).apply();
        lastConnError = "";
        updateConnectError();
        connect();
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
        shortcutGrid.removeAllViews();
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
            icon.setTextColor(getColor(R.color.relay_text));
            icon.setGravity(Gravity.CENTER);
            android.graphics.drawable.GradientDrawable bg =
                    new android.graphics.drawable.GradientDrawable();
            try {
                bg.setColor(Color.parseColor(s[2]));
            } catch (Exception e) {
                bg.setColor(getColor(R.color.relay_accent));
            }
            bg.setCornerRadius(dp(14));
            // Highlight if this shortcut's tab is open in Chrome.
            boolean tabOpen = isTabOpen(s[1]);
            if (tabOpen) {
                bg.setStroke((int) dp(2), getColor(R.color.relay_accent));
            }
            icon.setBackground(bg);
            LinearLayout.LayoutParams ilp = new LinearLayout.LayoutParams(
                    (int) dp(56), (int) dp(56));
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
                dotBg.setColor(getColor(R.color.relay_green));
                dotBg.setStroke((int) dp(1), getColor(R.color.relay_bg));
                dot.setBackground(dotBg);
                FrameLayout.LayoutParams dotLp = new FrameLayout.LayoutParams(
                        (int) dp(12), (int) dp(12), Gravity.TOP | Gravity.END);
                iconWrap.addView(dot, dotLp);
            }
            tile.addView(iconWrap);

            TextView label = new TextView(this);
            label.setText(s[0]);
            label.setTextSize(11);
            label.setTextColor(getColor(R.color.relay_text));
            label.setGravity(Gravity.CENTER);
            label.setSingleLine(true);
            label.setEllipsize(android.text.TextUtils.TruncateAt.END);
            LinearLayout.LayoutParams llp = new LinearLayout.LayoutParams(
                    (int) dp(76), ViewGroup.LayoutParams.WRAP_CONTENT);
            llp.topMargin = (int) dp(6);
            label.setLayoutParams(llp);
            tile.addView(label);
            // "open" label for open tabs.
            if (tabOpen) {
                TextView openLabel = new TextView(this);
                openLabel.setText("● open");
                openLabel.setTextSize(9);
                openLabel.setTextColor(getColor(R.color.relay_green));
                openLabel.setGravity(Gravity.CENTER);
                tile.addView(openLabel);
            }

            if (homeEditing) {
                TextView del = new TextView(this);
                del.setText("×");
                del.setTextSize(15);
                del.setTextColor(getColor(R.color.relay_text));
                del.setGravity(Gravity.CENTER);
                android.graphics.drawable.GradientDrawable dbg =
                        new android.graphics.drawable.GradientDrawable();
                dbg.setColor(getColor(R.color.relay_red));
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
                    Toast.makeText(this, "Shortcut removed", Toast.LENGTH_SHORT).show();
                });
                tile.setOnClickListener(null);
                GridLayout.LayoutParams wlp = new GridLayout.LayoutParams(
                        GridLayout.spec(i / 4), GridLayout.spec(i % 4, 1f));
                wlp.width = 0;
                wrapper.setLayoutParams(wlp);
                shortcutGrid.addView(wrapper);
            } else {
                tile.setOnClickListener(v -> {
                    // Gate 2: Chrome must be running.
                    if (!browserUp) {
                        updateBrowserDown();
                        Toast.makeText(this,
                                "Chrome isn't running on the laptop — start it first",
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
                    // The shortcut opened a page: show it.
                    openRemote();
                });
                shortcutGrid.addView(tile);
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
            plus.setTextColor(getColor(R.color.relay_muted));
            plus.setGravity(Gravity.CENTER);
            android.graphics.drawable.GradientDrawable pbg =
                    new android.graphics.drawable.GradientDrawable();
            pbg.setStroke((int) dp(1.5f), getColor(R.color.relay_border));
            pbg.setCornerRadius(dp(16));
            plus.setBackground(pbg);
            plus.setLayoutParams(new LinearLayout.LayoutParams((int) dp(60), (int) dp(60)));
            add.addView(plus);
            TextView alabel = new TextView(this);
            alabel.setText("Add");
            alabel.setTextSize(11.5f);
            alabel.setTextColor(getColor(R.color.relay_text));
            alabel.setGravity(Gravity.CENTER);
            LinearLayout.LayoutParams allp = new LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT);
            allp.topMargin = (int) dp(8);
            alabel.setLayoutParams(allp);
            add.addView(alabel);
            add.setOnClickListener(v -> askAddShortcut());
            shortcutGrid.addView(add);
        }
    }

    private void askAddShortcut() {
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setPadding((int) dp(4), 0, (int) dp(4), 0);
        final EditText nameIn = new EditText(this);
        nameIn.setHint("Name (e.g. Facebook)");
        final EditText urlIn = new EditText(this);
        urlIn.setHint("URL (e.g. facebook.com)");
        urlIn.setInputType(android.text.InputType.TYPE_TEXT_VARIATION_URI);
        col.addView(nameIn);
        col.addView(urlIn);
        new AlertDialog.Builder(this)
                .setTitle("Add shortcut")
                .setView(col)
                .setPositiveButton("Save", (d, w) -> {
                    String name = nameIn.getText().toString().trim();
                    String url = urlIn.getText().toString().trim()
                            .replaceFirst("^https?://", "").replaceAll("/$", "");
                    if (name.isEmpty()) {
                        Toast.makeText(this, "Enter a name first", Toast.LENGTH_SHORT).show();
                        return;
                    }
                    if (url.isEmpty() || !url.contains(".")) {
                        Toast.makeText(this, "Invalid URL", Toast.LENGTH_SHORT).show();
                        return;
                    }
                    java.util.List<String[]> l = loadHome();
                    l.add(new String[]{name, url, "#8B72F0"});
                    saveHome(l);
                    renderHome();
                    Toast.makeText(this, "Shortcut saved", Toast.LENGTH_SHORT).show();
                })
                .setNegativeButton("Cancel", null)
                .show();
    }

    private void askUrl() {
        // Gate 2: Chrome must be running before opening URLs.
        if (!browserUp) {
            updateBrowserDown();
            Toast.makeText(this, "Chrome isn't running on the laptop — start it first",
                    Toast.LENGTH_LONG).show();
            return;
        }
        final EditText input = new EditText(this);
        input.setInputType(android.text.InputType.TYPE_TEXT_VARIATION_URI);
        input.setText("https://");
        input.setSelection(input.getText().length());
        new AlertDialog.Builder(this)
                .setTitle("Open URL")
                .setView(input)
                .setPositiveButton("Open", (d, w) -> {
                    String u = input.getText().toString().trim();
                    if (!u.isEmpty()) relay.sendTabNew(u);
                })
                .setNegativeButton("Cancel", null)
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
                soon("Settings");
                break;
            default:
                break;
        }
    }

    private void askFind() {
        final EditText input = new EditText(this);
        input.setHint("Search on page…");
        new AlertDialog.Builder(this)
                .setTitle("Find in page")
                .setView(input)
                .setPositiveButton("Search", (d, w) -> {
                    String q = input.getText().toString().trim();
                    if (!q.isEmpty()) relay.sendFind(q);
                })
                .setNegativeButton("Cancel", null)
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
            Toast.makeText(this, "The phone clipboard is empty", Toast.LENGTH_SHORT).show();
            return;
        }
        relay.sendClipboardSet(text.toString());
        Toast.makeText(this, "Sent to the laptop", Toast.LENGTH_SHORT).show();
    }
}
