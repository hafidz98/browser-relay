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

/**
 * Manual server entry + pairing: enter the host, type the 6 digit
 * code from the PC panel, tap Connect.
 *
 * <p>Pairing happens two ways: this screen (type the code) or the
 * /pair link, which deep links into the app with the code.
 * Fingerprint (br_fingerprint) is the stable device identity.
 */
public class ConnectActivity extends Activity {

    private Button connectBtn;
    private EditText hostInput;
    private EditText tokenInput;
    private SharedPreferences prefs;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        setContentView(R.layout.activity_connect);

        prefs = getSharedPreferences("browserrelay", MODE_PRIVATE);
        hostInput = findViewById(R.id.hostInput);
        tokenInput = findViewById(R.id.tokenInput);
        connectBtn = findViewById(R.id.connectBtn);

        String savedHost = prefs.getString("br_host", "");
        hostInput.setText(savedHost);
        // Token field doubles as the pair-code input; don't preload a
        // single-use code (it's burned after one use).

        connectBtn.setOnClickListener(v -> onConnect());
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

    private void onConnect() {
        ServerAddress addr = ServerAddress.parse(hostInput.getText().toString());
        String code = tokenInput.getText().toString().trim();
        if (!addr.isValid()) {
            Toast.makeText(this, addr.error, Toast.LENGTH_LONG).show();
            return;
        }
        if (!code.matches("\\d{6}")) {
            Toast.makeText(this, "The code must be 6 digits", Toast.LENGTH_LONG).show();
            return;
        }
        setLoading(true);
        // Test the connection on a background thread before saving.
        new Thread(() -> {
            boolean ok = probeRelay(addr);
            runOnUiThread(() -> {
                setLoading(false);
                if (!ok) {
                    Toast.makeText(this,
                            "Server unreachable — check host:port and WiFi",
                            Toast.LENGTH_LONG).show();
                    return;
                }
                prefs.edit().putString("br_host", addr.display())
                        .putString("br_token", code).apply();
                // MainActivity is singleTask: this brings it forward and it connects.
                Intent it = new Intent(this, MainActivity.class);
                it.addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP);
                startActivity(it);
                finish();
            });
        }).start();
    }

    private void setLoading(boolean loading) {
        connectBtn.setEnabled(!loading);
        connectBtn.setText(loading ? "Connecting…" : "Connect");
    }

    /** True when something HTTP-ish answers at the relay root. */
    private boolean probeRelay(ServerAddress addr) {
        HttpURLConnection c = null;
        try {
            URL url = new URL("http://" + addr.host + ":" + addr.port + "/");
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
