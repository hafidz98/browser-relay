# 2026-10-10 — Project kickoff, plan v0.1

## What happened
- Hafidz asked whether I have a user-controllable browser feature. Confirmed:
  live browser watch/takeover exists but is client-dependent.
- He then asked for Android + Windows client apps so he can drive his
  laptop's browser from his phone: logins stay on the laptop, phone only
  gets the stream + sends input, local network for low latency.
- Brief detour: he got info about "Chrome remote" — clarified it meant
  Chrome Remote Desktop; compared (whole-desktop, via Google servers,
  needs Google login) vs the custom relay (browser-only, local/Tailscale).
  He chose to continue with the custom relay.
- Wrote `docs/plan/PLAN.md` v0.1 (this project), per the standing
  docs/plan-first rule.

## Key design decisions (plan)
- Relay server in Go, cross-compiled to a single `relay.exe` — zero
  runtime install on his Windows laptop.
- Attach to his real Chrome profile via `--remote-debugging-port` added to
  the Chrome shortcut (can't retrofit a running instance — documented).
- CDP `Page.startScreencast` (JPEG) + `Input.dispatch*`; WebSocket protocol
  relay<->client with token auth; bind Tailscale IP only, one driver at a
  time.
- Android = WebView-wrapper APK (proven build pattern). Windows client =
  the same web client via browser shortcut (no native .exe in v1 — honest
  cost/benefit call, documented as non-goal).
- v1 non-goals: audio (CDP has none), download-to-phone, multi-user,
  clipboard sync (deliberate, exfiltration path).

## Open questions for Hafidz (PLAN.md section 8)
Q1 Chrome install path, Q2 Tailscale on laptop+phone, Q3 tab picker in v1?,
Q4 Windows .exe client wanted later?

## Next
M0: verify CDP screencast flow locally + collect his laptop details, then
M1 relay prototype. Waiting on Hafidz's answers before M2 (needs his laptop).
