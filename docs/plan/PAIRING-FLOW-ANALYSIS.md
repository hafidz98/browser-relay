# Analysis: Request-Based Pairing Flow (Hafidz's proposal)

## Proposed flow
1. Relay PC running.
2. Web/client opens URL (unpaired) → taps "Minta Kode Pairing".
3. **New pair code appears on the PC panel** (not on the phone).
4. User reads the code from the PC screen, types it into the client.
5. Client pairs (single-use code + fingerprint contract).

## Security analysis

### Why this is strong
This is the "code on trusted display" pattern (YouTube on TV, Chromecast,
WhatsApp Web number-linking). The core security property:

**The pairing secret is never transmitted to the untrusted device.**
The phone only *requests*; the code is revealed exclusively on the PC
screen, which the owner physically controls. Consequences:

| Attacker capability | Outcome |
|---|---|
| On LAN, opens URL, taps "Minta kode" | Code appears on PC panel. Attacker can't see it. Owner sees unexpected request → cancels/ignores. **Blocked.** |
| Spams pairing requests | Rate-limited (1 pending code, per-IP cooldown). Panel shows latest request with device name/IP. Annoyance at most. **Mitigated.** |
| Shoulder-surfs the code off the PC screen | Can pair — but owner sees a new device in the client list and can revoke. Same exposure as QR flow. **Accepted (physical-access tier).** |
| Replays an old code | Single-use: burned on first presentation. **Blocked.** |
| Sniffs LAN traffic | Code typed by user goes over ws:// in cleartext — same as current model. LAN trust assumption unchanged. **No regression.** |

### What it fixes vs QR-only
- **Web clients** can't scan QR (no camera API in this flow) — request-code
  works for them with zero extra hardware.
- **No code in transit to the phone before approval.** In QR flow the code
  is embedded in the QR image; anyone who photographs the panel screen
  gets it. In request flow the code doesn't exist until someone asks, and
  the owner sees *who* asked (name/IP) at the moment it's created.

### Do we need an Approve/Deny button?
No — and adding one is *weaker* UX, not stronger. The code display IS the
approval: only someone with eyes on the PC screen can complete pairing.
An Approve button adds a click without adding security (the owner still
has to judge the request). What we DO need is **Cancel/Dismiss** on the
panel, for the spam/unsolicited-request case.

### Remaining abuse: request spam
Mitigations (all cheap):
- One pending code at a time; a new request replaces the old (panel updates).
- Per-IP cooldown (e.g., 30 s between requests from the same IP).
- 5-minute TTL on the displayed code; panel auto-clears it.
- Panel shows requester identity: device name (client-supplied, untrusted
  but useful), IP address, time. Owner pattern-matches "that's my phone".

## Recommendation: implement BOTH flows on one backend

| Flow | Initiator | Code transport | When to use |
|---|---|---|---|
| QR (existing) | Owner at PC | QR image → camera scan | Owner pairing their own phone quickly |
| Request-code (new) | Client | Eyes: PC screen → fingers | Web client, or owner not at phone |

Backend (already planned in PAIRING-FINGERPRINT.md) is identical:
single-use code + fingerprint contract + paired.json + revoke. The only
new pieces for request-code:

1. **Pre-auth `pair_request` message** (WS): `{type:"pair_request",
   fingerprint, name}` → relay rate-limits, generates code, stores as
   the pending code.
2. **Panel notification**: relay exposes the pending request
   (device name, IP, code, age); panel polls (already polls at 250 ms)
   and pops a prominent "Perangkat meminta pairing" card with the code
   big, + "Batalkan" button.
3. **Client UI**: unpaired state gets "Minta Kode Pairing" button →
   waiting state ("Cek panel di PC…") + 6-digit input → `auth`.
4. QR keeps working: it encodes a single-use code from the same pool
   (panel-initiated, no request needed).

## Decision needed
Approve implementing both flows (recommended), or request-code only?
