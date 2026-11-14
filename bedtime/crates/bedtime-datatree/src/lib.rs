//! Tree View: a JSON or TOML document as rows that open and close. Arrays
//! and tables show their size; scalars their value, coloured by kind. It
//! parses each revision on a worker and keeps showing the last one that
//! parsed, with the error, while the text doesn't.
//!
//! It isn't the default for these files (the editor is): it's in "Open
//! With", and an editor's `other_viewer`.
use bedtime_code::EditorPanel;
use bedtime_document::TextBuffer;
use bedtime_ui::{UiContext, data::DataTree, egui, theme};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    DocumentReference, FormatId, FormatsAppExt, ManagedTask, ManagedWork, Panel, TEXT,
    ViewersAppExt, WorkbenchWorldExt, viewer_document,
};
use bevy::prelude::*;
use std::{collections::HashSet, sync::Arc, time::Duration};

pub use bedtime_ui::data::{Format, Key, Kind, Value, parse, path_text};

/// The formats it shows. bedtime-code knows them as languages too (formats
/// are shared by id).
pub const JSON: FormatId = FormatId("json");
pub const TOML: FormatId = FormatId("toml");

pub struct DataTreePlugin;

impl Plugin for DataTreePlugin {
    fn build(&self, app: &mut App) {
        use bedtime_workbench::Format as FileFormat;
        app.register_type::<TreePanel>()
            .register_format(
                FileFormat::new(JSON)
                    .extensions(["json", "geojson", "webmanifest"])
                    .conforms_to([TEXT]),
            )
            .register_format(
                FileFormat::new(TOML)
                    .extensions(["toml"])
                    .names(["cargo.lock", "uv.lock", "poetry.lock"])
                    .conforms_to([TEXT]),
            )
            .register_viewer::<TextBuffer, TreePanel>("Tree View")
            .formats([JSON, TOML]);
        // The editor stays these files' default; an app can say otherwise.
        for format in [JSON, TOML] {
            app.set_default_viewer::<EditorPanel>(format);
        }
    }
}

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
pub struct TreePanel {
    /// Open arrays and tables, by `path_text`.
    expanded: HashSet<String>,
    #[reflect(ignore)]
    state: State,
}

#[derive(Default)]
struct State {
    /// The last revision that parsed, drawn.
    shown: Option<(u64, DataTree)>,
    /// The last revision that didn't parse, and why.
    error: Option<(u64, String)>,
    parsing: Option<(u64, ManagedTask<Result<Value, String>>)>,
}

impl Panel for TreePanel {
    const LABEL: &'static str = "Tree View";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = viewer_document::<TextBuffer>(ui, world, panel) else {
            return;
        };
        let title = world
            .get::<DocumentReference>(panel)
            .and_then(|reference| {
                world
                    .workbench()
                    .inspect_document(reference.document())
                    .ok()
            })
            .map(|info| info.title)
            .unwrap_or_default();
        let Some(format) = Format::of(&title) else {
            ui.label(format!("{title} isn't JSON or TOML"));
            return;
        };
        update(world, panel, format, &document.model, document.revision);
        if world
            .get::<TreePanel>(panel)
            .is_some_and(|tree| tree.state.parsing.is_some())
        {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        let Some(mut tree) = world.get_mut::<TreePanel>(panel) else {
            return;
        };
        ui.push_id(panel, |ui| draw(ui, context, &mut tree));
    }
}

/// Collect a finished parse, and start one for a newer revision.
fn update(world: &mut World, panel: Entity, format: Format, text: &TextBuffer, revision: u64) {
    let Some(mut tree) = world.get_mut::<TreePanel>(panel) else {
        return;
    };
    let tree = &mut *tree;
    if let Some((parsed, task)) = &tree.state.parsing {
        match task.try_recv() {
            Ok(Ok(value)) => {
                let value = Arc::new(value);
                match &mut tree.state.shown {
                    Some((shown, view)) => {
                        *shown = *parsed;
                        view.set_value(value);
                    }
                    // The first: open what was saved, else the first level.
                    None if tree.expanded.is_empty() => {
                        let view = DataTree::new(value);
                        tree.expanded = view.expanded.clone();
                        tree.state.shown = Some((*parsed, view));
                    }
                    None => {
                        let view = DataTree::with_expanded(value, tree.expanded.clone());
                        tree.state.shown = Some((*parsed, view));
                    }
                }
                tree.state.error = None;
                tree.state.parsing = None;
            }
            Ok(Err(error)) => {
                tree.state.error = Some((*parsed, error));
                tree.state.parsing = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => tree.state.parsing = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
        }
    }
    let done = tree.state.shown.as_ref().map(|(revision, _)| *revision);
    let failed = tree.state.error.as_ref().map(|(revision, _)| *revision);
    if done == Some(revision) || failed == Some(revision) {
        return;
    }
    let text = text.clone();
    let task = world.resource_mut::<ManagedWork>().spawn_task(
        Some(panel),
        "Reading the tree",
        move |_| parse(format, &text.range_text(0..text.len())),
    );
    if let (Ok(task), Some(mut tree)) = (task, world.get_mut::<TreePanel>(panel)) {
        tree.state.parsing = Some((revision, task));
    }
}

fn draw(ui: &mut egui::Ui, context: &UiContext<'_>, tree: &mut TreePanel) {
    let p = &context.theme.palette;
    let Some((_, view)) = tree.state.shown.as_mut() else {
        match &tree.state.error {
            Some((_, error)) => {
                egui::Frame::NONE.inner_margin(14).show(ui, |ui| {
                    ui.label(format!("This doesn't parse: {error}"));
                });
            }
            None => {
                ui.centered_and_justified(|ui| ui.spinner());
            }
        }
        return;
    };
    // The text doesn't parse now: say why, above the last tree that did.
    if let Some((_, error)) = &tree.state.error {
        egui::Frame::NONE
            .fill(theme::color(theme::mix(
                p.warning,
                p.panel_background,
                0.82,
            )))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(format!("Showing the last version that parsed. {error}"))
                        .color(theme::color(p.text)),
                );
            });
    }
    if view.show(ui, context) {
        tree.expanded = view.expanded.clone();
    }
}
