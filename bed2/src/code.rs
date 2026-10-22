use bedtime_editor2::workbench::EditorAnalysisPlugin;
#[cfg(feature = "automation")]
use bedtime_editor2::workbench::WorkbenchEditor;
#[cfg(feature = "automation")]
use bedtime_workbench2::*;
use bevy::prelude::*;

pub struct CodePlugin;
impl Plugin for CodePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((EditorAnalysisPlugin, bed_structure2::StructurePlugin));
        #[cfg(feature = "automation")]
        {
            use bedtime_automation2::AutomationAppExt;
            app.register_automation_command("app.inspect_code", inspect);
        }
    }
}

#[cfg(feature = "automation")]
fn inspect(world: &mut World, _: serde_json::Value) -> Result<serde_json::Value, String> {
    use bedtime_editor2::workbench::analysis_for_panel;
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<crate::files::FilePreviewPanel>>()
        .iter(world)
        .collect();
    let panels:Vec<_>=panels.into_iter().map(|panel| {
        let source=analysis_for_panel(world,panel);
        let view=world.get::<WorkbenchEditor>(panel);
        serde_json::json!({ "panel":panel.to_bits().to_string(),"document":world.get::<DocumentView>(panel).map(|v|v.document().0.to_bits().to_string()),
            "revision":source.as_ref().map(|o|o.key.revision),"language":source.as_ref().map(|o|o.language),
            "highlights":source.as_ref().map(|o|o.highlights.len()),"folds":source.as_ref().map(|o|o.folds.len()),
            "outline":source.as_ref().map(|o|o.outline.iter().map(|n|serde_json::json!({"id":n.id,"label":n.label,"depth":n.depth})).collect::<Vec<_>>()),
            "selection":view.map(|v|vec![v.view.selection().anchor(),v.view.selection().head()]),"scroll":view.map(|v|v.view.scroll_offset().y),"collapsed":view.map(|v|v.view.collapsed_regions().len()) })
    }).collect();
    let sources:Vec<_>=world.query::<(Entity,&bed_structure2::StructureSource)>().iter(world).map(|(workspace,s)|serde_json::json!({"workspace":workspace.to_bits().to_string(),"source":s.source.map(|source|source.panel.to_bits().to_string())})).collect();
    Ok(serde_json::json!({"panels":panels,"sources":sources}))
}

/// Optional editor actions live in the canvas context menu, leaving its top edge clear.
pub fn context_menu(ui: &mut bedtime_ui2::egui::Ui, world: &mut World, panel: Entity) {
    use bedtime_editor2::{
        TextBuffer, TextSnapshot,
        workbench::{CodeLanguage, WorkbenchEditor},
    };
    use bedtime_workbench2::*;
    let document = world.get::<DocumentView>(panel).unwrap().document();
    let mut folding = 0;
    if let Some(mut editor) = world.get_mut::<WorkbenchEditor>(panel) {
        editor.view.editing_menu(ui);
        ui.checkbox(&mut editor.view.settings.line_numbers, "Line numbers");
        ui.menu_button("Folding", |ui| {
            if ui.button("Fold all").clicked() {
                folding = 1;
                ui.close();
            }
            if ui.button("Unfold all").clicked() {
                folding = 2;
                ui.close();
            }
        });
    }
    let mut language = world.get::<CodeLanguage>(document.0).unwrap().0.clone();
    bedtime_ui2::egui::ComboBox::from_id_salt(("code-language", document))
        .selected_text(&language)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut language, "text".into(), "Plain text");
            for &name in bedtime_treesitter2::LANGUAGES {
                ui.selectable_value(&mut language, name.to_owned(), name);
            }
        });
    if world.get::<CodeLanguage>(document.0).unwrap().0 != language {
        world.entity_mut(document.0).insert(CodeLanguage(language));
    }
    if folding != 0 {
        let handle = cast_document::<TextBuffer>(world, document).unwrap();
        let snapshot = TextSnapshot {
            text: read_document(world, handle).unwrap().clone(),
            revision: inspect_document(world, document).unwrap().revision,
        };
        let mut editor = world.get_mut::<WorkbenchEditor>(panel).unwrap();
        if folding == 1 {
            let _ = editor.view.fold_all(&snapshot);
        } else {
            editor.view.unfold_all();
        }
        ui.close();
    }
}
