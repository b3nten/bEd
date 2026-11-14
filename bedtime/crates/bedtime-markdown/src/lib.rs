//! Markdown Preview: the document rendered, following edits. Code blocks are
//! coloured by the document's syntax analysis (shared with its editors), so
//! the preview waits briefly for it after each edit instead of flashing
//! uncoloured code. Links open in the browser, relative files in the app,
//! and `#anchors` scroll to their heading. Images beside the file show in
//! place, at their own size or the text's width.
//!
//! It isn't the default for Markdown files (the editor is): it's in "Open
//! With", and an editor's `other_viewer`.
//!
//! Parsing and drawing are `bedtime_ui::markdown` (`MarkdownView`); this
//! crate shows a document's text with it and follows its links.
use bedtime_code::{
    EditorPanel,
    syntax::{AnalysisStatus, DocumentSyntax, SyntaxViewer},
};
use bedtime_document::TextBuffer;
use bedtime_ui::{
    LinksWorldExt, UiContext, egui,
    markdown::{MarkdownView, percent_decode},
    theme::SyntaxRole,
};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{
    DocumentReference, Format, FormatId, FormatsAppExt, Notification, Panel, PathOptions, TEXT,
    ViewerFile, ViewersAppExt, WorkbenchWorldExt, viewer_document,
};
use bevy::prelude::*;
use std::{path::PathBuf, time::Duration};

pub use bedtime_ui::markdown::{Block, Callout, Code, Item, Markdown, Run, Style, parse, slug};

/// Markdown files. bedtime-code knows them as a language too (formats are
/// shared by id).
pub const MARKDOWN: FormatId = FormatId("markdown");

pub struct MarkdownPlugin;

impl Plugin for MarkdownPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<MarkdownPanel>()
            .register_format(
                Format::new(MARKDOWN)
                    .extensions(["md", "markdown", "mdown", "mkd", "mkdn"])
                    .conforms_to([TEXT]),
            )
            .register_viewer::<TextBuffer, MarkdownPanel>("Markdown Preview")
            .formats([MARKDOWN])
            // The editor stays the default; an app can say otherwise.
            .set_default_viewer::<EditorPanel>(MARKDOWN);
    }
}

/// How long a new revision waits for its code's colours.
const WAIT_FOR_COLOURS: f64 = 0.3;

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
#[require(SyntaxViewer)]
pub struct MarkdownPanel {
    #[reflect(ignore)]
    state: State,
}

#[derive(Default)]
struct State {
    shown: Option<Shown>,
    /// When the document changed past `shown`.
    waiting_since: Option<f64>,
    /// A heading to scroll to, by anchor.
    scroll_to: Option<String>,
}

struct Shown {
    revision: u64,
    view: MarkdownView,
    coloured: bool,
}

impl Panel for MarkdownPanel {
    const LABEL: &'static str = "Markdown Preview";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = viewer_document::<TextBuffer>(ui, world, panel) else {
            return;
        };
        let Some(entity) = world
            .get::<DocumentReference>(panel)
            .map(|r| r.document().entity())
        else {
            return;
        };
        let now = ui.input(|i| i.time);
        let syntax = world.get::<DocumentSyntax>(entity);
        // Colours for `revision` are on their way.
        let colouring = |revision: u64| {
            syntax.is_some_and(|syntax| {
                syntax.at(revision).is_none() && *syntax.session.status() == AnalysisStatus::Pending
            })
        };
        let pending = colouring(document.revision);
        let observation = syntax
            .and_then(|syntax| syntax.at(document.revision))
            .cloned();
        let base = world
            .get::<ViewerFile>(panel)
            .and_then(|file| file.0.parent().map(PathBuf::from));
        let Some(mut preview) = world.get_mut::<MarkdownPanel>(panel) else {
            return;
        };
        let state = &mut preview.state;
        let stale = state
            .shown
            .as_ref()
            .is_none_or(|shown| shown.revision != document.revision);
        if stale {
            let since = *state.waiting_since.get_or_insert(now);
            if state.shown.is_none() || !pending || now - since >= WAIT_FOR_COLOURS {
                let text = document.model.range_text(0..document.model.len());
                let mut view = MarkdownView::new(&text);
                view.images = true;
                state.shown = Some(Shown {
                    revision: document.revision,
                    view,
                    coloured: false,
                });
                state.waiting_since = None;
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
        }
        if let Some(shown) = &mut state.shown
            && !shown.coloured
        {
            if let Some(observation) = observation.filter(|_| shown.revision == document.revision) {
                let spans: Vec<_> = observation
                    .highlights
                    .iter()
                    .filter_map(|h| Some((h.range.clone(), SyntaxRole::of_capture(&h.capture)?)))
                    .collect();
                shown.view.colour(&spans);
                shown.coloured = true;
            } else if pending {
                ui.ctx().request_repaint_after(Duration::from_millis(16));
            }
        }
        let mut clicked = None;
        if let Some(shown) = &mut state.shown {
            if let Some(anchor) = state.scroll_to.take() {
                shown.view.scroll_to = Some(anchor);
            }
            shown.view.base = base;
            clicked = shown.view.show(ui, context);
        }
        if let Some(link) = clicked {
            follow(world, panel, &link);
        }
    }
}

/// Open a clicked link: an anchor here, a web page outside, a file in the app.
fn follow(world: &mut World, panel: Entity, link: &str) {
    if let Some(anchor) = link.strip_prefix('#') {
        if let Some(mut preview) = world.get_mut::<MarkdownPanel>(panel) {
            preview.state.scroll_to = Some(anchor.to_lowercase());
        }
        return;
    }
    let external = link.contains("://") || link.starts_with("mailto:");
    let result = if external {
        world.open_externally_from(link, panel)
    } else {
        let file = link.split('#').next().unwrap_or(link);
        let file = percent_decode(file);
        match world.get::<ViewerFile>(panel) {
            Some(viewer) => {
                let base = viewer.0.parent().unwrap_or(std::path::Path::new(""));
                let path = base.join(&file);
                world
                    .workbench_mut()
                    .open_path(
                        &path,
                        PathOptions {
                            place: Some(bedtime_workbench::Placement::From(panel)),
                            ..default()
                        },
                    )
                    .map(|_| ())
                    .map_err(|error| format!("{}: {error}", path.display()))
            }
            None => Err(format!("{file} is relative to a file this isn't")),
        }
    };
    if let Err(error) = result {
        world.workbench_mut().notify(Notification::warning(error));
    }
}
