//! Key handling for the in-TUI `[model.*]` add/edit/remove screen.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::app::actions::Action;

use super::state::{
    CredentialMode, FieldEdit, ModelEditorField, ModelEditorForm, ModelEditorMode,
    ModelEditorState, FIELD_ORDER,
};

#[derive(Debug)]
pub enum ModelEditorKeyOutcome {
    Close,
    Action(Action),
    Changed,
    Unchanged,
}

/// Inserts a bracketed-paste directly into the focused text field, so an
/// embedded/trailing newline in the clipboard can't hit the Enter-commits
/// path and truncate the value.
pub fn handle_model_editor_paste(state: &mut ModelEditorState, text: &str) -> bool {
    let ModelEditorMode::Editing {
        field_edit: Some(FieldEdit::Text { buffer, cursor_byte, .. }),
        ..
    } = &mut state.mode
    else {
        return false;
    };
    let cleaned: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    if cleaned.is_empty() {
        return false;
    }
    buffer.insert_str(*cursor_byte, &cleaned);
    *cursor_byte += cleaned.len();
    true
}

pub fn handle_model_editor_key(
    state: &mut ModelEditorState,
    key: &KeyEvent,
) -> ModelEditorKeyOutcome {
    if key.kind == KeyEventKind::Release {
        return ModelEditorKeyOutcome::Unchanged;
    }

    // Take ownership of the mode so we can freely rebuild it without
    // fighting the borrow checker over `state.mode`.
    let mode = std::mem::replace(&mut state.mode, ModelEditorMode::Browse);
    match mode {
        ModelEditorMode::Browse => handle_browse(state, key),
        ModelEditorMode::Editing {
            original_key,
            form,
            field_focus,
            field_edit: None,
        } => handle_editing_row_focus(state, key, original_key, form, field_focus),
        ModelEditorMode::Editing {
            original_key,
            form,
            field_focus,
            field_edit: Some(edit),
        } => handle_editing_field(state, key, original_key, form, field_focus, edit),
        ModelEditorMode::ConfirmDelete { model_key } => handle_confirm_delete(state, key, model_key),
        ModelEditorMode::Saving { model_key } => {
            // Write in flight: ignore input, restore state unchanged.
            state.mode = ModelEditorMode::Saving { model_key };
            ModelEditorKeyOutcome::Unchanged
        }
        ModelEditorMode::Deleting { model_key } => {
            state.mode = ModelEditorMode::Deleting { model_key };
            ModelEditorKeyOutcome::Unchanged
        }
    }
}

/// Rows visible in Browse: real entries in their configured order, then the
/// trailing "+ Add new model..." synthetic row.
fn visible_fields(form: &ModelEditorForm) -> Vec<ModelEditorField> {
    FIELD_ORDER
        .into_iter()
        .filter(|f| {
            *f != ModelEditorField::CredentialValue || form.credential_mode != CredentialMode::None
        })
        .collect()
}

fn handle_browse(state: &mut ModelEditorState, key: &KeyEvent) -> ModelEditorKeyOutcome {
    let last = state.add_row_index();
    match key.code {
        KeyCode::Esc => ModelEditorKeyOutcome::Close,
        KeyCode::Up | KeyCode::Char('k') => {
            state.selected = state.selected.saturating_sub(1);
            state.mode = ModelEditorMode::Browse;
            ModelEditorKeyOutcome::Changed
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.selected = (state.selected + 1).min(last);
            state.mode = ModelEditorMode::Browse;
            ModelEditorKeyOutcome::Changed
        }
        KeyCode::Enter => {
            if state.selected == last {
                state.start_add();
            } else if let Some(key) = state.entries.get_index(state.selected).map(|(k, _)| k.clone())
            {
                state.start_edit(&key);
            } else {
                state.mode = ModelEditorMode::Browse;
            }
            ModelEditorKeyOutcome::Changed
        }
        KeyCode::Char('d') if state.selected < last => {
            if let Some((model_key, _)) = state.entries.get_index(state.selected) {
                state.mode = ModelEditorMode::ConfirmDelete {
                    model_key: model_key.clone(),
                };
            } else {
                state.mode = ModelEditorMode::Browse;
            }
            ModelEditorKeyOutcome::Changed
        }
        _ => {
            state.mode = ModelEditorMode::Browse;
            ModelEditorKeyOutcome::Unchanged
        }
    }
}

fn handle_confirm_delete(
    state: &mut ModelEditorState,
    key: &KeyEvent,
    model_key: String,
) -> ModelEditorKeyOutcome {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            state.mode = ModelEditorMode::Deleting {
                model_key: model_key.clone(),
            };
            ModelEditorKeyOutcome::Action(Action::DeleteModelOverride { model_key })
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            state.mode = ModelEditorMode::Browse;
            ModelEditorKeyOutcome::Changed
        }
        _ => {
            state.mode = ModelEditorMode::ConfirmDelete { model_key };
            ModelEditorKeyOutcome::Unchanged
        }
    }
}

fn handle_editing_row_focus(
    state: &mut ModelEditorState,
    key: &KeyEvent,
    original_key: Option<String>,
    form: ModelEditorForm,
    field_focus: usize,
) -> ModelEditorKeyOutcome {
    let visible = visible_fields(&form);
    let max_idx = visible.len().saturating_sub(1);
    // `field_focus` indexes FIELD_ORDER directly; clamp against the visible
    // subset's length so a skipped CredentialValue row can't be focused.
    let focus = field_focus.min(max_idx);

    match key.code {
        KeyCode::Esc => {
            state.mode = ModelEditorMode::Browse;
            ModelEditorKeyOutcome::Changed
        }
        // Bare `s` as a fallback: Ctrl+S/Cmd+S are unreliable across terminals.
        KeyCode::Char('s')
            if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.is_empty() =>
        {
            let existing_others = &state.entries;
            match form.validate_all(original_key.as_deref(), existing_others) {
                Some(err) => {
                    state.error = Some(err);
                    state.mode = ModelEditorMode::Editing {
                        original_key,
                        form,
                        field_focus: focus,
                        field_edit: None,
                    };
                    ModelEditorKeyOutcome::Changed
                }
                None => {
                    state.error = None;
                    let model_key = form.id.trim().to_string();
                    let fields = form.to_write_fields();
                    state.mode = ModelEditorMode::Saving {
                        model_key: model_key.clone(),
                    };
                    ModelEditorKeyOutcome::Action(Action::SaveModelOverride {
                        original_key,
                        model_key,
                        fields,
                    })
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let new_focus = focus.saturating_sub(1);
            state.mode = ModelEditorMode::Editing {
                original_key,
                form,
                field_focus: new_focus,
                field_edit: None,
            };
            ModelEditorKeyOutcome::Changed
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let new_focus = (focus + 1).min(max_idx);
            state.mode = ModelEditorMode::Editing {
                original_key,
                form,
                field_focus: new_focus,
                field_edit: None,
            };
            ModelEditorKeyOutcome::Changed
        }
        KeyCode::Enter => {
            let field = visible[focus];
            let edit = start_field_edit(field, &form);
            state.mode = ModelEditorMode::Editing {
                original_key,
                form,
                field_focus: focus,
                field_edit: Some(edit),
            };
            ModelEditorKeyOutcome::Changed
        }
        _ => {
            state.mode = ModelEditorMode::Editing {
                original_key,
                form,
                field_focus: focus,
                field_edit: None,
            };
            ModelEditorKeyOutcome::Unchanged
        }
    }
}

fn start_field_edit(field: ModelEditorField, form: &ModelEditorForm) -> FieldEdit {
    match field {
        ModelEditorField::ApiBackend => {
            // Labels spell out which providers each wire protocol matches —
            // the user picks a provider, not a protocol name they'd have to
            // already know (e.g. Anthropic needs "messages", not the more
            // commonly-assumed "chat_completions").
            let choices = vec![
                "chat_completions — OpenAI-compatible (OpenAI, Ollama, vLLM, LiteLLM, most others)"
                    .to_string(),
                "responses — OpenAI Responses API".to_string(),
                "messages — Anthropic (Claude)".to_string(),
            ];
            let index = match form.api_backend {
                ainxt_shell::sampling::ApiBackend::ChatCompletions => 0,
                ainxt_shell::sampling::ApiBackend::Responses => 1,
                ainxt_shell::sampling::ApiBackend::Messages => 2,
            };
            FieldEdit::Choice { choices, index }
        }
        ModelEditorField::CredentialMode => {
            let choices: Vec<String> = CredentialMode::CHOICES
                .iter()
                .map(|c| c.label().to_string())
                .collect();
            let index = CredentialMode::CHOICES
                .iter()
                .position(|c| *c == form.credential_mode)
                .unwrap_or(0);
            FieldEdit::Choice { choices, index }
        }
        _ => {
            let buffer = form.text_buffer_for(field);
            let cursor_byte = buffer.len();
            FieldEdit::Text {
                buffer,
                cursor_byte,
                error: None,
            }
        }
    }
}

fn commit_choice(field: ModelEditorField, form: &mut ModelEditorForm, index: usize) {
    match field {
        ModelEditorField::ApiBackend => {
            form.api_backend = match index {
                0 => ainxt_shell::sampling::ApiBackend::ChatCompletions,
                1 => ainxt_shell::sampling::ApiBackend::Responses,
                _ => ainxt_shell::sampling::ApiBackend::Messages,
            };
        }
        ModelEditorField::CredentialMode => {
            form.credential_mode = *CredentialMode::CHOICES
                .get(index)
                .unwrap_or(&CredentialMode::EnvKey);
        }
        _ => {}
    }
}

fn handle_editing_field(
    state: &mut ModelEditorState,
    key: &KeyEvent,
    original_key: Option<String>,
    mut form: ModelEditorForm,
    field_focus: usize,
    edit: FieldEdit,
) -> ModelEditorKeyOutcome {
    let visible = visible_fields(&form);
    let field = *visible.get(field_focus.min(visible.len().saturating_sub(1))).unwrap_or(&ModelEditorField::Id);

    match edit {
        FieldEdit::Text { mut buffer, mut cursor_byte, error: _ } => match key.code {
            KeyCode::Esc => {
                state.mode = ModelEditorMode::Editing {
                    original_key,
                    form,
                    field_focus,
                    field_edit: None,
                };
                ModelEditorKeyOutcome::Changed
            }
            KeyCode::Enter => {
                form.apply_text(field, buffer.clone());
                let err = form.validate_field(field, original_key.as_deref(), &state.entries);
                if let Some(err) = err {
                    state.mode = ModelEditorMode::Editing {
                        original_key,
                        form,
                        field_focus,
                        field_edit: Some(FieldEdit::Text {
                            buffer,
                            cursor_byte,
                            error: Some(err),
                        }),
                    };
                } else {
                    state.mode = ModelEditorMode::Editing {
                        original_key,
                        form,
                        field_focus,
                        field_edit: None,
                    };
                }
                ModelEditorKeyOutcome::Changed
            }
            KeyCode::Backspace => {
                if cursor_byte > 0 {
                    let prev = buffer[..cursor_byte]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    buffer.drain(prev..cursor_byte);
                    cursor_byte = prev;
                }
                state.mode = ModelEditorMode::Editing {
                    original_key,
                    form,
                    field_focus,
                    field_edit: Some(FieldEdit::Text {
                        buffer,
                        cursor_byte,
                        error: None,
                    }),
                };
                ModelEditorKeyOutcome::Changed
            }
            KeyCode::Left => {
                if cursor_byte > 0 {
                    cursor_byte = buffer[..cursor_byte]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
                state.mode = ModelEditorMode::Editing {
                    original_key,
                    form,
                    field_focus,
                    field_edit: Some(FieldEdit::Text { buffer, cursor_byte, error: None }),
                };
                ModelEditorKeyOutcome::Changed
            }
            KeyCode::Right => {
                if cursor_byte < buffer.len() {
                    cursor_byte += buffer[cursor_byte..]
                        .chars()
                        .next()
                        .map(char::len_utf8)
                        .unwrap_or(0);
                }
                state.mode = ModelEditorMode::Editing {
                    original_key,
                    form,
                    field_focus,
                    field_edit: Some(FieldEdit::Text { buffer, cursor_byte, error: None }),
                };
                ModelEditorKeyOutcome::Changed
            }
            KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                buffer.insert(cursor_byte, c);
                cursor_byte += c.len_utf8();
                state.mode = ModelEditorMode::Editing {
                    original_key,
                    form,
                    field_focus,
                    field_edit: Some(FieldEdit::Text { buffer, cursor_byte, error: None }),
                };
                ModelEditorKeyOutcome::Changed
            }
            _ => {
                state.mode = ModelEditorMode::Editing {
                    original_key,
                    form,
                    field_focus,
                    field_edit: Some(FieldEdit::Text { buffer, cursor_byte, error: None }),
                };
                ModelEditorKeyOutcome::Unchanged
            }
        },
        FieldEdit::Choice { choices, mut index } => match key.code {
            KeyCode::Esc => {
                state.mode = ModelEditorMode::Editing {
                    original_key,
                    form,
                    field_focus,
                    field_edit: None,
                };
                ModelEditorKeyOutcome::Changed
            }
            KeyCode::Up | KeyCode::Char('k') => {
                index = index.saturating_sub(1);
                state.mode = ModelEditorMode::Editing {
                    original_key,
                    form,
                    field_focus,
                    field_edit: Some(FieldEdit::Choice { choices, index }),
                };
                ModelEditorKeyOutcome::Changed
            }
            KeyCode::Down | KeyCode::Char('j') => {
                index = (index + 1).min(choices.len().saturating_sub(1));
                state.mode = ModelEditorMode::Editing {
                    original_key,
                    form,
                    field_focus,
                    field_edit: Some(FieldEdit::Choice { choices, index }),
                };
                ModelEditorKeyOutcome::Changed
            }
            KeyCode::Enter => {
                commit_choice(field, &mut form, index);
                state.mode = ModelEditorMode::Editing {
                    original_key,
                    form,
                    field_focus,
                    field_edit: None,
                };
                ModelEditorKeyOutcome::Changed
            }
            _ => {
                state.mode = ModelEditorMode::Editing {
                    original_key,
                    form,
                    field_focus,
                    field_edit: Some(FieldEdit::Choice { choices, index }),
                };
                ModelEditorKeyOutcome::Unchanged
            }
        },
    }
}
