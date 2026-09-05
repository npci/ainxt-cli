//! Sovereign-action classification for INV-5.
//!
//! Maps a permission [`AccessKind`] to a candidate [`SovereignAction`]. The
//! [`crate::permission::manager`] consults this so that a classified Sovereign
//! action can never be auto-approved (YOLO / session grant / auto-mode); it is
//! forced to an interactive human prompt.
//!
//! ## Scope and honest limitations
//!
//! This classifier is a **human-in-the-loop trigger, not a containment
//! boundary.** Recognising persistence/credential/priv-esc intent in a shell
//! string is inherently heuristic (an attacker can obfuscate `sudo` or
//! `crontab`), so this is deliberately *fail-safe in the permissive direction*:
//! a miss means "no extra prompt", and the capability layers (egress broker,
//! exec allowlist, filesystem sandbox) remain the actual boundary. It exists to
//! make the *obvious* dangerous actions un-auto-approvable, not to catch every
//! encoding. Do not treat a `None` result as "this is safe".

use ainxt_policy::types::capability::SovereignAction;

use super::types::AccessKind;

/// Classify an access into a candidate Sovereign action, if any.
pub fn classify(access: &AccessKind) -> Option<SovereignAction> {
    match access {
        AccessKind::Bash(cmd) => classify_command(cmd),
        AccessKind::Edit(path) => classify_write_path(path),
        AccessKind::Read(Some(path)) => classify_credential_path(path),
        _ => None,
    }
}

/// Substrings that mark a security-config-changing invocation — these are the
/// governance-bypass knobs (`AINXT_SEC-001` §5.5.1, TM-12).
const SECURITY_CONFIG_MARKERS: &[&str] =
    &["ainxt_tls_insecure", "ainxt_api_backend", "ainxt_policy_authority"];

/// Privilege-escalation launchers.
const PRIVESC_MARKERS: &[&str] = &["sudo ", "doas ", "pkexec ", "su -", "su root"];

/// Persistence mechanisms.
const PERSISTENCE_MARKERS: &[&str] = &[
    "crontab",
    "systemctl enable",
    "systemctl --user enable",
    "launchctl load",
    "launchctl bootstrap",
    ".bashrc",
    ".zshrc",
    ".bash_profile",
    ".profile",
    "/etc/cron",
    ".git/hooks",
    "/launchagents/",
    "/launchdaemons/",
    // Both system (`/etc/systemd/`) and per-user (`~/.config/systemd/user/`) units.
    "/systemd/",
];

/// Credential locations (also used for the read-path check).
const CREDENTIAL_MARKERS: &[&str] = &[
    "/.ssh/",
    "id_rsa",
    "id_ed25519",
    "/.aws/credentials",
    "/.kube/config",
    "/etc/shadow",
    "169.254.169.254", // cloud instance metadata
    "/.netrc",
];

fn classify_command(cmd: &str) -> Option<SovereignAction> {
    let lc = cmd.to_ascii_lowercase();
    // Order: the most security-critical wins when several markers appear.
    if SECURITY_CONFIG_MARKERS.iter().any(|m| lc.contains(m)) {
        return Some(SovereignAction::SecurityConfigChange);
    }
    if PRIVESC_MARKERS.iter().any(|m| lc.contains(m)) || lc.trim_start().starts_with("su ") {
        return Some(SovereignAction::PrivilegeEscalation);
    }
    if CREDENTIAL_MARKERS.iter().any(|m| lc.contains(m)) || lc.contains(".env") {
        return Some(SovereignAction::CredentialAccess);
    }
    if PERSISTENCE_MARKERS.iter().any(|m| lc.contains(m)) {
        return Some(SovereignAction::Persistence);
    }
    None
}

fn classify_write_path(path: &str) -> Option<SovereignAction> {
    let lc = path.to_ascii_lowercase();
    if CREDENTIAL_MARKERS.iter().any(|m| lc.contains(m)) {
        return Some(SovereignAction::CredentialAccess);
    }
    if PERSISTENCE_MARKERS.iter().any(|m| lc.contains(m)) {
        return Some(SovereignAction::Persistence);
    }
    None
}

fn classify_credential_path(path: &str) -> Option<SovereignAction> {
    let lc = path.to_ascii_lowercase();
    if CREDENTIAL_MARKERS.iter().any(|m| lc.contains(m)) || lc.ends_with(".env") {
        return Some(SovereignAction::CredentialAccess);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privilege_escalation() {
        assert_eq!(classify(&AccessKind::Bash("sudo rm -rf /x".into())), Some(SovereignAction::PrivilegeEscalation));
        assert_eq!(classify(&AccessKind::Bash("su - root".into())), Some(SovereignAction::PrivilegeEscalation));
    }

    #[test]
    fn persistence() {
        assert_eq!(classify(&AccessKind::Bash("crontab -e".into())), Some(SovereignAction::Persistence));
        assert_eq!(classify(&AccessKind::Bash("echo x >> ~/.bashrc".into())), Some(SovereignAction::Persistence));
        assert_eq!(classify(&AccessKind::Edit("/home/u/.config/systemd/user/x.service".into())), Some(SovereignAction::Persistence));
    }

    #[test]
    fn credential_access() {
        assert_eq!(classify(&AccessKind::Bash("cat ~/.ssh/id_rsa".into())), Some(SovereignAction::CredentialAccess));
        assert_eq!(classify(&AccessKind::Read(Some("/home/u/.aws/credentials".into()))), Some(SovereignAction::CredentialAccess));
        assert_eq!(classify(&AccessKind::Bash("curl http://169.254.169.254/".into())), Some(SovereignAction::CredentialAccess));
    }

    #[test]
    fn security_config_change() {
        assert_eq!(classify(&AccessKind::Bash("export AINXT_TLS_INSECURE=1".into())), Some(SovereignAction::SecurityConfigChange));
    }

    #[test]
    fn benign_is_none() {
        assert_eq!(classify(&AccessKind::Bash("ls -la && cargo test".into())), None);
        assert_eq!(classify(&AccessKind::Read(Some("src/main.rs".into()))), None);
        assert_eq!(classify(&AccessKind::WebSearch("rust landlock".into())), None);
    }
}
