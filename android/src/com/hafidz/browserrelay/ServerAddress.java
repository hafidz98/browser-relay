package com.hafidz.browserrelay;

/**
 * The server address the phone talks to, parsed once and used
 * everywhere.
 *
 * <p>The address reaches the app from three places: the pairing card
 * (the user types it), the deep link from the relay's /pair page, and
 * the saved preference. All three can carry junk: a scheme
 * ("http://192.168.1.10:8765"), a trailing slash, a pasted path
 * ("192.168.1.10:8765/app"), a missing port, or a stray space.
 *
 * <p>The app used to paste that string straight into
 * "ws://" + host + "/ws" and let java.net.URI sort it out. The
 * result was silent: a missing port became port 80, a pasted scheme
 * made {@code URI.getHost()} return null (so the app connected to
 * nowhere), and a pasted path made the websocket live on /app/ws,
 * where the relay answers with its web page. The user only ever saw
 * "EOFException".
 *
 * <p>This class turns the raw text into a validated host and port
 * (and always the one websocket path the relay serves), and says what
 * is wrong when it cannot.
 */
public final class ServerAddress {

    /** The path the relay serves its websocket on. */
    public static final String WS_PATH = "/ws";
    /** The relay's default port. */
    public static final int DEFAULT_PORT = 8765;

    public final String host;
    public final int port;

    private ServerAddress(String host, int port) {
        this.host = host;
        this.port = port;
    }

    /** The empty address - nothing typed yet. */
    public static ServerAddress empty() {
        return new ServerAddress("", 0);
    }

    /**
     * Parse the text the user typed (or the deep link carried).
     *
     * @return the address, or an address whose {@link #error} explains
     *         why the text cannot be used.
     */
    public static ServerAddress parse(String raw) {
        String text = raw == null ? "" : raw.trim();
        if (text.isEmpty()) {
            ServerAddress a = new ServerAddress("", 0);
            a.error = "type the PC address, for example 192.168.1.10:8765";
            return a;
        }
        // A scheme the user pasted in. Everything the relay serves is
        // plain ws:// on the LAN, so the scheme carries no information.
        text = text.replaceFirst("^[a-zA-Z][a-zA-Z0-9+.-]*://", "");
        // A path, a query or a fragment - the relay's websocket has a
        // fixed path, so anything after the host is a mistake.
        int cut = indexOfAny(text, "/?#");
        if (cut >= 0) {
            text = text.substring(0, cut);
        }
        if (text.isEmpty()) {
            ServerAddress a = new ServerAddress("", 0);
            a.error = "that is not a server address - use host:port, "
                    + "for example 192.168.1.10:8765";
            return a;
        }

        String host;
        int port = DEFAULT_PORT;
        if (text.startsWith("[")) {
            // IPv6 literal: [::1]:8765
            int end = text.indexOf(']');
            if (end < 0) {
                ServerAddress a = new ServerAddress("", 0);
                a.error = "that address is missing a closing ]";
                return a;
            }
            host = text.substring(0, end + 1);
            String rest = text.substring(end + 1);
            if (rest.startsWith(":")) {
                try {
                    port = Integer.parseInt(rest.substring(1));
                } catch (NumberFormatException e) {
                    ServerAddress a = new ServerAddress("", 0);
                    a.error = "\"" + rest.substring(1) + "\" is not a port number";
                    return a;
                }
            } else if (!rest.isEmpty()) {
                ServerAddress a = new ServerAddress("", 0);
                a.error = "that address has something after the port";
                return a;
            }
        } else {
            int colon = text.lastIndexOf(':');
            if (colon >= 0) {
                host = text.substring(0, colon);
                String portText = text.substring(colon + 1);
                try {
                    port = Integer.parseInt(portText);
                } catch (NumberFormatException e) {
                    ServerAddress a = new ServerAddress("", 0);
                    a.error = "\"" + portText + "\" is not a port number";
                    return a;
                }
            } else {
                host = text;
            }
        }

        if (host.isEmpty()) {
            ServerAddress a = new ServerAddress("", 0);
            a.error = "the address needs a host, for example 192.168.1.10:8765";
            return a;
        }
        if (port < 1 || port > 65535) {
            ServerAddress a = new ServerAddress("", 0);
            a.error = "port " + port + " is out of range";
            return a;
        }
        if (host.indexOf(' ') >= 0) {
            ServerAddress a = new ServerAddress("", 0);
            a.error = "the address cannot contain spaces";
            return a;
        }
        ServerAddress a = new ServerAddress(host, port);
        a.error = null;
        return a;
    }

    private static int indexOfAny(String s, String chars) {
        for (int i = 0; i < s.length(); i++) {
            if (chars.indexOf(s.charAt(i)) >= 0) return i;
        }
        return -1;
    }

    /** What is wrong with the text, or null when it is fine. */
    public String error;

    public boolean isValid() {
        return error == null && !host.isEmpty();
    }

    public boolean isEmpty() {
        return host.isEmpty() && port == 0;
    }

    /** "host:port" - what the app saves and shows. */
    public String display() {
        if (host.isEmpty()) return "";
        return port == DEFAULT_PORT && !host.startsWith("[")
                ? host + ":" + port
                : host + ":" + port;
    }

    /** The websocket URL. The path is always the relay's /ws. */
    public String wsUrl() {
        return "ws://" + host + ":" + port + WS_PATH;
    }
}
