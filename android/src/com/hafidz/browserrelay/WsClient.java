package com.hafidz.browserrelay;

import android.os.Handler;
import android.os.Looper;
import android.util.Base64;
import android.util.Log;

import java.io.ByteArrayOutputStream;
import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.ConnectException;
import java.net.InetSocketAddress;
import java.net.NoRouteToHostException;
import java.net.Socket;
import java.net.SocketTimeoutException;
import java.net.URI;
import java.net.UnknownHostException;
import java.security.MessageDigest;
import java.security.SecureRandom;

/**
 * Minimal RFC 6455 WebSocket client: text frames only, ws:// only.
 * The relay speaks plain JSON text frames, so this is all we need —
 * zero external dependencies. Reader runs on its own thread;
 * callbacks fire on the main thread.
 */
public class WsClient {
    private static final String TAG = "WsClient";
    private static final String GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    private static final long MAX_FRAME = 16 * 1024 * 1024;

    public interface Listener {
        void onOpen();
        void onMessage(String text);
        void onClose();
        /**
         * @param kind one of the {@link Failure} constants, so the UI
         *              can react (offer the code field, offer a retry).
         */
        void onError(String msg, int kind);
    }

    /** Why a connection failed. The UI maps these to an explanation. */
    public static final class Failure {
        public static final int UNKNOWN = 0;
        /** The host cannot be reached (no route / wrong Wi-Fi). */
        public static final int UNREACHABLE = 1;
        /** Nothing is listening on that port. */
        public static final int REFUSED = 2;
        /** The host did not answer in time. */
        public static final int TIMEOUT = 3;
        /** The peer closed the connection before the handshake. */
        public static final int CLOSED = 4;
        /** Something answered, but it is not the relay's websocket. */
        public static final int NOT_RELAY = 5;
        /** The relay refused the pairing code. */
        public static final int BAD_CODE = 6;
        /** This device is not paired yet. */
        public static final int NOT_PAIRED = 7;
        /** The connection was lost after it had been established. */
        public static final int LOST = 8;
    }

    /** A failure that already carries its own user-facing sentence. */
    private static class ConnectFail extends IOException {
        final int kind;

        ConnectFail(int kind, String msg) {
            super(msg);
            this.kind = kind;
        }
    }

    private final Handler main = new Handler(Looper.getMainLooper());
    /** Cryptographic randomness for the handshake and the frame masks. */
    private final SecureRandom random = new SecureRandom();
    private volatile Socket socket;
    private volatile OutputStream out;
    private volatile boolean closed;
    private volatile boolean closeNotified;
    /** Read timeout in ms; 0 means "wait forever". */
    private volatile int readTimeoutMs = 0;

    /**
     * Set the read timeout of the socket. A peer that stops answering
     * (dropped Wi-Fi) then closes the socket instead of freezing the
     * picture forever. 0 disables it.
     */
    public void setReadTimeoutMs(int ms) {
        readTimeoutMs = ms;
        Socket s = socket;
        if (s == null || ms <= 0) return;
        try {
            s.setSoTimeout(ms);
        } catch (Exception e) {
            Log.w(TAG, "setSoTimeout: " + e);
        }
    }

    public void connect(String wsUrl, Listener listener) {
        closed = false;
        closeNotified = false;
        Thread t = new Thread(() -> runConnect(wsUrl, listener), "ws-reader");
        t.setDaemon(true);
        t.start();
    }

    private void post(Runnable r) {
        main.post(r);
    }

    private void runConnect(String wsUrl, Listener listener) {
        String host = "";
        int port = 0;
        try {
            URI uri = new URI(wsUrl);
            if (!"ws".equalsIgnoreCase(uri.getScheme())) {
                throw new ConnectFail(Failure.UNKNOWN,
                        "only ws:// is supported in v1");
            }
            // getHost() is null for an authority the URI parser does not
            // recognise; say so instead of connecting to "null".
            host = uri.getHost() == null ? "" : uri.getHost();
            port = uri.getPort() > 0 ? uri.getPort() : 80;
            String path = uri.getRawPath();
            if (path == null || path.isEmpty()) path = "/";
            if (uri.getRawQuery() != null) path += "?" + uri.getRawQuery();
            if (host.isEmpty()) {
                throw new ConnectFail(Failure.UNKNOWN,
                        "\"" + wsUrl + "\" is not a server address - "
                                + "use host:port, for example 192.168.1.10:8765");
            }

            Socket s = new Socket();
            try {
                s.connect(new InetSocketAddress(host, port), 8000);
            } catch (UnknownHostException e) {
                throw new ConnectFail(Failure.UNREACHABLE,
                        "cannot find \"" + host + "\" - check the server address");
            } catch (NoRouteToHostException e) {
                throw new ConnectFail(Failure.UNREACHABLE,
                        "no way to reach " + host + " - check the Wi-Fi and the IP");
            } catch (ConnectException e) {
                throw new ConnectFail(Failure.REFUSED,
                        "nothing is listening on " + host + ":" + port
                                + " - is the relay running on the PC?");
            } catch (SocketTimeoutException e) {
                throw new ConnectFail(Failure.TIMEOUT,
                        host + ":" + port + " did not answer - check the Wi-Fi and the IP");
            }
            if (readTimeoutMs > 0) {
                try {
                    s.setSoTimeout(readTimeoutMs);
                } catch (Exception e) {
                    Log.w(TAG, "setSoTimeout: " + e);
                }
            }
            socket = s;
            InputStream in = s.getInputStream();
            out = s.getOutputStream();

            // The handshake must not hang forever either.
            try {
                s.setSoTimeout(10000);
            } catch (Exception e) {
                Log.w(TAG, "setSoTimeout: " + e);
            }
            byte[] keyBytes = new byte[16];
            random.nextBytes(keyBytes);
            String key = Base64.encodeToString(keyBytes, Base64.NO_WRAP);

            String req = "GET " + path + " HTTP/1.1\r\n"
                    + "Host: " + host + ":" + port + "\r\n"
                    + "Upgrade: websocket\r\n"
                    + "Connection: Upgrade\r\n"
                    + "Sec-WebSocket-Key: " + key + "\r\n"
                    + "Sec-WebSocket-Version: 13\r\n\r\n";
            out.write(req.getBytes("UTF-8"));
            out.flush();

            String headers;
            try {
                headers = readHttpHeaders(in);
            } catch (EOFException e) {
                // The peer accepted the TCP connection and then hung up
                // without a word: a wrong port, a wrong host, or a
                // firewall that drops the handshake. This used to reach
                // the user as a bare "EOFException".
                throw new ConnectFail(Failure.CLOSED,
                        host + ":" + port + " closed the connection before the "
                                + "handshake - check the address and the port");
            }
            if (!headers.startsWith("HTTP/1.1 101") && !headers.startsWith("HTTP/1.0 101")) {
                throw new ConnectFail(Failure.NOT_RELAY,
                        host + ":" + port + " answered \"" + firstLine(headers)
                                + "\" - that is not the relay (it wants /ws)");
            }
            String accept = headerValue(headers, "Sec-WebSocket-Accept");
            MessageDigest sha1 = MessageDigest.getInstance("SHA-1");
            byte[] digest = sha1.digest((key + GUID).getBytes("UTF-8"));
            String expected = Base64.encodeToString(digest, Base64.NO_WRAP);
            if (accept == null || !expected.equals(accept.trim())) {
                throw new ConnectFail(Failure.NOT_RELAY,
                        host + ":" + port + " is not a browser-relay server");
            }

            // From here on the read timeout is the one the client asked
            // for (0 = wait forever until the peer says something).
            try {
                s.setSoTimeout(readTimeoutMs > 0 ? readTimeoutMs : 0);
            } catch (Exception e) {
                Log.w(TAG, "setSoTimeout: " + e);
            }
            post(listener::onOpen);
            readLoop(in, listener);
        } catch (ConnectFail e) {
            if (!closed) {
                post(() -> listener.onError(e.getMessage(), e.kind));
            }
        } catch (EOFException e) {
            // The connection dropped while it was running.
            if (!closed) {
                post(() -> listener.onError(
                        "the connection to the PC was closed", Failure.LOST));
            }
        } catch (Exception e) {
            if (!closed) {
                final String msg;
                if (e instanceof SocketTimeoutException) {
                    // The peer stopped answering (dropped Wi-Fi, PC gone).
                    msg = "the PC stopped answering";
                } else {
                    msg = "cannot connect to " + host + ":" + port
                            + " (" + e.getMessage() + ")";
                }
                post(() -> listener.onError(msg, Failure.UNKNOWN));
            }
        } finally {
            notifyClose(listener);
            closeSocket();
        }
    }

    private void notifyClose(Listener listener) {
        if (!closeNotified) {
            closeNotified = true;
            post(listener::onClose);
        }
    }

    private void closeSocket() {
        try {
            if (socket != null) socket.close();
        } catch (Exception ignored) {
        }
        socket = null;
        out = null;
    }

    private static String firstLine(String headers) {
        int i = headers.indexOf("\r\n");
        return i >= 0 ? headers.substring(0, i) : headers;
    }

    private static String headerValue(String headers, String name) {
        String needle = "\r\n" + name.toLowerCase() + ":";
        int i = headers.toLowerCase().indexOf(needle);
        if (i < 0) return null;
        int start = i + needle.length();
        int end = headers.indexOf("\r\n", start);
        return end >= 0 ? headers.substring(start, end) : headers.substring(start);
    }

    private static String readHttpHeaders(InputStream in) throws IOException {
        ByteArrayOutputStream buf = new ByteArrayOutputStream();
        int[] tail = new int[4];
        int n = 0;
        while (n < 8192) {
            int b = in.read();
            if (b < 0) throw new EOFException("eof during handshake");
            buf.write(b);
            tail[0] = tail[1];
            tail[1] = tail[2];
            tail[2] = tail[3];
            tail[3] = b;
            n++;
            if (tail[0] == '\r' && tail[1] == '\n' && tail[2] == '\r' && tail[3] == '\n') {
                break;
            }
        }
        return new String(buf.toByteArray(), "UTF-8");
    }

    private void readLoop(InputStream in, Listener listener) throws IOException {
        ByteArrayOutputStream frag = null;
        while (!closed) {
            int b0 = in.read();
            int b1 = in.read();
            if (b0 < 0 || b1 < 0) throw new EOFException("eof");
            boolean fin = (b0 & 0x80) != 0;
            int opcode = b0 & 0x0F;
            boolean masked = (b1 & 0x80) != 0;
            long len = b1 & 0x7F;
            if (len == 126) {
                len = readU16(in);
            } else if (len == 127) {
                len = readU64(in);
            }
            if (len > MAX_FRAME) throw new IOException("frame too large: " + len);
            byte[] mask = null;
            if (masked) {
                mask = new byte[4];
                readFully(in, mask, 0, 4);
            }
            int ilen = (int) len;
            byte[] payload = new byte[ilen];
            readFully(in, payload, 0, ilen);
            if (masked) {
                for (int i = 0; i < ilen; i++) payload[i] ^= mask[i % 4];
            }

            switch (opcode) {
                case 0x8: // close
                    try {
                        sendFrame(0x8, new byte[0]);
                    } catch (Exception ignored) {
                    }
                    throw new EOFException("server close");
                case 0x9: // ping -> pong
                    try {
                        sendFrame(0xA, payload);
                    } catch (Exception ignored) {
                    }
                    break;
                case 0xA: // pong: ignore
                    break;
                case 0x1: // text: start of a (possibly fragmented) message
                    frag = new ByteArrayOutputStream();
                    frag.write(payload, 0, ilen);
                    if (fin) {
                        final String text = new String(frag.toByteArray(), "UTF-8");
                        frag = null;
                        post(() -> listener.onMessage(text));
                    }
                    break;
                case 0x0: // continuation
                    if (frag == null) break; // stray: ignore
                    frag.write(payload, 0, ilen);
                    if (fin) {
                        final String text = new String(frag.toByteArray(), "UTF-8");
                        frag = null;
                        post(() -> listener.onMessage(text));
                    }
                    break;
                case 0x2: // binary: not used by the relay
                    break;
                default:
                    throw new IOException("bad opcode " + opcode);
            }
        }
    }

    private static int readU16(InputStream in) throws IOException {
        byte[] b = new byte[2];
        readFully(in, b, 0, 2);
        return ((b[0] & 0xFF) << 8) | (b[1] & 0xFF);
    }

    private static long readU64(InputStream in) throws IOException {
        byte[] b = new byte[8];
        readFully(in, b, 0, 8);
        long v = 0;
        for (int i = 0; i < 8; i++) v = (v << 8) | (b[i] & 0xFF);
        return v;
    }

    private static void readFully(InputStream in, byte[] b, int off, int len) throws IOException {
        while (len > 0) {
            int n = in.read(b, off, len);
            if (n < 0) throw new EOFException("eof");
            off += n;
            len -= n;
        }
    }

    /** Send one masked text frame. */
    public void send(String text) {
        OutputStream o = out;
        if (closed || o == null) return;
        try {
            sendFrame(0x1, text.getBytes("UTF-8"));
        } catch (IOException e) {
            Log.w(TAG, "send failed: " + e);
        }
    }

    private synchronized void sendFrame(int opcode, byte[] payload) throws IOException {
        OutputStream o = out;
        if (closed || o == null) return;
        int len = payload.length;
        ByteArrayOutputStream h = new ByteArrayOutputStream();
        h.write(0x80 | opcode);
        if (len < 126) {
            h.write(0x80 | len);
        } else if (len < 65536) {
            h.write(0x80 | 126);
            h.write((len >> 8) & 0xFF);
            h.write(len & 0xFF);
        } else {
            h.write(0x80 | 127);
            h.write(0);
            h.write(0);
            h.write(0);
            h.write(0);
            h.write((len >> 24) & 0xFF);
            h.write((len >> 16) & 0xFF);
            h.write((len >> 8) & 0xFF);
            h.write(len & 0xFF);
        }
        byte[] mask = new byte[4];
        random.nextBytes(mask);
        h.write(mask, 0, 4);
        byte[] masked = new byte[len];
        for (int i = 0; i < len; i++) masked[i] = (byte) (payload[i] ^ mask[i % 4]);
        o.write(h.toByteArray());
        o.write(masked);
        o.flush();
    }

    public void close() {
        if (closed) return;
        // Send the close frame BEFORE tearing the socket down: the peer
        // should see a clean close, not a TCP reset. The flag is set
        // afterwards, because sendFrame() refuses to write once closed.
        try {
            sendFrame(0x8, new byte[0]);
        } catch (Exception ignored) {
        }
        closed = true;
        closeSocket();
    }
}
