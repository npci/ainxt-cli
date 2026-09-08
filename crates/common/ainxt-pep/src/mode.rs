//! The **only** place in this crate that reads [`Enforcement`].
//!
//! Keeping the posture in one total function is what makes observe mode
//! trustworthy. If `if mode == Warn` appeared at a dozen decision sites, then
//! "observe changes nothing" would be a claim about a dozen scattered branches
//! — unprovable, and wrong the first time someone forgets one. Here it is a
//! property of a single pure function, and the test suite asserts both that the
//! function is a no-op under `Warn` and that this file is the sole reader.

use ainxt_policy_types::Enforcement;

use crate::{Judgement, Obligation};

/// Whether to evaluate at all.
///
/// `Off` is the OSS build with no bundle: no evaluation, no audit, no cost.
/// Both other postures evaluate and audit identically — they differ only in
/// what they *do* with the result.
pub(crate) fn evaluation_required(mode: Enforcement) -> bool {
    mode != Enforcement::Off
}

/// Project ground truth onto what the caller must do.
///
/// [`Judgement`] is computed with no knowledge of the posture; this is where
/// posture enters, and nowhere else.
pub(crate) fn obligate(judgement: &Judgement, mode: Enforcement) -> Obligation {
    match mode {
        // Evaluated and recorded, but never acted on. This is the rollout
        // posture: it produces the block-rate evidence needed to justify
        // flipping a department to `Block`, at zero risk of breaking anyone.
        Enforcement::Off | Enforcement::Warn => Obligation::Proceed,
        Enforcement::Block => match judgement {
            Judgement::Permit => Obligation::Proceed,
            Judgement::RequireHuman { reason, .. } => Obligation::Prompt {
                reason: reason.clone(),
                sovereign: true,
            },
            Judgement::Deny { reason, .. } => Obligation::Refuse {
                reason: reason.clone(),
            },
        },
    }
}
