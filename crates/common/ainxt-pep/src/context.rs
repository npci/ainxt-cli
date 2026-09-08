//! Deriving a [`Principal`] and shell kind from the local environment.
//!
//! Shared by the permission adapter and by `ainxt policy`, so that what the
//! explain command describes is keyed to the same subject the enforcement path
//! actually charges. If these drifted, `ainxt policy status` would cheerfully
//! report a healthy budget belonging to a different identity than the one being
//! spent.

use crate::{ClientId, Principal, SessionId, Shell};

/// Risk-ledger identity.
///
/// Must be stable across processes on a host, because the budget is
/// deliberately shared between clients — a per-process subject would let an
/// attacker reset it by opening a second client.
pub fn subject() -> String {
    if let Ok(id) = std::env::var("AINXT_USER_ID")
        && !id.trim().is_empty()
    {
        return id;
    }
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_owned());
    format!("local:{user}")
}

pub fn client_label() -> String {
    std::env::var("AINXT_CLIENT_NAME").unwrap_or_else(|_| "ainxt-cli".to_owned())
}

/// The principal for work originating on this machine.
///
/// A subagent charges its parent, so spawning children is not a route to a
/// fresh budget.
pub fn local_principal(session: Option<&str>, subagent: Option<&str>) -> Principal {
    let session_id = SessionId(session.unwrap_or("unknown").to_owned());
    Principal {
        subject: subject(),
        client: ClientId(client_label()),
        session: session_id.clone(),
        parent_session: subagent.map(|_| session_id),
    }
}

/// Which shell will interpret a command on this platform.
///
/// Approximate: the authoritative answer comes from `ShellKind::detect()` at
/// the spawn site, which lives in `ainxt-tools` and would be a dependency cycle
/// from here. The error is bounded and in the safe direction — a Windows host
/// running git-bash is evaluated with the stricter fail-closed backend, never
/// the looser one.
pub fn default_shell() -> Shell {
    if cfg!(windows) {
        Shell::PowerShell
    } else {
        Shell::Posix
    }
}
