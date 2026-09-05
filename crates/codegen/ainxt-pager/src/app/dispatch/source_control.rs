//! Source-control dispatchers (Phase B).
//!
//! Handles the clickable "changed files" status-bar indicator, the
//! `/changes` slash command, the Git/Session mode toggle, and the changed-files
//! panel + click-to-diff. This is **read-only**: staging/committing lands in
//! Phase C.
//!
//! Opening the panel fires an `Effect::GitStatus` (ACP `ainxt.dev/git/status`);
//! selecting a file fires an `Effect::GitDiff` (ACP `ainxt.dev/git/diffs`,
//! `HEAD`→`working` with content). Async replies are guarded against staleness
//! by an `agent_generation` counter derived from the panel's open identity —
//! see [`SourceControlState::accepts_generation`].

use super::ctx::get_active_agent_mut;
use crate::app::actions::{Effect, GitStatusPayload};
use crate::app::agent::AgentId;
use crate::app::agent_view::SourceControlMode;
use crate::app::app_view::{ActiveView, AppView};
use crate::views::source_control_panel::SourceControlState;

/// A monotonically-increasing generation for a fresh panel open. Each time the
/// panel is (re)opened or re-fetched we bump this so an in-flight reply for a
/// prior open is dropped. Global (not per-agent) is fine: it only needs to be
/// unique across overlapping requests.
fn next_generation() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static GEN: AtomicU64 = AtomicU64::new(1);
    GEN.fetch_add(1, Ordering::Relaxed)
}

/// Open the source-control changed-files panel and kick off a status fetch.
///
/// Sets the active agent's `source_control` to a fresh `Loading` panel and
/// returns an `Effect::GitStatus`. For [`SourceControlMode::Session`] there is
/// no backend yet (Phase B intentionally does not build `session_changes`), so
/// we still open the panel but leave a hint in the empty state; see
/// [`crate::views::source_control_panel`].
pub(super) fn dispatch_open_source_control(app: &mut AppView) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    let Some(session_id) = agent.session.session_id.clone() else {
        agent.show_toast("No active session");
        return vec![];
    };
    let mode = agent.source_control_mode;
    let generation = next_generation();
    agent.source_control = Some(SourceControlState::loading(mode, generation));

    match mode {
        SourceControlMode::Git => vec![Effect::GitStatus {
            agent_id: id,
            session_id,
            generation,
            include_untracked: true,
            include_stats: true,
        }],
        SourceControlMode::Session => {
            // TODO(Phase B/C): no session-scoped changes backend yet. Show an
            // empty list with a "Session view coming soon" hint (rendered by the
            // panel) instead of fetching. Do NOT build session_changes here.
            if let Some(sc) = agent.source_control.as_mut() {
                sc.set_files(Vec::new(), None);
            }
            vec![]
        }
    }
}

/// Flip the active [`SourceControlMode`] (Git ⇄ Session) for the current agent.
///
/// Updates the stored mode so the status-bar indicator relabels, and — when the
/// panel is open — re-fetches for the new mode (a `GitStatus` request for Git;
/// an empty "coming soon" list for Session, per the Phase B TODO).
pub(super) fn dispatch_source_control_toggle_mode(app: &mut AppView) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    agent.source_control_mode = match agent.source_control_mode {
        SourceControlMode::Git => SourceControlMode::Session,
        SourceControlMode::Session => SourceControlMode::Git,
    };
    let mode = agent.source_control_mode;

    // If the panel is open, re-fetch for the new mode.
    if agent.source_control.is_some() {
        let generation = next_generation();
        agent.source_control = Some(SourceControlState::loading(mode, generation));
        match mode {
            SourceControlMode::Git => {
                if let Some(session_id) = agent.session.session_id.clone() {
                    return vec![Effect::GitStatus {
                        agent_id: id,
                        session_id,
                        generation,
                        include_untracked: true,
                        include_stats: true,
                    }];
                }
            }
            SourceControlMode::Session => {
                // TODO(Phase B/C): session_changes backend not built yet.
                if let Some(sc) = agent.source_control.as_mut() {
                    sc.set_files(Vec::new(), None);
                }
            }
        }
    }
    vec![]
}

/// Select the file at `idx` in the panel and open its diff (fires an
/// `Effect::GitDiff` for the working-tree-vs-HEAD diff of that file).
pub(super) fn dispatch_source_control_select_file(app: &mut AppView, idx: usize) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    let Some(session_id) = agent.session.session_id.clone() else {
        return vec![];
    };
    let Some(sc) = agent.source_control.as_mut() else {
        return vec![];
    };
    if idx >= sc.files.len() {
        return vec![];
    }
    sc.selected = idx;
    let generation = sc.agent_generation;
    let Some(path) = sc.open_selected_diff() else {
        return vec![];
    };
    vec![Effect::GitDiff {
        agent_id: id,
        session_id,
        generation,
        path,
    }]
}

/// Handle a `TaskResult::GitStatusLoaded`: populate the panel's file list.
///
/// Dropped if the agent is gone, the panel was closed, or the reply's
/// `generation` no longer matches the open panel (stale — the panel was closed
/// and reopened, or the mode toggled and re-fetched).
pub(crate) fn handle_git_status_loaded(
    app: &mut AppView,
    agent_id: AgentId,
    generation: u64,
    result: Result<GitStatusPayload, String>,
) -> Vec<Effect> {
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    let Some(sc) = agent.source_control.as_mut() else {
        return vec![]; // panel closed
    };
    if !sc.accepts_generation(generation) {
        return vec![]; // stale reply
    }
    match result {
        Ok(payload) => sc.set_files(payload.files, payload.branch),
        Err(error) => sc.set_error(error),
    }
    vec![]
}

/// Handle a `TaskResult::GitDiffLoaded`: build hunks from the old/new text and
/// store the rendered-ready diff. Stale (generation mismatch / panel closed /
/// user navigated to a different file) results are dropped; a backend error
/// (e.g. diff-size-exceeded) is stored as a message, not a crash.
pub(crate) fn handle_git_diff_loaded(
    app: &mut AppView,
    agent_id: AgentId,
    generation: u64,
    path: String,
    result: Result<(String, String), String>,
) -> Vec<Effect> {
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    let Some(sc) = agent.source_control.as_mut() else {
        return vec![];
    };
    if !sc.accepts_generation(generation) {
        return vec![];
    }
    match result {
        Ok((old_text, new_text)) => {
            let hunks = crate::diff::diff_hunks_from_strings(&old_text, &new_text, 1);
            sc.set_diff(path, hunks);
        }
        Err(error) => sc.set_diff_error(path, error),
    }
    vec![]
}

/// Go back one level in the panel: commit → list, diff → list, or close the
/// panel when already at the list view.
pub(super) fn dispatch_source_control_back(app: &mut AppView) -> Vec<Effect> {
    if let Some(agent) = get_active_agent_mut(app)
        && let Some(sc) = agent.source_control.as_mut()
    {
        use crate::views::source_control_panel::PanelView;
        if sc.view == PanelView::Commit {
            sc.close_commit();
        } else if !sc.back_to_list() {
            // Already at the list — close the panel entirely.
            agent.source_control = None;
        }
    }
    vec![]
}

/// Re-fetch git status for the open panel, bumping the generation so any
/// in-flight status reply for the prior generation is dropped. Returns the
/// `GitStatus` effect (or nothing if the panel/session is gone).
fn refetch_status(app: &mut AppView) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    let Some(session_id) = agent.session.session_id.clone() else {
        return vec![];
    };
    let Some(sc) = agent.source_control.as_mut() else {
        return vec![];
    };
    let generation = next_generation();
    sc.agent_generation = generation;
    vec![Effect::GitStatus {
        agent_id: id,
        session_id,
        generation,
        include_untracked: true,
        include_stats: true,
    }]
}

/// Toggle stage/unstage on the selected file (Phase C, `space`). Optimistically
/// flips the row's staged flag, then fires the matching stage/unstage effect.
pub(super) fn dispatch_source_control_toggle_stage(app: &mut AppView) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    let Some(session_id) = agent.session.session_id.clone() else {
        return vec![];
    };
    let Some(sc) = agent.source_control.as_mut() else {
        return vec![];
    };
    let Some((path, want_staged)) = sc.toggle_selected_stage() else {
        return vec![];
    };
    let generation = sc.agent_generation;
    let paths = vec![path];
    if want_staged {
        vec![Effect::GitStage {
            agent_id: id,
            session_id,
            generation,
            paths,
        }]
    } else {
        vec![Effect::GitUnstage {
            agent_id: id,
            session_id,
            generation,
            paths,
        }]
    }
}

/// Open the commit sub-view (Phase C, `c`) and kick off an AI message
/// suggestion. Shows a toast (no view change) if nothing is staged.
pub(super) fn dispatch_source_control_open_commit(app: &mut AppView) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    let Some(session_id) = agent.session.session_id.clone() else {
        return vec![];
    };
    let Some(sc) = agent.source_control.as_mut() else {
        return vec![];
    };
    if !sc.open_commit() {
        agent.show_toast("Nothing staged to commit");
        return vec![];
    }
    let generation = sc.agent_generation;
    vec![Effect::SuggestCommitMessage {
        agent_id: id,
        session_id,
        generation,
    }]
}

/// Commit the current (edited) message (Phase C, `Ctrl+S`). No-op if the
/// message is blank or a commit is already in flight.
pub(super) fn dispatch_source_control_commit(app: &mut AppView) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    let Some(session_id) = agent.session.session_id.clone() else {
        return vec![];
    };
    let Some(sc) = agent.source_control.as_mut() else {
        return vec![];
    };
    let generation = sc.agent_generation;
    let Some(commit) = sc.commit.as_mut() else {
        return vec![];
    };
    if commit.committing {
        return vec![];
    }
    let message = commit.message();
    if message.trim().is_empty() {
        commit.error = Some("Enter a commit message first".to_string());
        return vec![];
    }
    commit.committing = true;
    commit.error = None;
    vec![Effect::GitCommit {
        agent_id: id,
        session_id,
        generation,
        message,
    }]
}

/// Route a key event to the commit-view message editor (Phase C). Marks the
/// field as user-edited on any content-changing key so the AI suggestion never
/// clobbers what the user typed.
pub(super) fn dispatch_source_control_edit_message(
    app: &mut AppView,
    key: crossterm::event::KeyEvent,
) -> Vec<Effect> {
    if let Some(agent) = get_active_agent_mut(app)
        && let Some(sc) = agent.source_control.as_mut()
        && let Some(commit) = sc.commit.as_mut()
    {
        let before = commit.textarea.text().to_string();
        commit.textarea.input(key);
        if commit.textarea.text() != before {
            commit.user_edited = true;
            // Clear any "couldn't suggest" note once the user starts typing.
            commit.error = None;
        }
    }
    vec![]
}

/// Handle a `TaskResult::GitStaged`: on success re-fetch status so staged /
/// unstaged state refreshes; on error surface a toast (the optimistic flip is
/// corrected by the re-fetch anyway).
pub(crate) fn handle_git_staged(
    app: &mut AppView,
    agent_id: AgentId,
    generation: u64,
    result: Result<(), String>,
) -> Vec<Effect> {
    {
        let Some(agent) = app.agents.get_mut(&agent_id) else {
            return vec![];
        };
        let Some(sc) = agent.source_control.as_mut() else {
            return vec![];
        };
        if !sc.accepts_generation(generation) {
            return vec![]; // stale reply
        }
        if let Err(error) = result {
            agent.show_toast(&format!("Stage failed: {error}"));
            // Fall through to a re-fetch to correct the optimistic flip.
        }
    }
    refetch_status(app)
}

/// Handle a `TaskResult::GitCommitDone`: on success toast + refresh status +
/// return to the list; on error show it in the commit view.
pub(crate) fn handle_git_commit_done(
    app: &mut AppView,
    agent_id: AgentId,
    generation: u64,
    result: Result<Option<String>, String>,
) -> Vec<Effect> {
    let refetch = {
        let Some(agent) = app.agents.get_mut(&agent_id) else {
            return vec![];
        };
        let Some(sc) = agent.source_control.as_mut() else {
            return vec![];
        };
        if !sc.accepts_generation(generation) {
            return vec![]; // stale reply
        }
        match result {
            Ok(short) => {
                let toast = match short {
                    Some(h) => format!("Committed {h}"),
                    None => "Committed".to_string(),
                };
                sc.close_commit();
                agent.show_toast(&toast);
                true
            }
            Err(error) => {
                if let Some(commit) = sc.commit.as_mut() {
                    commit.committing = false;
                    commit.error = Some(error);
                }
                false
            }
        }
    };
    if refetch { refetch_status(app) } else { vec![] }
}

/// Handle a `TaskResult::CommitMessageSuggested`: prefill the editor iff the
/// user hasn't typed (never clobbers user input); on error show "couldn't
/// suggest" (only when the field is still empty).
pub(crate) fn handle_commit_message_suggested(
    app: &mut AppView,
    agent_id: AgentId,
    generation: u64,
    result: Result<String, String>,
) -> Vec<Effect> {
    let Some(agent) = app.agents.get_mut(&agent_id) else {
        return vec![];
    };
    let Some(sc) = agent.source_control.as_mut() else {
        return vec![]; // panel closed
    };
    if !sc.accepts_generation(generation) {
        return vec![]; // stale reply
    }
    let Some(commit) = sc.commit.as_mut() else {
        return vec![]; // left the commit view
    };
    match result {
        Ok(message) => {
            commit.apply_suggestion(&message);
        }
        Err(error) => {
            commit.suggestion_failed(error);
        }
    }
    vec![]
}
