//! Windows Firewall status for the relay's inbound listener.
//!
//! The relay binds a TCP listener on every interface so the phone can
//! reach it over the LAN. Windows Firewall only lets that traffic
//! through when an inbound *Allow* rule exists for the binary. The
//! panel used to say "Server running" either way, so a declined
//! firewall prompt looked like a phone/network problem.
//!
//! The probe asks Windows directly (PowerShell + NetSecurity
//! cmdlets) instead of guessing. It runs off the UI thread because
//! starting PowerShell takes a second or two.

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

/// Probe the firewall for an inbound allow rule for `exe`.
///
/// Windows Firewall keeps its rules in the NetSecurity module, so we
/// ask PowerShell. The output is a single token, which keeps parsing
/// trivial and independent of the OS language.
pub fn probe(exe: &Path) -> Verdict {
    probe_with_timeout(exe, std::time::Duration::from_secs(8))
}

fn probe_with_timeout(exe: &Path, timeout: std::time::Duration) -> Verdict {
    let (tx, rx) = std::sync::mpsc::channel();
    let exe = exe.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(probe_inner(&exe));
    });
    match rx.recv_timeout(timeout) {
        Ok(v) => v,
        Err(_) => Verdict::Unknown,
    }
}

#[cfg(windows)]
fn probe_inner(exe: &Path) -> Verdict {
    use std::os::windows::process::CommandExt;

    // Escape the path for a single-quoted PowerShell string.
    let path = exe.display().to_string().replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference='SilentlyContinue';\
         $prof=@(Get-NetFirewallProfile -PolicyStore ActiveStore | Where-Object {{$_.Enabled -eq 'True'}});\
         if($prof.Count -eq 0){{'OFF'}}else{{\
         $f=@(Get-NetFirewallApplicationFilter -PolicyStore ActiveStore -Program '{path}' \
         | Get-NetFirewallRule \
         | Where-Object {{$_.Enabled -eq 'True' -and $_.Action -eq 'Allow' -and $_.Direction -eq 'Inbound'}});\
         if($f.Count -gt 0){{'ALLOW'}}else{{'BLOCKED'}}}}",
        path = path
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
                "OFF" | "ALLOW" | "BLOCKED" | "ON" | "ALLOWED" | "BLOCK"
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
fn probe_inner(_exe: &Path) -> Verdict {
    Verdict::Unknown
}

/// Add an inbound allow rule for `exe` (needs administrator rights).
///
/// The elevation prompt is the point: the user sees exactly what is
/// being changed and can decline. Returns true when the rule was
/// accepted.
pub fn add_allow_rule(exe: &Path) -> bool {
    add_allow_rule_with_timeout(exe, std::time::Duration::from_secs(60))
}

fn add_allow_rule_with_timeout(exe: &Path, timeout: std::time::Duration) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let exe = exe.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(add_allow_rule_inner(&exe));
    });
    matches!(rx.recv_timeout(timeout), Ok(true))
}

#[cfg(windows)]
fn add_allow_rule_inner(exe: &Path) -> bool {
    use std::os::windows::process::CommandExt;

    let path = exe.display().to_string().replace('\'', "''");
    // One argument string for -ArgumentList so the quoted program path
    // survives the trip through PowerShell.
    let netsh_args = format!(
        "advfirewall firewall add rule name=\"browser-relay\" dir=in action=allow enable=yes profile=any program=\"{path}\"",
        path = path
    );
    let script = format!(
        "$p = Start-Process -FilePath netsh -Verb RunAs -Wait -PassThru -ArgumentList '{args}'; \
         if ($p.ExitCode -eq 0) {{ 'OK' }} else {{ 'FAIL' }}",
        args = netsh_args.replace('\'', "''")
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
fn add_allow_rule_inner(_exe: &Path) -> bool {
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
