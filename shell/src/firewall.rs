//! Windows Firewall status for the relay's inbound listener.
//!
//! The relay binds a TCP listener on every interface so the phone can
//! reach it over the LAN. Windows Firewall only lets that traffic
//! through when an inbound *Allow* rule exists for the binary. The
//! panel used to say "Server running" either way, so a declined
//! firewall prompt looked like a phone/network problem.
//!
//! The probe asks Windows directly instead of guessing. It runs off
//! the UI thread because starting PowerShell takes a few seconds.
//!
//! Two traps:
//!
//! * `Get-NetFirewallRule` needs an elevated session. A tray app is
//!   not elevated, so the cmdlet throws "Access is denied" - and with
//!   the error swallowed that looks like "there are no rules", which
//!   would report *blocked* on every machine. `netsh` reads the same
//!   store without elevation, so the rule lookup uses netsh.
//! * Firewall rules are per program *path*. Every new build lands on
//!   a new path, so a rule the user accepted stops matching. The
//!   "allow" action therefore also adds a rule for the port, which
//!   survives updates and moves.

use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    /// An inbound allow rule exists for this binary - the phone can
    /// connect.
    Allowed,
    /// The firewall is on and no inbound allow rule exists for this
    /// binary: inbound connections are blocked.
    Blocked,
    /// Windows Firewall is switched off - nothing can block us.
    Disabled,
    /// The probe could not run (no PowerShell, timeout, error).
    Unknown,
}

impl Verdict {
    /// Only a real "blocked" verdict deserves a warning banner.
    pub fn is_blocked(self) -> bool {
        matches!(self, Verdict::Blocked)
    }
}

/// Probe the firewall for an inbound allow rule that covers `exe` or
/// its `port`.
///
/// The output is a single token, which keeps parsing trivial and
/// independent of the OS language.
pub fn probe(exe: &Path, port: u16) -> Verdict {
    // Reading every rule through netsh takes a few seconds.
    probe_with_timeout(exe, port, std::time::Duration::from_secs(30))
}

fn probe_with_timeout(exe: &Path, port: u16, timeout: std::time::Duration) -> Verdict {
    let (tx, rx) = std::sync::mpsc::channel();
    let exe = exe.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(probe_inner(&exe, port));
    });
    match rx.recv_timeout(timeout) {
        Ok(v) => v,
        Err(_) => Verdict::Unknown,
    }
}

#[cfg(windows)]
fn probe_inner(exe: &Path, port: u16) -> Verdict {
    use std::os::windows::process::CommandExt;

    // Escape the path for a single-quoted PowerShell string.
    let path = exe.display().to_string().replace('\'', "''");
    // Get-NetFirewallProfile reads fine unelevated; the rule store
    // does not, so netsh does that part.
    let script = format!(
        "$ErrorActionPreference='Stop';\
         $prof=@(Get-NetFirewallProfile -PolicyStore ActiveStore | Where-Object {{$_.Enabled -eq 'True'}});\
         if($prof.Count -eq 0){{'OFF';exit}};\
         $raw=(netsh advfirewall firewall show rule name=all verbose 2>$null) -join \"`n\";\
         if(-not $raw){{'UNKNOWN';exit}};\
         $ok=$false;\
         foreach($b in ($raw -split '-{{20,}}')){{\
           if($b -match '(?im)^\\s*Direction:\\s*In\\s*$' -and $b -match '(?im)^\\s*Action:\\s*Allow\\s*$' \
              -and $b -match '(?im)^\\s*Enabled:\\s*Yes\\s*$'){{\
             $prog='';$prt='';\
             if($b -match '(?im)^\\s*Program:\\s*(.+?)\\s*$'){{$prog=$matches[1]}};\
             if($b -match '(?im)^\\s*LocalPort:\\s*(.+?)\\s*$'){{$prt=$matches[1]}};\
             if($prog -and $prog -ine 'Any' -and $prog -ieq '{path}'){{$ok=$true;break}};\
             if($prt -eq '{port}'){{$ok=$true;break}};\
           }}}};\
         if($ok){{'ALLOW'}}else{{'BLOCKED'}}",
        path = path,
        port = port
    );

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let Ok(out) = out else {
        return Verdict::Unknown;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let token = text
        .split_whitespace()
        .find(|t| {
            matches!(
                *t,
                "OFF" | "ALLOW" | "ALLOWED" | "BLOCKED" | "BLOCK" | "UNKNOWN"
            )
        })
        .unwrap_or("");
    match token {
        "OFF" => Verdict::Disabled,
        "ALLOW" | "ALLOWED" => Verdict::Allowed,
        "BLOCKED" | "BLOCK" => Verdict::Blocked,
        _ => Verdict::Unknown,
    }
}

#[cfg(not(windows))]
fn probe_inner(_exe: &Path, _port: u16) -> Verdict {
    Verdict::Unknown
}

/// Add inbound allow rules for `exe` and its `port` (needs
/// administrator rights).
///
/// The elevation prompt is the point: the user sees exactly what is
/// being changed and can decline. Two rules are added in one elevated
/// shell, so there is a single prompt: one for the exact binary, one
/// for the port (which keeps working after an update or a move).
/// Returns true when at least one rule was accepted.
pub fn add_allow_rule(exe: &Path, port: u16) -> bool {
    add_allow_rule_with_timeout(exe, port, std::time::Duration::from_secs(120))
}

fn add_allow_rule_with_timeout(exe: &Path, port: u16, timeout: std::time::Duration) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let exe = exe.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(add_allow_rule_inner(&exe, port));
    });
    matches!(rx.recv_timeout(timeout), Ok(true))
}

#[cfg(windows)]
fn add_allow_rule_inner(exe: &Path, port: u16) -> bool {
    use std::os::windows::process::CommandExt;

    let path = exe.display().to_string().replace('\'', "''");
    // One elevated shell, two netsh commands.
    let cmd = format!(
        "netsh advfirewall firewall add rule name=\"browser-relay\" dir=in action=allow \
         enable=yes profile=any program=\"{path}\" & \
         netsh advfirewall firewall add rule name=\"browser-relay port {port}\" dir=in \
         action=allow enable=yes profile=any protocol=TCP localport={port}",
        path = path,
        port = port
    );
    let script = format!(
        "$p = Start-Process -FilePath cmd -Verb RunAs -Wait -PassThru -ArgumentList '/c', '{cmd}'; \
         if ($p.ExitCode -eq 0) {{ 'OK' }} else {{ 'FAIL' }}",
        cmd = cmd.replace('\'', "''")
    );

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .split_whitespace()
            .any(|t| t == "OK"),
        Err(_) => false,
    }
}

#[cfg(not(windows))]
fn add_allow_rule_inner(_exe: &Path, _port: u16) -> bool {
    false
}

/// Open the Windows Firewall control panel (no elevation needed).
pub fn open_settings() {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("rundll32")
            .args([
                "shell32.dll,Control_RunDLL",
                "firewall.cpl",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(not(windows))]
    {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_is_the_only_warning_verdict() {
        assert!(Verdict::Blocked.is_blocked());
        assert!(!Verdict::Allowed.is_blocked());
        assert!(!Verdict::Disabled.is_blocked());
        assert!(!Verdict::Unknown.is_blocked());
    }
}
