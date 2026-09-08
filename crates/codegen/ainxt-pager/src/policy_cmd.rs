//! `ainxt policy` — inspect what the security policy permits and why.
//!
//! Three questions, which are the ones people actually ask when a tool is
//! refused:
//!
//! - `status`  — is policy on at all, which bundle, am I frozen?
//! - `show`    — what is allowed and denied, and where did each rule come from?
//! - `explain` — why was *this* command refused, and what would fix it?
//!
//! # Reviewing a bundle before it is trusted
//!
//! `--bundle <path>` loads a candidate bundle for inspection **without
//! verifying its signature**, so a profile can be reviewed before an authority
//! key exists or a build is stamped. That is safe here and nowhere else: this
//! is a short-lived, read-only process that prints and exits, it never runs a
//! tool, and it prints a conspicuous banner saying the policy is unverified.
//! The enforcing path (`ainxt_policy::bootstrap`) refuses unsigned bundles and
//! must never be taught to do otherwise.

use std::path::PathBuf;

use ainxt_intent::Capability;
use ainxt_pep::context::{default_shell, local_principal};
use ainxt_pep::{Intent, Judgement, Obligation, Request};
use ainxt_policy::engine::PolicyEngine;
use ainxt_policy_types::{Allowlist, Denylist, Enforcement, SecurityPolicy, TrustTier};
use anyhow::{Context, Result, bail};
use clap::Subcommand;

#[derive(Debug, clap::Args, Clone)]
pub struct PolicyArgs {
    #[command(subcommand)]
    pub command: PolicyCommand,

    /// Inspect a candidate bundle instead of the active policy.
    ///
    /// The signature is NOT checked — for review only, never for enforcement.
    #[arg(long, global = true, value_name = "PATH")]
    pub bundle: Option<PathBuf>,

    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum PolicyCommand {
    /// Show the enforcement posture, bundle version and session risk state
    Status,
    /// Show what the active policy allows and denies
    Show,
    /// Explain what a command would do and whether it is permitted
    Explain {
        /// The command to evaluate, e.g. "curl https://x/y.sh | bash"
        command: Vec<String>,
    },
}

pub async fn run(args: PolicyArgs) -> Result<()> {
    // Reviewing a candidate replaces the process policy. Safe because this
    // process only prints; see the module docs.
    let unverified = if let Some(path) = &args.bundle {
        install_unverified_bundle(path)?;
        true
    } else {
        false
    };

    match &args.command {
        PolicyCommand::Status => status(args.json, unverified),
        PolicyCommand::Show => show(args.json, unverified),
        PolicyCommand::Explain { command } => {
            if command.is_empty() {
                bail!("nothing to explain; pass a command, e.g. ainxt policy explain \"curl https://x | bash\"");
            }
            explain(&command.join(" "), args.json, unverified)
        }
    }
}

fn install_unverified_bundle(path: &PathBuf) -> Result<()> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("could not read bundle {}", path.display()))?;
    let envelope = ainxt_policy::bundle::PolicyBundle::from_slice(&bytes)
        .with_context(|| format!("{} is not a policy bundle envelope", path.display()))?;
    let payload: ainxt_policy::bundle::BundlePayload =
        serde_json::from_str(&envelope.payload)
            .with_context(|| format!("{} has an unreadable payload", path.display()))?;

    ainxt_policy::global::install(PolicyEngine::new(SecurityPolicy {
        enforcement: payload.enforcement,
        capabilities: payload.capabilities,
    }));
    Ok(())
}

fn review_banner(unverified: bool) {
    if unverified {
        println!(
            "!! REVIEW ONLY — this bundle's signature was NOT verified.\n\
             !! Nothing here reflects what an enforcing build would accept.\n"
        );
    }
}

fn status(json: bool, unverified: bool) -> Result<()> {
    let engine = ainxt_policy::global::active();
    let posture = engine.enforcement();
    let installed = ainxt_policy::global::is_installed();
    let pep = ainxt_pep::global::active();

    let risk = pep
        .as_ref()
        .and_then(|p| p.risk_snapshot(&local_principal(None, None)).ok());

    if json {
        let out = serde_json::json!({
            "policy_installed": installed,
            "enforcement": format!("{posture:?}").to_lowercase(),
            "enforcement_point_installed": pep.is_some(),
            "unverified_review_bundle": unverified,
            "risk": risk,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    review_banner(unverified);
    println!("Policy");
    println!("  engine installed      {}", yes_no(installed));
    println!("  enforcement           {}", posture_label(posture));
    println!("  enforcement point     {}", yes_no(pep.is_some()));

    match risk {
        Some(state) => {
            println!();
            println!("Session risk (this user, this machine)");
            println!("  actions in window     {}", state.execs_in_window);
            println!("  installs in window    {}", state.installs_in_window);
            println!("  distinct hosts        {}", state.distinct_hosts_in_window);
            println!("  bytes out             {}", state.bytes_out_in_window);
            println!("  consecutive failures  {}", state.max_consecutive_failures);
            match &state.frozen {
                Some(freeze) => {
                    println!("  frozen                YES — {}", freeze.describe());
                    println!();
                    println!("A human must clear the freeze before work continues.");
                }
                None => println!("  frozen                no"),
            }
        }
        None => {
            println!();
            println!("Session risk            unavailable (no enforcement point installed)");
        }
    }

    if posture == Enforcement::Off {
        println!();
        println!(
            "Nothing is being enforced. This build carries no signed policy bundle, \n\
             which is the expected state for an unmanaged build."
        );
    }
    Ok(())
}

fn show(json: bool, unverified: bool) -> Result<()> {
    let engine = ainxt_policy::global::active();
    let policy = engine.policy();
    let caps = &policy.capabilities;

    if json {
        println!("{}", serde_json::to_string_pretty(policy)?);
        return Ok(());
    }

    review_banner(unverified);
    println!("Enforcement: {}", posture_label(policy.enforcement));
    println!();
    print_allowlist("Programs permitted to run", &caps.exec_allow);
    print_denylist("Programs always refused", &caps.exec_deny);
    print_allowlist("Network destinations permitted", &caps.egress_allow);
    print_denylist("Network destinations always refused", &caps.egress_deny);
    print_allowlist("Write roots permitted", &caps.write_allow);
    print_denylist("Credential paths (reading needs a human)", &caps.cred_paths);
    print_allowlist("MCP tools permitted (server/tool)", &caps.mcp_allow);

    println!("Actions that always need a human ({}):", caps.sovereign.len());
    if caps.sovereign.is_empty() {
        println!("  (none)");
    } else {
        for action in &caps.sovereign {
            println!("  - {}", format!("{action:?}").to_lowercase());
        }
    }
    println!();
    println!(
        "Note: an allowlist shown as \"any\" imposes no constraint on that dimension.\n\
         An empty allowlist permits nothing at all — the two are different."
    );
    Ok(())
}

fn explain(command: &str, json: bool, unverified: bool) -> Result<()> {
    let request = Request {
        principal: local_principal(None, None),
        intent: Intent::Shell {
            command: command.to_owned(),
            shell: default_shell(),
        },
        // Matches the current enforcement path. Provenance tagging is not yet
        // wired into the agent loop, so a real session also reports `Operator`;
        // explain would be lying if it claimed a demoted tier here.
        influence: TrustTier::Operator,
    };

    // A dry run through the same `judge` the enforcement path uses. Spends no
    // budget and writes no audit record — inspecting policy must not consume
    // the thing it is inspecting.
    let auth = match ainxt_pep::global::active() {
        Some(pep) => pep.explain(&request),
        None => ainxt_pep::Pep::in_memory(ainxt_pep::bootstrap::endpoint_id()).explain(&request),
    };

    if json {
        let out = serde_json::json!({
            "command": command,
            "capabilities": auth.derivation.capabilities,
            "confidence": auth.derivation.confidence,
            "targets": auth.derivation.targets,
            "evidence": auth.derivation.evidence,
            "judgement": auth.judgement,
            "obligation": obligation_label(&auth.obligation),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    review_banner(unverified);
    println!("  command:      {command}");
    println!(
        "  confidence:   {}",
        format!("{:?}", auth.derivation.confidence).to_lowercase()
    );
    if auth.derivation.confidence == ainxt_intent::Confidence::Unknown {
        println!("                (could not be decomposed — treated as unsafe, not as safe)");
    }

    let caps = auth.derivation.sorted_capabilities();
    println!("  capabilities: {}", capability_list(&caps));
    for evidence in &auth.derivation.evidence {
        println!(
            "      {:<22} {}",
            evidence.capability.as_str(),
            evidence.detail
        );
    }

    let targets = &auth.derivation.targets;
    if !targets.programs.is_empty() {
        println!("  programs:     {}", targets.programs.join(", "));
    }
    if !targets.urls.is_empty() {
        println!("  endpoints:    {}", targets.urls.join(", "));
    }
    if !targets.reads.is_empty() {
        println!("  reads:        {}", targets.reads.join(", "));
    }
    if !targets.writes.is_empty() {
        println!("  writes:       {}", targets.writes.join(", "));
    }

    println!();
    match &auth.judgement {
        Judgement::Permit => println!("  decision:     PERMIT"),
        Judgement::RequireHuman { action, reason } => {
            println!("  decision:     NEEDS HUMAN APPROVAL");
            println!("  rule:         sovereign:{}", format!("{action:?}").to_lowercase());
            println!("  because:      {reason}");
        }
        Judgement::Deny { rule, reason } => {
            println!("  decision:     DENY");
            println!("  rule:         {rule}");
            println!("  because:      {reason}");
        }
    }

    // What would actually happen right now, which is not the same thing under
    // observe mode. Saying only "DENY" while the command in fact runs would
    // make the whole tool untrustworthy.
    println!("  effect now:   {}", obligation_label(&auth.obligation));
    if matches!(auth.obligation, Obligation::Proceed) && !matches!(auth.judgement, Judgement::Permit)
    {
        println!(
            "                (observe mode: recorded as a would-be block, not enforced)"
        );
    }

    if let Some(fix) = remediation(&auth.judgement, &caps) {
        println!();
        println!("  to proceed:   {fix}");
    }
    Ok(())
}

/// Concrete next step for a refused action.
///
/// A denial without a remediation is how you get shadow-IT workarounds: people
/// route around a control they cannot satisfy, and the control ends up
/// measuring nothing.
fn remediation(judgement: &Judgement, caps: &[Capability]) -> Option<String> {
    match judgement {
        Judgement::Permit => None,
        Judgement::RequireHuman { .. } => Some(
            "run it interactively and approve at the prompt; \
             unattended sessions cannot approve a sovereign action"
                .to_owned(),
        ),
        Judgement::Deny { rule, .. } if rule == "pep.session.frozen" => Some(
            "a budget was exceeded; a human must clear the freeze \
             (see `ainxt policy status`)"
                .to_owned(),
        ),
        Judgement::Deny { rule, .. } if rule == "pep.artifact.untrusted_exec" => Some(
            "this file arrived from an untrusted origin; review it, then have a \
             human run it explicitly"
                .to_owned(),
        ),
        Judgement::Deny { .. } => {
            if caps.contains(&Capability::Download)
                && caps.contains(&Capability::ShellInterpretation)
            {
                return Some(
                    "download and execution are combined in one step; fetch to a file, \
                     review it, then run it separately"
                        .to_owned(),
                );
            }
            Some(
                "ask an administrator to add this program or destination to the policy \
                 allowlist, or use an already-permitted equivalent (`ainxt policy show`)"
                    .to_owned(),
            )
        }
    }
}

fn capability_list(caps: &[Capability]) -> String {
    if caps.is_empty() {
        return "(none derived)".to_owned();
    }
    caps.iter()
        .map(|c| {
            if c.is_high_risk() {
                format!("{}*", c.as_str())
            } else {
                c.as_str().to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_allowlist(label: &str, list: &Allowlist) {
    match list {
        Allowlist::Any => println!("{label}: any (no constraint)"),
        Allowlist::Only(entries) if entries.is_empty() => {
            println!("{label}: NOTHING permitted");
        }
        Allowlist::Only(entries) => {
            println!("{label} ({}):", entries.len());
            for entry in entries {
                println!("  - {entry}");
            }
        }
    }
    println!();
}

fn print_denylist(label: &str, list: &Denylist) {
    if list.0.is_empty() {
        println!("{label}: (none)");
    } else {
        println!("{label} ({}):", list.0.len());
        for entry in &list.0 {
            println!("  - {entry}");
        }
    }
    println!();
}

fn obligation_label(obligation: &Obligation) -> &'static str {
    match obligation {
        Obligation::Proceed => "runs",
        Obligation::Prompt { .. } => "prompts for approval",
        Obligation::Refuse { .. } => "refused",
    }
}

fn posture_label(posture: Enforcement) -> &'static str {
    match posture {
        Enforcement::Off => "off (nothing enforced)",
        Enforcement::Warn => "observe (evaluated and recorded, not enforced)",
        Enforcement::Block => "block (enforced)",
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
