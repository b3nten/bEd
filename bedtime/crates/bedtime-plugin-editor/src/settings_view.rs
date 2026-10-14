use dear_imgui_rs::Ui;
use serde_json::{Value, json};

pub(crate) fn draw(ui: &Ui, settings: &mut Value) -> bool {
    let mut changed = false;
    for (key, label) in [
        ("word_wrap", "Wrap long lines"),
        ("minimap", "Show minimap"),
        ("rainbow", "Rainbow text"),
        ("cursor_smear", "Animate cursor movement"),
    ] {
        let mut value = settings[key]
            .as_bool()
            .expect("published editor preference");
        if bedtime_ui::settings::checkbox(ui, label, &mut value) {
            settings[key] = json!(value);
            changed = true;
        }
    }
    #[cfg(feature = "language-services")]
    {
        ui.spacing();
        ui.separator_with_text("Language services");
        for (key, label) in [
            ("lsp_autocomplete", "Language server autocomplete"),
            ("lsp_format_on_save", "Format on save"),
            ("git_changed_lines", "Show changed lines"),
            ("treesitter", "Syntax highlighting"),
        ] {
            let mut value = settings[key]
                .as_bool()
                .expect("published editor preference");
            if bedtime_ui::settings::checkbox(ui, label, &mut value) {
                settings[key] = json!(value);
                changed = true;
            }
        }
        let mut delay = settings["lsp_completion_delay_ms"]
            .as_f64()
            .expect("published completion delay");
        if bedtime_ui::settings::number(ui, "Completion delay (ms)", &mut delay) {
            settings["lsp_completion_delay_ms"] = json!(delay);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    #[test]
    fn code_editor_publishes_one_named_settings_renderer() {
        let mut app = bevy::prelude::App::new();
        app.add_plugins(crate::EditorPlugin);
        let registry = app
            .world()
            .resource::<bedtime_api::features::FeatureRegistry>();
        let sections = registry
            .settings()
            .iter()
            .filter(|section| section.plugin == crate::MODULE_ID)
            .collect::<Vec<_>>();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].label, "Code Editor");
        assert!(sections[0].draw.is_some());
        assert!(
            sections[0].draw_page.is_none(),
            "the dashboard is a command, not another settings page"
        );
    }
}
