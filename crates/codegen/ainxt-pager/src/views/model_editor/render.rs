//! Rendering for the in-TUI `[model.*]` add/edit/remove screen.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::theme::Theme;
use crate::views::modal_window::{self as mw, ModalContentArea, ModalSizing, ModalWindowConfig, Shortcut};

use super::state::{FieldEdit, ModelEditorField, ModelEditorMode, ModelEditorState, FIELD_ORDER};

pub fn render_model_editor(buf: &mut Buffer, area: Rect, state: &mut ModelEditorState, compact: bool) {
    let theme = Theme::current();
    let title = state.mode.title();
    let shortcuts = footer_shortcuts(&state.mode);
    let sizing = ModalSizing::medium().with_compact(compact);
    let config = ModalWindowConfig {
        title,
        tabs: None,
        shortcuts: &shortcuts,
        sizing,
        fold_info: None,
    };

    let Some(ModalContentArea { content, .. }) =
        mw::render_modal_window(buf, area, &mut state.window, &config, &theme)
    else {
        return;
    };

    let lines = match &state.mode {
        ModelEditorMode::Browse => browse_lines(state),
        ModelEditorMode::Editing {
            form,
            field_focus,
            field_edit,
            ..
        } => editing_lines(form, *field_focus, field_edit.as_ref(), state.error.as_deref()),
        ModelEditorMode::ConfirmDelete { model_key } => vec![
            Line::from(format!("Remove '{model_key}' from config.toml?")),
            Line::from(""),
            Line::from("y = remove   n / Esc = cancel"),
        ],
        ModelEditorMode::Saving { .. } => vec![Line::from("Saving...")],
        ModelEditorMode::Deleting { .. } => vec![Line::from("Removing...")],
    };

    Paragraph::new(lines).render(content, buf);
}

pub const SHORTCUT_ENTER_ID: usize = 1;
pub const SHORTCUT_DELETE_ID: usize = 2;
pub const SHORTCUT_ESC_ID: usize = 3;
pub const SHORTCUT_SAVE_ID: usize = 4;
pub const SHORTCUT_CONFIRM_YES_ID: usize = 5;
pub const SHORTCUT_CONFIRM_NO_ID: usize = 6;

fn footer_shortcuts(mode: &ModelEditorMode) -> Vec<Shortcut<'static>> {
    match mode {
        ModelEditorMode::Browse => vec![
            Shortcut { label: "\u{2191}/\u{2193} nav", clickable: false, id: 0 },
            Shortcut { label: "Enter open", clickable: true, id: SHORTCUT_ENTER_ID },
            Shortcut { label: "d delete", clickable: true, id: SHORTCUT_DELETE_ID },
            Shortcut { label: "Esc close", clickable: true, id: SHORTCUT_ESC_ID },
        ],
        ModelEditorMode::Editing { field_edit: None, .. } => vec![
            Shortcut { label: "\u{2191}/\u{2193} nav", clickable: false, id: 0 },
            Shortcut { label: "Enter edit field", clickable: true, id: SHORTCUT_ENTER_ID },
            Shortcut { label: "s / Ctrl+S save", clickable: true, id: SHORTCUT_SAVE_ID },
            Shortcut { label: "Esc back", clickable: true, id: SHORTCUT_ESC_ID },
        ],
        ModelEditorMode::Editing { field_edit: Some(FieldEdit::Text { .. }), .. } => vec![
            Shortcut { label: "Enter commit", clickable: true, id: SHORTCUT_ENTER_ID },
            Shortcut { label: "Esc cancel field", clickable: true, id: SHORTCUT_ESC_ID },
        ],
        ModelEditorMode::Editing { field_edit: Some(FieldEdit::Choice { .. }), .. } => vec![
            Shortcut { label: "\u{2191}/\u{2193} choose", clickable: false, id: 0 },
            Shortcut { label: "Enter select", clickable: true, id: SHORTCUT_ENTER_ID },
            Shortcut { label: "Esc cancel", clickable: true, id: SHORTCUT_ESC_ID },
        ],
        ModelEditorMode::ConfirmDelete { .. } => vec![
            Shortcut { label: "y remove", clickable: true, id: SHORTCUT_CONFIRM_YES_ID },
            Shortcut { label: "n / Esc cancel", clickable: true, id: SHORTCUT_CONFIRM_NO_ID },
        ],
        ModelEditorMode::Saving { .. } | ModelEditorMode::Deleting { .. } => vec![],
    }
}

fn browse_lines(state: &ModelEditorState) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(state.entries.len() + 3);
    if let Some(err) = &state.error {
        lines.push(Line::from(Span::styled(
            format!("\u{2717} {err}"),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }
    if state.entries.is_empty() {
        lines.push(Line::from("No models configured yet."));
        lines.push(Line::from(""));
    }
    for (i, (key, over)) in state.entries.iter().enumerate() {
        let display = over.name.clone().unwrap_or_else(|| key.clone());
        let model = over.model.clone().unwrap_or_default();
        let text = format!("{display}  ({key})  \u{2014}  {model}");
        lines.push(row_line(text, i == state.selected));
    }
    lines.push(row_line("+ Add new model...".to_string(), state.selected == state.add_row_index()));
    lines
}

fn row_line(text: String, selected: bool) -> Line<'static> {
    if selected {
        Line::from(Span::styled(
            format!("\u{203a} {text}"),
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ))
    } else {
        Line::from(format!("  {text}"))
    }
}

fn editing_lines(
    form: &super::state::ModelEditorForm,
    field_focus: usize,
    field_edit: Option<&FieldEdit>,
    error: Option<&str>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(FIELD_ORDER.len() + 4);
    if let Some(err) = error {
        lines.push(Line::from(Span::styled(
            format!("\u{2717} {err}"),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }
    for (i, field) in FIELD_ORDER.into_iter().enumerate() {
        // Credential value only matters when a credential mode is set.
        if field == ModelEditorField::CredentialValue
            && form.credential_mode == super::state::CredentialMode::None
        {
            continue;
        }
        let is_focused = i == field_focus;
        let editing_this = is_focused && field_edit.is_some();
        let label = if field == ModelEditorField::CredentialValue {
            form.credential_value_label()
        } else {
            field.label()
        };
        let value = if editing_this {
            match field_edit.unwrap() {
                FieldEdit::Text { buffer, .. } => format!("{buffer}\u{2588}"),
                FieldEdit::Choice { choices, index } => {
                    choices.get(*index).cloned().unwrap_or_default()
                }
            }
        } else {
            form.display_value(field)
        };
        let text = format!("{label}: {value}");
        lines.push(row_line(text, is_focused));
        if is_focused && !editing_this {
            let hint = if field == ModelEditorField::CredentialValue {
                form.credential_value_hint()
            } else {
                field.hint()
            };
            if let Some(hint) = hint {
                lines.push(Line::from(format!("    {hint}")));
            }
        }
        if editing_this {
            if let Some(FieldEdit::Text { error: Some(e), .. }) = field_edit {
                lines.push(Line::from(Span::styled(
                    format!("    \u{2717} {e}"),
                    Style::default().add_modifier(Modifier::BOLD),
                )));
            }
            if let Some(FieldEdit::Choice { choices, index }) = field_edit {
                for (ci, choice) in choices.iter().enumerate() {
                    let marker = if ci == *index { "\u{25cf}" } else { "\u{25cb}" };
                    lines.push(Line::from(format!("      {marker} {choice}")));
                }
            }
        }
    }
    lines
}
