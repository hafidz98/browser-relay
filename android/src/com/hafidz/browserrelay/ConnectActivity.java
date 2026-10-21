package com.hafidz.browserrelay;

import android.app.Activity;
import android.content.Intent;
import android.content.SharedPreferences;
import android.os.Bundle;
import android.widget.Button;
import android.widget.EditText;
import android.widget.Toast;

import java.net.HttpURLConnection;
import java.net.URL;
import java.util.UUID;

import org.json.JSONObject;

/**
 * Manual server entry + pairing. Two flows:
 * - QR scan (bypasses this screen, handled in MainActivity).
 * - Request-code: enter host, tap "Request Pairing Code", read the
 *   single-use code off the PC panel, enter it, tap "Connect".
 * Fingerprint (br_fingerprint) is the stable device identity.
 */
public class ConnectActivity extends Activity {

    private Button connectBtn;
    private Button requestBtn;
    private EditText hostInput;
    private EditText tokenInput;
    private SharedPreferences prefs;

    // Pairing-request state: live WS used only to request + submit the code.
    private WsClient pairWs;
    private boolean pairRequested = false;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_connect);

        prefs = getSharedPreferences("browserrelay", MODE_PRIVATE);
        hostInput = findViewById(R.id.hostInput);
        tokenInput = findViewById(R.id.tokenInput);
        connectBtn = findViewById(R.id.connectBtn);
        requestBtn = findViewById(R.id.requestBtn);

        String savedHost = prefs.getString("br_host", "");
        hostInput.setText(savedHost);
        // Token field doubles as the pair-code input; don't preload a
        // single-use code (it's burned after one use).

        connectBtn.setOnClickListener(v -> onHubungkan());
        requestBtn.setOnClickListener(v -> onRequestCode());
    }

    private String fingerprint() {
        String fp = prefs.getString("br_fingerprint", null);
        if (fp == null) {
            fp = UUID.randomUUID().toString();
            prefs.edit().putString("br_fingerprint", fp).apply();
        }
        return fp;
    }

    private String deviceName() {
        return android.os.Build.MODEL != null ? android.os.Build.MODEL : "Android";
    }

    private String cleanHost() {
        return hostInput.getText().toString().trim()
                .replaceFirst("^https?://", "").replaceAll("/+$", "");
    }

    private void onHubungkan() {
        String host = cleanHost();
        String code = tokenInput.getText().toString().trim();
        if (host.isEmpty()) {
            Toast.makeText(this, "Isi server dulu (host:port)",
                    Toast.LENGTH_SHORT).show();
            return;
        }
        if (!code.matches("\\d{6}")) {
            Toast.makeText(this, "The code must be 6 digits", Toast.LENGTH_SHORT).show();
            return;
        }
        setLoading(true);
        // Test the connection on a background thread before saving.
        new Thread(() -> {
            boolean ok = probeRelay(host);
            runOnUiThread(() -> {
                setLoading(false);
                if (!ok) {
                    Toast.makeText(this,
                            "Server unreachable — check host:port and WiFi",
                            Toast.LENGTH_LONG).show();
                    return;
                }
                prefs.edit().putString("br_host", host)
                        .putString("br_token", code).apply();
                closePairWs();
                // MainActivity is singleTask: this brings it forward and it connects.
                Intent it = new Intent(this, MainActivity.class);
                it.addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP);
                startActivity(it);
                finish();
            });
        }).start();
    }

    private void onRequestCode() {
        String host = cleanHost();
        if (host.isEmpty()) {
            Toast.makeText(this, "Isi server dulu (host:port)",
                    Toast.LENGTH_SHORT).show();
            return;
        }
        requestBtn.setEnabled(false);
        requestBtn.setText("Meminta…");
        closePairWs();
        pairWs = new WsClient();
        String wsUrl = "ws://" + host + "/ws";
        String fp = fingerprint();
        String name = deviceName();
        pairWs.connect(wsUrl, new WsClient.Listener() {
            @Override
            public void onOpen() {
                try {
                    pairWs.send(new JSONObject()
                            .put("type", "pair_request")
                            .put("fingerprint", fp)
                            .put("name", name)
                            .toString());
                } catch (Exception e) {
                    onPairError("gagal kirim request");
                }
            }

            @Override
            public void onMessage(String text) {
                try {
                    JSONObject m = new JSONObject(text);
                    String t = m.optString("type", "");
                    if ("pair_wait".equals(t)) {
                        runOnUiThread(() -> {
                            pairRequested = true;
                            requestBtn.setText("Code requested");
                            Toast.makeText(ConnectActivity.this,
                                    "Check the PC panel — type the 6-digit code below, then connect",
                                    Toast.LENGTH_LONG).show();
                            tokenInput.requestFocus();
                        });
                    } else if ("error".equals(t)) {
                        onPairError(m.optString("message", "error"));
                    }
                } catch (Exception ignored) {}
            }

            @Override
            public void onClose() {}

            @Override
            public void onError(String msg) {
                onPairError(msg);
            }
        });
    }

    private void onPairError(String msg) {
        runOnUiThread(() -> {
            requestBtn.setEnabled(true);
            requestBtn.setText("Request Pairing Code");
            Toast.makeText(this, "Failed to request code: " + MainActivity.friendlyError(msg),
                    Toast.LENGTH_LONG).show();
        });
    }

    private void closePairWs() {
        if (pairWs != null) {
            pairWs.close();
            pairWs = null;
        }
        pairRequested = false;
    }

    @Override
    protected void onDestroy() {
        closePairWs();
        super.onDestroy();
    }

    private void setLoading(boolean loading) {
        connectBtn.setEnabled(!loading);
        connectBtn.setText(loading ? "Connecting…" : "Connect");
    }

    /** True when something HTTP-ish answers at the relay root. */
    private boolean probeRelay(String hostPort) {
        HttpURLConnection c = null;
        try {
            URL url = new URL("http://" + hostPort + "/");
            c = (HttpURLConnection) url.openConnection();
            c.setConnectTimeout(4000);
            c.setReadTimeout(4000);
            c.setRequestMethod("GET");
            int code = c.getResponseCode();
            return code >= 200 && code < 500;
        } catch (Exception e) {
            return false;
        } finally {
            if (c != null) c.disconnect();
        }
    }
}
