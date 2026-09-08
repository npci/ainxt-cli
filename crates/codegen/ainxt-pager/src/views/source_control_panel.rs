//! Source-control changed-files panel (Phase B).
//!
//! A dismissible overlay that lists the files changed in the git working
//! tree (relative to `HEAD`) and, when a file is selected, shows its diff.
//! Phase C adds staging (`space`) and a commit sub-view (`c`) with an
//! AI-suggested, fully-editable commit message ([`CommitState`]).
//!
//! Lifecycle mirrors the `/btw` inline panel ([`crate::views::btw_overlay`]):
//! a `LoadState` state machine (Loading / Loaded / Error) that is populated by
//! async ACP replies, with an `agent_generation` guard so a stale reply for a
//! panel that was closed and reopened is dropped (see
//! [`SourceControlState::accepts_generation`]).
//!
//! The file list + `j/k` navigation + click-to-select is modeled on the
//! `/rewind` picker ([`crate::views::rewind`]) via the shared
//! [`crate::views::overlay_list::ListOverlay`] geometry, so the render,
//! hit-test, and height paths cannot drift.
//!
//! The diff view reuses the standalone diff renderer
//! ([`crate::diff::diff_hunks_from_strings`] +
//! [`crate::scrollback::blocks::tool::edit::render_diff_hunks_highlighted`]),
//! the exact same path used for tool-call edit previews.

use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};
use unicode_width::UnicodeWidthStr;

use ainxt_ratatui_textarea::{TextArea, TextAreaState};
use ainxt_workspace::session::git::ChangeType;

use crate::scrollback::blocks::tool::{DiffRenderConfig, render_diff_hunks_highlighted};
use crate::theme::Theme;

/// Which source the panel is attributing changes to.
pub use crate::app::agent_view::SourceControlMode;

/// Async load state for the changed-files list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    /// Waiting for the `ainxt.dev/git/status` reply.
    Loading,
    /// Status arrived (files may be empty — a clean tree).
    Loaded,
    /// The status request failed.
    Error(String),
}

/// Which sub-view of the panel is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelView {
    /// The scrollable list of changed files.
    List,
    /// The diff for the selected file.
    Diff,
    /// The commit message editor (Phase C).
    Commit,
}

/// State for the commit sub-view (Phase C): a multi-line message editor plus
/// the async suggestion / commit lifecycle flags.
#[derive(Debug)]
pub struct CommitState {
    /// The editable commit message field.
    pub textarea: TextArea,
    /// Render/scroll state for the textarea.
    pub textarea_state: TextAreaState,
    /// Whether the user has typed into the field. Guards the AI suggestion from
    /// clobbering user input: a suggestion is only applied while this is false.
    pub user_edited: bool,
    /// True while an `Effect::SuggestCommitMessage` is in flight (spinner).
    pub suggesting: bool,
    /// True while an `Effect::GitCommit` is in flight (block re-entry).
    pub committing: bool,
    /// Last error to show in the view (suggest or commit failure).
    pub error: Option<String>,
}

impl Clone for CommitState {
    fn clone(&self) -> Self {
        let mut textarea = TextArea::new();
        textarea.show_scrollbar = false;
        textarea.set_text(self.textarea.text());
        textarea.set_cursor(self.textarea.cursor());
        Self {
            textarea,
            textarea_state: self.textarea_state.clone(),
            user_edited: self.user_edited,
            suggesting: self.suggesting,
            committing: self.committing,
            error: self.error.clone(),
        }
    }
}

impl CommitState {
    /// A fresh commit editor with an empty message and a pending suggestion.
    pub fn new() -> Self {
        let mut textarea = TextArea::new();
        textarea.show_scrollbar = false;
        Self {
            textarea,
            textarea_state: TextAreaState::default(),
            user_edited: false,
            suggesting: true,
            committing: false,
            error: None,
        }
    }

    /// The current message text.
    pub fn message(&self) -> String {
        self.textarea.text().to_string()
    }

    /// Apply an AI suggestion — **only** if the user hasn't typed yet. Returns
    /// `true` if the suggestion was applied. Never clobbers user input.
    pub fn apply_suggestion(&mut self, message: &str) -> bool {
        self.suggesting = false;
        if self.user_edited || !self.textarea.text().is_empty() {
            return false;
        }
        self.textarea.set_text(message);
        self.textarea.set_cursor(message.len());
        true
    }

    /// Record that a suggestion request failed (no clobber; user types manually).
    pub fn suggestion_failed(&mut self, error: String) {
        self.suggesting = false;
        // Only surface the "couldn't suggest" note if the field is still empty;
        // if the user already started typing it isn't worth interrupting them.
        if !self.user_edited && self.textarea.text().is_empty() {
            self.error = Some(error);
        }
    }
}

impl Default for CommitState {
    fn default() -> Self {
        Self::new()
    }
}

/// One changed file in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRow {
    pub path: String,
    pub change_type: ChangeType,
    pub additions: u64,
    pub deletions: u64,
    /// Whether staged. `None` when not applicable. (Phase C uses this.)
    pub staged: Option<bool>,
}

impl ChangeRow {
    /// Single-character status glyph (git short-status style).
    pub fn glyph(&self) -> char {
        match self.change_type {
            ChangeType::Create | ChangeType::Untracked => 'A',
            ChangeType::Delete => 'D',
            ChangeType::Rename => 'R',
            ChangeType::Copy => 'C',
            ChangeType::Typechange => 'T',
            ChangeType::Edit => 'M',
        }
    }

    /// Themed color for the status glyph: added = green, deleted = red,
    /// everything else (modified/rename/…) = yellow.
    pub fn glyph_color(&self, theme: &Theme) -> Color {
        match self.change_type {
            ChangeType::Create | ChangeType::Untracked => theme.accent_success,
            ChangeType::Delete => theme.accent_error,
            _ => theme.warning,
        }
    }
}

/// The loaded diff for the selected file, ready to render.
#[derive(Debug, Clone)]
pub struct DiffContent {
    /// Path the diff belongs to (guards against a stale reply for a file the
    /// user has since navigated away from).
    pub path: String,
    /// Precomputed hunks from the old/new text.
    pub hunks: Vec<crate::diff::DiffHunk>,
    /// Vertical scroll offset (in rendered lines).
    pub scroll_offset: usize,
}

/// Loading state for the diff sub-view.
#[derive(Debug, Clone)]
pub enum DiffState {
    /// Waiting for `ainxt.dev/git/diffs`.
    Loading { path: String },
    /// Diff ready.
    Ready(DiffContent),
    /// Diff failed (e.g. size-limit exceeded). Shown, not crashed.
    Error { path: String, error: String },
}

impl DiffState {
    pub fn path(&self) -> &str {
        match self {
            DiffState::Loading { path } | DiffState::Error { path, .. } => path,
            DiffState::Ready(c) => &c.path,
        }
    }
}

/// The panel's full state. Mirrors `BtwOverlayState`'s Loading/Done/Error plus
/// a monotonic generation guard for stale async results.
#[derive(Debug, Clone)]
pub struct SourceControlState {
    pub mode: SourceControlMode,
    pub load: LoadState,
    pub files: Vec<ChangeRow>,
    pub selected: usize,
    pub view: PanelView,
    pub diff: Option<DiffState>,
    /// Commit sub-view state (Phase C). `Some` only while in `PanelView::Commit`.
    pub commit: Option<CommitState>,
    /// Optional branch name for the header.
    pub branch: Option<String>,
    /// Generation this panel was opened at. Async replies carrying a
    /// different generation are dropped (the panel was closed / reopened).
    pub agent_generation: u64,
    /// First visible row for list scrolling (kept in sync by render).
    pub scroll_offset: usize,
}

impl SourceControlState {
    /// Build a fresh panel in the `Loading` state for `mode`.
    pub fn loading(mode: SourceControlMode, agent_generation: u64) -> Self {
        Self {
            mode,
            load: LoadState::Loading,
            files: Vec::new(),
            selected: 0,
            view: PanelView::List,
            diff: None,
            commit: None,
            branch: None,
            agent_generation,
            scroll_offset: 0,
        }
    }

    /// Count of currently-staged files (used by the commit-view summary).
    pub fn staged_count(&self) -> usize {
        self.files.iter().filter(|f| f.staged == Some(true)).count()
    }

    /// Toggle stage/unstage on the selected file. Returns
    /// `Some((path, want_staged))` describing the intent (`want_staged == true`
    /// means "stage it"), or `None` when there is no selectable file. The
    /// staged flag is optimistically flipped so the row updates before the
    /// status re-fetch lands.
    pub fn toggle_selected_stage(&mut self) -> Option<(String, bool)> {
        if self.view != PanelView::List {
            return None;
        }
        let file = self.files.get_mut(self.selected)?;
        let currently_staged = file.staged.unwrap_or(false);
        let want_staged = !currently_staged;
        file.staged = Some(want_staged);
        Some((file.path.clone(), want_staged))
    }

    /// Enter the commit sub-view with a fresh editor. Returns `false` if there
    /// is nothing staged to commit (caller shows a toast).
    pub fn open_commit(&mut self) -> bool {
        if self.staged_count() == 0 {
            return false;
        }
        self.view = PanelView::Commit;
        self.commit = Some(CommitState::new());
        true
    }

    /// Leave the commit sub-view back to the list, discarding the editor.
    pub fn close_commit(&mut self) {
        if self.view == PanelView::Commit {
            self.view = PanelView::List;
            self.commit = None;
        }
    }

    /// True if `generation` matches the panel's open generation (i.e. the
    /// async reply is not stale). Panels opened with a mismatched generation
    /// have been closed/reopened and any in-flight reply must be dropped.
    pub fn accepts_generation(&self, generation: u64) -> bool {
        self.agent_generation == generation
    }

    /// Populate the file list from a status reply and enter `Loaded`.
    pub fn set_files(&mut self, files: Vec<ChangeRow>, branch: Option<String>) {
        self.files = files;
        self.branch = branch;
        self.load = LoadState::Loaded;
        if self.selected >= self.files.len() {
            self.selected = self.files.len().saturating_sub(1);
        }
    }

    /// Mark the status load as failed.
    pub fn set_error(&mut self, error: String) {
        self.load = LoadState::Error(error);
    }

    /// The currently selected file, if any.
    pub fn selected_file(&self) -> Option<&ChangeRow> {
        self.files.get(self.selected)
    }

    /// Move the list cursor by `delta` rows, clamped. No-op in Diff view.
    pub fn move_selection(&mut self, delta: isize) {
        if self.view != PanelView::List || self.files.is_empty() {
            return;
        }
        let len = self.files.len() as isize;
        let cur = self.selected as isize;
        let next = (cur + delta).clamp(0, len - 1);
        self.selected = next as usize;
    }

    /// Enter the diff sub-view for the selected file, in `Loading` state.
    /// Returns the path to fetch (`None` if no file is selected).
    pub fn open_selected_diff(&mut self) -> Option<String> {
        let path = self.selected_file()?.path.clone();
        self.view = PanelView::Diff;
        self.diff = Some(DiffState::Loading { path: path.clone() });
        Some(path)
    }

    /// Store a loaded diff, if it still matches the file we're viewing and the
    /// panel is in the Diff view. Stale diffs (user navigated away) are dropped.
    pub fn set_diff(&mut self, path: String, hunks: Vec<crate::diff::DiffHunk>) {
        if self.view != PanelView::Diff {
            return;
        }
        let matches = self
            .diff
            .as_ref()
            .map(|d| d.path() == path)
            .unwrap_or(false);
        if !matches {
            return;
        }
        self.diff = Some(DiffState::Ready(DiffContent {
            path,
            hunks,
            scroll_offset: 0,
        }));
    }

    /// Store a diff error, if it still matches the file we're viewing.
    pub fn set_diff_error(&mut self, path: String, error: String) {
        if self.view != PanelView::Diff {
            return;
        }
        let matches = self
            .diff
            .as_ref()
            .map(|d| d.path() == path)
            .unwrap_or(false);
        if !matches {
            return;
        }
        self.diff = Some(DiffState::Error { path, error });
    }

    /// Go back from the diff view to the list. Returns `true` if a transition
    /// happened; `false` means we're already at the list (caller should close).
    pub fn back_to_list(&mut self) -> bool {
        if self.view == PanelView::Diff {
            self.view = PanelView::List;
            self.diff = None;
            true
        } else {
            false
        }
    }

    /// Scroll the diff up by `n` lines. No-op unless a Ready diff is showing.
    pub fn diff_scroll_up(&mut self, n: usize) {
        if let Some(DiffState::Ready(c)) = self.diff.as_mut() {
            c.scroll_offset = c.scroll_offset.saturating_sub(n);
        }
    }

    /// Scroll the diff down by `n` lines, clamped to `max`.
    pub fn diff_scroll_down(&mut self, n: usize, max: usize) {
        if let Some(DiffState::Ready(c)) = self.diff.as_mut() {
            c.scroll_offset = (c.scroll_offset + n).min(max);
        }
    }
}

/// Panel geometry: the overlay occupies a centered rect. Returns the rect the
/// panel should be painted into, given the full agent-view `area`.
pub fn panel_area(area: Rect) -> Rect {
    // Wide, tall overlay leaving a small margin — like goal_detail.
    let width = area.width.saturating_sub(4).min(100).max(20.min(area.width));
    let height = area
        .height
        .saturating_sub(2)
        .clamp(6.min(area.height), 40.min(area.height.max(6)));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Rows available for the file list inside `area` (borders + header excluded).
fn list_visible_rows(area: Rect) -> usize {
    // top border(1) + header(1) + bottom border(1) = 3 chrome rows.
    area.height.saturating_sub(3) as usize
}

/// First visible list row that keeps `selected` in view.
fn list_scroll_offset(selected: usize, visible: usize) -> usize {
    if visible > 0 && selected >= visible {
        selected - visible + 1
    } else {
        0
    }
}

/// Hit-test a screen position against the file-list rows.
///
/// Returns the file index under `(col, row)`, or `None` off the rows / when the
/// panel is not in the List view. Geometry matches [`render_source_control_panel`].
pub fn source_control_row_at(
    state: &SourceControlState,
    area: Rect,
    col: u16,
    row: u16,
) -> Option<usize> {
    if state.view != PanelView::List {
        return None;
    }
    if area.height < 4 || area.width < 10 {
        return None;
    }
    if col < area.x || col >= area.x + area.width {
        return None;
    }
    if row < area.y || row >= area.y + area.height {
        return None;
    }
    // Rows start after top border(1) + header(1).
    let first = area.y + 2;
    if row < first {
        return None;
    }
    let visible = list_visible_rows(area);
    let rel = (row - first) as usize;
    if rel >= visible {
        return None;
    }
    let idx = list_scroll_offset(state.selected, visible) + rel;
    (idx < state.files.len()).then_some(idx)
}

/// Truncate `s` to at most `max` display columns, appending `…` if cut.
fn truncate_cols(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('\u{2026}');
    out
}

/// Render the source-control panel into `area`, returning the `[Esc]`/close
/// hit rect (top-right of the border) so the caller can wire a click target.
pub fn render_source_control_panel(
    buf: &mut Buffer,
    state: &mut SourceControlState,
    area: Rect,
    tick: u64,
    focused: bool,
) -> Option<Rect> {
    if area.width < 12 || area.height < 4 {
        return None;
    }
    let theme = Theme::current();
    let bg = theme.bg_light;

    Clear.render(area, buf);
    buf.set_style(area, Style::default().bg(bg));

    let border_color = if focused {
        theme.accent_user
    } else {
        theme.gray_dim
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color).bg(bg))
        .style(Style::default().bg(bg))
        .render(area, buf);

    // ── Title in top border ──
    let mode_label = match state.mode {
        SourceControlMode::Git => "git",
        SourceControlMode::Session => "session",
    };
    let branch = state.branch.as_deref().unwrap_or("");
    let title = if branch.is_empty() {
        format!(" Changes ({mode_label}) ")
    } else {
        format!(" Changes ({mode_label}) · {branch} ")
    };
    let title_x = area.x + 2;
    let title_style = Style::default()
        .fg(theme.accent_user)
        .bg(bg)
        .add_modifier(Modifier::BOLD);

    // ── [Esc] / [g toggle] hint on the top border (right) ──
    let hint = match state.view {
        PanelView::List => " [space] stage  [c] commit  [Esc] ".to_string(),
        PanelView::Diff => " [h/Esc] back ".to_string(),
        PanelView::Commit => " [^S] commit  [Esc] cancel ".to_string(),
    };
    let hint_w = hint.width() as u16;
    let hint_x = (area.x + area.width).saturating_sub(1 + hint_w);
    let is_hovered_close = false;
    let hint_style = if is_hovered_close {
        Style::default().fg(theme.text_primary).bg(bg)
    } else {
        Style::default().fg(theme.gray).bg(bg)
    };
    let max_title = hint_x.saturating_sub(title_x).saturating_sub(1) as usize;
    let title_text = truncate_cols(&title, max_title);
    buf.set_line(
        title_x,
        area.y,
        &Line::from(Span::styled(title_text.clone(), title_style)),
        (title_text.width() as u16).min(hint_x.saturating_sub(title_x)),
    );
    let mut close_rect = None;
    if hint_x >= title_x {
        buf.set_line(
            hint_x,
            area.y,
            &Line::from(Span::styled(hint, hint_style)),
            hint_w,
        );
        close_rect = Some(Rect {
            x: hint_x,
            y: area.y,
            width: hint_w,
            height: 1,
        });
    }

    let content_x = area.x + 2;
    let content_w = area.width.saturating_sub(4);
    let header_y = area.y + 1;
    let body_y = area.y + 2;

    match state.view {
        PanelView::List => render_list_body(
            buf, state, &theme, bg, content_x, content_w, header_y, body_y, area, tick,
        ),
        PanelView::Diff => render_diff_body(
            buf, state, &theme, bg, content_x, content_w, header_y, body_y, area, tick,
        ),
        PanelView::Commit => render_commit_body(
            buf, state, &theme, bg, content_x, content_w, header_y, body_y, area, tick,
        ),
    }

    close_rect
}

#[allow(clippy::too_many_arguments)]
fn render_commit_body(
    buf: &mut Buffer,
    state: &mut SourceControlState,
    theme: &Theme,
    bg: Color,
    content_x: u16,
    content_w: u16,
    header_y: u16,
    body_y: u16,
    area: Rect,
    tick: u64,
) {
    let staged = state.staged_count();
    let Some(commit) = state.commit.as_mut() else {
        return;
    };

    // Header: staged-file summary (+ spinner while suggesting).
    let header = if commit.suggesting {
        let frames = crate::glyphs::braille_spinner_frames();
        let f = frames[((tick / 4) % frames.len() as u64) as usize];
        format!("{f} Commit {staged} staged file(s) · suggesting message\u{2026}")
    } else if commit.committing {
        let frames = crate::glyphs::braille_spinner_frames();
        let f = frames[((tick / 4) % frames.len() as u64) as usize];
        format!("{f} Committing {staged} staged file(s)\u{2026}")
    } else {
        format!("Commit {staged} staged file(s)")
    };
    buf.set_line(
        content_x,
        header_y,
        &Line::from(Span::styled(
            truncate_cols(&header, content_w as usize),
            Style::default().fg(theme.gray).bg(bg),
        )),
        content_w,
    );

    // Layout: editor fills the body minus a 1-row hint line at the bottom (and
    // an optional 1-row error line above the hint).
    let body_h = area.height.saturating_sub(3); // borders + header
    if body_h == 0 {
        return;
    }
    let has_error = commit.error.is_some();
    let hint_rows: u16 = 1 + if has_error { 1 } else { 0 };
    let editor_h = body_h.saturating_sub(hint_rows).max(1);
    let editor_area = Rect {
        x: content_x,
        y: body_y,
        width: content_w,
        height: editor_h,
    };
    // Paint the editor background so it reads as a field.
    for y in editor_area.y..editor_area.y + editor_area.height {
        for x in editor_area.x..editor_area.x + editor_area.width {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_bg(theme.bg_dark);
            }
        }
    }
    use ratatui::widgets::StatefulWidgetRef;
    (&commit.textarea).render_ref(editor_area, buf, &mut commit.textarea_state);

    let mut y = body_y + editor_h;
    if let Some(err) = &commit.error {
        buf.set_line(
            content_x,
            y,
            &Line::from(Span::styled(
                truncate_cols(err, content_w as usize),
                Style::default().fg(theme.accent_error).bg(bg),
            )),
            content_w,
        );
        y += 1;
    }
    let hint = "[^S] commit   [Esc] cancel   type to edit";
    buf.set_line(
        content_x,
        y,
        &Line::from(Span::styled(
            truncate_cols(hint, content_w as usize),
            Style::default().fg(theme.gray_dim).bg(bg),
        )),
        content_w,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_list_body(
    buf: &mut Buffer,
    state: &mut SourceControlState,
    theme: &Theme,
    bg: Color,
    content_x: u16,
    content_w: u16,
    header_y: u16,
    body_y: u16,
    area: Rect,
    tick: u64,
) {
    // Header row.
    let header = match &state.load {
        LoadState::Loading => {
            let frames = crate::glyphs::braille_spinner_frames();
            let f = frames[((tick / 4) % frames.len() as u64) as usize];
            format!("{f} Loading changes\u{2026}")
        }
        LoadState::Error(e) => format!("Error: {e}"),
        LoadState::Loaded => {
            if state.files.is_empty() {
                if state.mode == SourceControlMode::Session {
                    // TODO(Phase B/C): no session-changes backend yet.
                    "Session view coming soon".to_string()
                } else {
                    "No changes — working tree clean".to_string()
                }
            } else {
                format!("{} changed file(s)", state.files.len())
            }
        }
    };
    let header_style = match &state.load {
        LoadState::Error(_) => Style::default().fg(theme.accent_error).bg(bg),
        _ => Style::default().fg(theme.gray).bg(bg),
    };
    buf.set_line(
        content_x,
        header_y,
        &Line::from(Span::styled(truncate_cols(&header, content_w as usize), header_style)),
        content_w,
    );

    if state.load != LoadState::Loaded || state.files.is_empty() {
        return;
    }

    let visible = list_visible_rows(area);
    let offset = list_scroll_offset(state.selected, visible);
    state.scroll_offset = offset;
    let end = (offset + visible).min(state.files.len());

    for (row, idx) in (offset..end).enumerate() {
        let file = &state.files[idx];
        let y = body_y + row as u16;
        let is_cursor = idx == state.selected;
        let row_bg = if is_cursor { theme.bg_visual } else { bg };
        // Paint the full-width row background.
        for x in area.x + 1..area.x + area.width - 1 {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_bg(row_bg);
            }
        }

        let glyph = file.glyph();
        let glyph_style = Style::default().fg(file.glyph_color(theme)).bg(row_bg);
        let stats = format!("+{} -{}", file.additions, file.deletions);
        let stats_w = stats.width() as u16;
        // path width = content minus "X " (2) minus stats + gap.
        let path_avail = content_w.saturating_sub(2 + stats_w + 2) as usize;
        let path = truncate_cols(&file.path, path_avail.max(1));
        let path_style = if is_cursor {
            Style::default().fg(theme.text_primary).bg(row_bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_primary).bg(row_bg)
        };
        let line = Line::from(vec![
            Span::styled(format!("{glyph} "), glyph_style),
            Span::styled(path, path_style),
        ]);
        buf.set_line(content_x, y, &line, content_w);

        // Right-align the +/- stats.
        let stats_x = (area.x + area.width).saturating_sub(2 + stats_w);
        let add_span = Span::styled(
            format!("+{}", file.additions),
            Style::default().fg(theme.accent_success).bg(row_bg),
        );
        let del_span = Span::styled(
            format!(" -{}", file.deletions),
            Style::default().fg(theme.accent_error).bg(row_bg),
        );
        buf.set_line(
            stats_x,
            y,
            &Line::from(vec![add_span, del_span]),
            stats_w + 1,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_diff_body(
    buf: &mut Buffer,
    state: &mut SourceControlState,
    theme: &Theme,
    bg: Color,
    content_x: u16,
    content_w: u16,
    header_y: u16,
    body_y: u16,
    area: Rect,
    tick: u64,
) {
    let Some(diff) = state.diff.as_mut() else {
        return;
    };
    // Header: file path.
    let (header, header_style) = match &diff {
        DiffState::Loading { path } => {
            let frames = crate::glyphs::braille_spinner_frames();
            let f = frames[((tick / 4) % frames.len() as u64) as usize];
            (
                format!("{f} {path}"),
                Style::default().fg(theme.gray).bg(bg),
            )
        }
        DiffState::Error { path, .. } => (
            path.clone(),
            Style::default().fg(theme.text_primary).bg(bg).add_modifier(Modifier::BOLD),
        ),
        DiffState::Ready(c) => (
            c.path.clone(),
            Style::default().fg(theme.text_primary).bg(bg).add_modifier(Modifier::BOLD),
        ),
    };
    buf.set_line(
        content_x,
        header_y,
        &Line::from(Span::styled(truncate_cols(&header, content_w as usize), header_style)),
        content_w,
    );

    let body_h = area.height.saturating_sub(3) as usize; // borders + header

    match diff {
        DiffState::Loading { .. } => {}
        DiffState::Error { error, .. } => {
            let msg = truncate_cols(error, content_w as usize);
            buf.set_line(
                content_x,
                body_y,
                &Line::from(Span::styled(
                    msg,
                    Style::default().fg(theme.accent_error).bg(bg),
                )),
                content_w,
            );
        }
        DiffState::Ready(c) => {
            let path = PathBuf::from(&c.path);
            let cfg = DiffRenderConfig::default();
            let outputs =
                render_diff_hunks_highlighted(&c.hunks, &path, theme, content_w, &cfg);
            let total = outputs.len();
            let max_off = total.saturating_sub(body_h);
            let off = c.scroll_offset.min(max_off);
            c.scroll_offset = off;
            let end = (off + body_h).min(total);
            for (row, out) in outputs[off..end].iter().enumerate() {
                let y = body_y + row as u16;
                if let Some(color) = out.background {
                    for x in area.x + 1..area.x + area.width - 1 {
                        if let Some(cell) = buf.cell_mut((x, y)) {
                            cell.set_bg(color);
                        }
                    }
                }
                buf.set_line(content_x, y, &out.line, content_w);
            }
        }
    }
}

/// Max diff scroll offset for the Ready diff at `area`, for input clamping.
pub fn diff_max_scroll(state: &SourceControlState, area: Rect) -> usize {
    let Some(DiffState::Ready(c)) = state.diff.as_ref() else {
        return 0;
    };
    let theme = Theme::current();
    let content_w = area.width.saturating_sub(4);
    let body_h = area.height.saturating_sub(3) as usize;
    let path = PathBuf::from(&c.path);
    let cfg = DiffRenderConfig::default();
    let total = render_diff_hunks_highlighted(&c.hunks, &path, &theme, content_w, &cfg).len();
    total.saturating_sub(body_h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, ct: ChangeType, add: u64, del: u64) -> ChangeRow {
        ChangeRow {
            path: path.to_string(),
            change_type: ct,
            additions: add,
            deletions: del,
            staged: None,
        }
    }

    fn loaded_state(files: Vec<ChangeRow>) -> SourceControlState {
        let mut s = SourceControlState::loading(SourceControlMode::Git, 0);
        s.set_files(files, Some("main".into()));
        s
    }

    #[test]
    fn glyph_and_color_map_by_change_type() {
        let theme = Theme::current();
        let m = row("a.rs", ChangeType::Edit, 1, 1);
        assert_eq!(m.glyph(), 'M');
        assert_eq!(m.glyph_color(&theme), theme.warning);
        let a = row("b.rs", ChangeType::Create, 3, 0);
        assert_eq!(a.glyph(), 'A');
        assert_eq!(a.glyph_color(&theme), theme.accent_success);
        let d = row("c.rs", ChangeType::Delete, 0, 5);
        assert_eq!(d.glyph(), 'D');
        assert_eq!(d.glyph_color(&theme), theme.accent_error);
    }

    #[test]
    fn generation_guard_rejects_stale() {
        let s = SourceControlState::loading(SourceControlMode::Git, 7);
        assert!(s.accepts_generation(7));
        assert!(!s.accepts_generation(8));
    }

    #[test]
    fn move_selection_clamps() {
        let mut s = loaded_state(vec![
            row("a", ChangeType::Edit, 1, 0),
            row("b", ChangeType::Edit, 1, 0),
            row("c", ChangeType::Edit, 1, 0),
        ]);
        assert_eq!(s.selected, 0);
        s.move_selection(-1);
        assert_eq!(s.selected, 0, "clamps at top");
        s.move_selection(2);
        assert_eq!(s.selected, 2);
        s.move_selection(5);
        assert_eq!(s.selected, 2, "clamps at bottom");
    }

    #[test]
    fn open_diff_transitions_to_diff_view() {
        let mut s = loaded_state(vec![row("src/x.rs", ChangeType::Edit, 2, 1)]);
        let path = s.open_selected_diff().expect("path");
        assert_eq!(path, "src/x.rs");
        assert_eq!(s.view, PanelView::Diff);
        assert!(matches!(s.diff, Some(DiffState::Loading { .. })));
        // Back returns to list and clears diff.
        assert!(s.back_to_list());
        assert_eq!(s.view, PanelView::List);
        assert!(s.diff.is_none());
        // Back again at list is a no-op (caller closes).
        assert!(!s.back_to_list());
    }

    #[test]
    fn set_diff_drops_stale_path() {
        let mut s = loaded_state(vec![row("a.rs", ChangeType::Edit, 1, 1)]);
        s.open_selected_diff();
        // A reply for a different file must be ignored.
        s.set_diff("other.rs".into(), vec![]);
        assert!(matches!(s.diff, Some(DiffState::Loading { .. })));
        // A reply for the right file lands.
        s.set_diff("a.rs".into(), vec![]);
        assert!(matches!(s.diff, Some(DiffState::Ready(_))));
    }

    #[test]
    fn set_files_clamps_selection() {
        let mut s = loaded_state(vec![
            row("a", ChangeType::Edit, 1, 0),
            row("b", ChangeType::Edit, 1, 0),
        ]);
        s.selected = 1;
        // Reload with fewer files.
        s.set_files(vec![row("a", ChangeType::Edit, 1, 0)], None);
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn row_hit_test_matches_list_geometry() {
        let s = loaded_state(vec![
            row("a", ChangeType::Edit, 1, 0),
            row("b", ChangeType::Edit, 1, 0),
            row("c", ChangeType::Edit, 1, 0),
        ]);
        let area = Rect::new(0, 0, 40, 8);
        // top border=row0, header=row1, first file row=row2.
        assert_eq!(source_control_row_at(&s, area, 5, 1), None, "header row");
        assert_eq!(source_control_row_at(&s, area, 5, 2), Some(0));
        assert_eq!(source_control_row_at(&s, area, 5, 3), Some(1));
        assert_eq!(source_control_row_at(&s, area, 5, 4), Some(2));
        // Past the last file — none.
        assert_eq!(source_control_row_at(&s, area, 5, 5), None);
        // Outside the panel horizontally.
        assert_eq!(source_control_row_at(&s, area, 99, 2), None);
    }

    #[test]
    fn row_hit_test_none_in_diff_view() {
        let mut s = loaded_state(vec![row("a", ChangeType::Edit, 1, 0)]);
        s.open_selected_diff();
        let area = Rect::new(0, 0, 40, 8);
        assert_eq!(source_control_row_at(&s, area, 5, 2), None);
    }

    #[test]
    fn error_state_sets_load_error() {
        let mut s = SourceControlState::loading(SourceControlMode::Git, 0);
        s.set_error("boom".into());
        assert_eq!(s.load, LoadState::Error("boom".into()));
    }

    fn staged_row(path: &str, staged: bool) -> ChangeRow {
        ChangeRow {
            path: path.to_string(),
            change_type: ChangeType::Edit,
            additions: 1,
            deletions: 0,
            staged: Some(staged),
        }
    }

    // ── Phase C ──────────────────────────────────────────────────────────

    #[test]
    fn toggle_selected_stage_flips_and_reports_intent() {
        let mut s = loaded_state(vec![staged_row("a.rs", false), staged_row("b.rs", true)]);
        // First file unstaged → toggle stages it.
        let (path, want_staged) = s.toggle_selected_stage().expect("intent");
        assert_eq!(path, "a.rs");
        assert!(want_staged, "unstaged file should be staged");
        assert_eq!(s.files[0].staged, Some(true), "optimistic flip");
        // Move to the staged file → toggle unstages it.
        s.move_selection(1);
        let (path, want_staged) = s.toggle_selected_stage().expect("intent");
        assert_eq!(path, "b.rs");
        assert!(!want_staged, "staged file should be unstaged");
        assert_eq!(s.files[1].staged, Some(false));
    }

    #[test]
    fn toggle_selected_stage_none_off_list_view() {
        let mut s = loaded_state(vec![staged_row("a.rs", false)]);
        s.view = PanelView::Diff;
        assert!(s.toggle_selected_stage().is_none());
    }

    #[test]
    fn open_commit_requires_staged_files() {
        // Nothing staged → open_commit returns false, stays in List.
        let mut s = loaded_state(vec![staged_row("a.rs", false)]);
        assert!(!s.open_commit());
        assert_eq!(s.view, PanelView::List);
        assert!(s.commit.is_none());
        // One staged → transitions to Commit with a fresh (suggesting) editor.
        let mut s = loaded_state(vec![staged_row("a.rs", true)]);
        assert!(s.open_commit());
        assert_eq!(s.view, PanelView::Commit);
        let commit = s.commit.as_ref().expect("commit state");
        assert!(commit.suggesting);
        assert!(!commit.user_edited);
        assert!(commit.message().is_empty());
        // close_commit returns to List and drops the editor.
        s.close_commit();
        assert_eq!(s.view, PanelView::List);
        assert!(s.commit.is_none());
    }

    #[test]
    fn suggestion_prefills_only_when_user_has_not_typed() {
        let mut c = CommitState::new();
        // Empty + not edited → suggestion is applied.
        assert!(c.apply_suggestion("feat: add thing"));
        assert_eq!(c.message(), "feat: add thing");
        assert!(!c.suggesting);
    }

    #[test]
    fn suggestion_never_clobbers_user_input() {
        let mut c = CommitState::new();
        // Simulate the user typing before the suggestion lands.
        c.textarea.set_text("my own message");
        c.user_edited = true;
        assert!(
            !c.apply_suggestion("feat: something else"),
            "must not apply over user input"
        );
        assert_eq!(c.message(), "my own message");
    }

    #[test]
    fn suggestion_failure_only_notes_when_field_empty() {
        // Field empty, not edited → show the "couldn't suggest" note.
        let mut c = CommitState::new();
        c.suggestion_failed("couldn't suggest".into());
        assert!(!c.suggesting);
        assert_eq!(c.error.as_deref(), Some("couldn't suggest"));
        // User already typing → don't interrupt with an error.
        let mut c = CommitState::new();
        c.textarea.set_text("wip");
        c.user_edited = true;
        c.suggestion_failed("couldn't suggest".into());
        assert!(c.error.is_none());
    }

    #[test]
    fn staged_count_counts_only_staged() {
        let s = loaded_state(vec![
            staged_row("a", true),
            staged_row("b", false),
            staged_row("c", true),
        ]);
        assert_eq!(s.staged_count(), 2);
    }
}
