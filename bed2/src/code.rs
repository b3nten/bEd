use bedtime_editor2::workbench::EditorAnalysisPlugin;
#[cfg(feature = "automation")]
use bedtime_editor2::workbench::WorkbenchEditor;
#[cfg(feature = "automation")]
use bedtime_workbench2::*;
use bevy::prelude::*;

pub struct CodePlugin;
impl Plugin for CodePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            EditorAnalysisPlugin,
            bed_structure2::StructurePlugin,
            crate::lsp::LanguageToolsPlugin,
        ));
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
