//! Dispatch handlers for the in-TUI `[model.*]` add/edit/remove screen.
//!
//! `state.mode` for the Saving/Deleting transition is set synchronously by
//! `views::model_editor::input` when the key that triggers the action is
//! handled (mirroring how `settings_modal` commits before the effect is
//! even built) — these handlers only need to resolve the current agent and
//! build the `Effect`.

use ainxt_shell::agent::config_model_override_write::ModelOverrideFormFields;

use crate::app::actions::Effect;
use crate::app::app_view::{ActiveView, AppView};

pub(super) fn dispatch_open_model_editor(app: &mut AppView) -> Vec<Effect> {
    use crate::views::modal::ActiveModal;
    use crate::views::model_editor::ModelEditorState;

    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let entries =
        match ainxt_shell::agent::config_model_override_write::model_overrides_from_effective_config()
        {
            Ok(entries) => entries,
            Err(e) => {
                app.show_toast(&format!("\u{2717} Could not read config.toml: {e}"));
                Default::default()
            }
        };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    agent.active_modal = Some(ActiveModal::ModelEditor {
        state: Box::new(ModelEditorState::new(entries)),
    });
    vec![]
}

pub(super) fn dispatch_save_model_override(
    app: &mut AppView,
    original_key: Option<String>,
    model_key: String,
    fields: ModelOverrideFormFields,
) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    vec![Effect::SaveModelOverride {
        agent_id: id,
        original_key,
        model_key,
        fields,
    }]
}

pub(super) fn dispatch_delete_model_override(app: &mut AppView, model_key: String) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    vec![Effect::DeleteModelOverride {
        agent_id: id,
        model_key,
    }]
}
