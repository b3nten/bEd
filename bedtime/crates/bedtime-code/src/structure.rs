//! Structure: the outline of the focused editor's text, or of the last editor
//! focused while another kind of panel (Structure itself, say) has focus, or
//! of the editor it's pinned to. It follows the editor's caret; clicking an
//! entry selects its name in that editor and focuses it.
//!
//! The same outline gives the editor its breadcrumbs (`breadcrumbs`) and Go
//! to Symbol in File (`picker`).
use crate::{
    editor::{EditorCommand, EditorPanel, Navigation, Scroll, TextSelection, TextSnapshot},
    syntax::{AnalysisStatus, DocumentSyntax, Observation, ObservationKey},
};
use bedtime_document::TextBuffer;
use bedtime_ui::{UiContext, egui};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{DocumentReference, LayoutsWorldExt, Panel, WorkbenchWorldExt};
use bevy::prelude::*;
use std::{collections::HashSet, sync::Arc};

pub(crate) mod breadcrumbs;
pub(crate) mod picker;
pub mod symbols;
mod view;

pub use picker::GoToSymbol;

/// The editor panel Structure shows (unless it's pinned to another).
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct StructureSource(pub Option<Entity>);

/// Follow the focused editor, in `Update`.
pub(crate) fn update_source(world: &mut World) {
    let focused = world
        .layouts()
        .active()
        .and_then(|layout| world.layouts().focused_panel(layout))
        .filter(|&panel| has_outline(world, panel));
    let remembered = world
        .resource::<StructureSource>()
        .0
        .filter(|&panel| has_outline(world, panel));
    let source = StructureSource(focused.or(remembered));
    if *world.resource::<StructureSource>() != source {
        world.insert_resource(source);
    }
}

/// An editor showing a document with a language.
pub(crate) fn has_outline(world: &World, panel: Entity) -> bool {
    world.get::<EditorPanel>(panel).is_some()
        && world
            .get::<DocumentReference>(panel)
            .and_then(|reference| {
                world
                    .workbench()
                    .inspect_document(reference.document())
                    .ok()
            })
            .is_some_and(|info| {
                crate::syntax::document_language(world, info.id, &info.title).is_some()
            })
}

/// The current analysis of the text `panel` shows: at its current revision.
pub fn current_outline(world: &World, panel: Entity) -> Option<Arc<Observation>> {
    world.get::<EditorPanel>(panel)?;
    let document = world.get::<DocumentReference>(panel)?.document();
    let revision = world.workbench().inspect_document(document).ok()?.revision;
    world
        .get::<DocumentSyntax>(document.entity())?
        .at(revision)
        .cloned()
}

/// The entry of `observation` around `editor`'s primary caret (its index),
/// if `observation` is of the editor's text as it is now.
pub fn caret_entry(world: &World, editor: Entity, observation: &Observation) -> Option<usize> {
    let current = current_outline(world, editor)?;
    if current.key != observation.key {
        return None;
    }
    let head = world.get::<EditorPanel>(editor)?.view.selection().head();
    symbols::entry_at(&observation.outline, head)
}

/// Where in `editor` an entry is shown, and what's done there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reveal {
    /// Select its name and focus the editor.
    Focus,
    /// Select its name, leaving focus where it is (browsing the outline).
    Preview,
    /// Select all of it and focus the editor.
    SelectAll,
    /// Fold it in the editor.
    Fold,
}

/// Select outline entry `item` of analysis `key` in `editor`, and focus it.
/// An entry from an older analysis, or an editor that's gone or composing
/// text, does nothing (returns false).
pub fn reveal_entry(world: &mut World, editor: Entity, key: ObservationKey, item: u64) -> bool {
    reveal_entry_as(world, editor, key, item, Reveal::Focus)
}

/// `reveal_entry`, doing `how`.
pub fn reveal_entry_as(
    world: &mut World,
    editor: Entity,
    key: ObservationKey,
    item: u64,
    how: Reveal,
) -> bool {
    let Some(observation) = current_outline(world, editor).filter(|o| o.key == key) else {
        return false;
    };
    let Some(node) = observation.outline.iter().find(|node| node.id == item) else {
        return false;
    };
    let Some(document) = world.get::<DocumentReference>(editor).map(|r| r.document()) else {
        return false;
    };
    let workbench = world.workbench();
    let Some(text) = workbench
        .cast_document::<TextBuffer>(document)
        .and_then(|handle| workbench.read_document(handle).cloned())
        .ok()
    else {
        return false;
    };
    let snapshot = TextSnapshot {
        revision: key.revision,
        text,
    };
    let (anchor, head) = match how {
        Reveal::Focus | Reveal::Preview => (node.name_range.start, node.name_range.end),
        Reveal::SelectAll => (node.range.start, node.range.end),
        Reveal::Fold => (node.name_range.start, node.name_range.start),
    };
    let Ok(selection) = TextSelection::new(&snapshot.text, anchor, head) else {
        return false;
    };
    let navigation = Navigation {
        revision: key.revision,
        range: node.name_range.clone(),
        selection: Some(selection),
        reveal: Scroll::Center,
        focus: false,
    };
    let accepted = world
        .get_mut::<EditorPanel>(editor)
        .is_some_and(|mut panel| {
            let accepted = panel.view.request_navigation(&snapshot, navigation).is_ok();
            if accepted && how == Reveal::Fold {
                panel.view.queue_command(EditorCommand::Fold);
            }
            accepted
        });
    match how {
        Reveal::Preview | Reveal::Fold => accepted,
        Reveal::Focus | Reveal::SelectAll => {
            accepted && world.layouts_mut().focus_panel(editor).is_ok()
        }
    }
}

/// Shade `range` of `editor`'s text this pass (an entry under the pointer),
/// if `key` is the analysis of its text as it is now.
pub(crate) fn peek(
    world: &mut World,
    editor: Entity,
    key: ObservationKey,
    range: std::ops::Range<usize>,
    pass: u64,
) {
    if current_outline(world, editor).is_none_or(|o| o.key != key) {
        return;
    }
    if let Some(mut panel) = world.get_mut::<EditorPanel>(editor) {
        panel.view.peek = Some((key.revision, range, pass));
    }
}

/// Open Structure: a new one, in the focused area.
pub struct ShowStructure;
impl bedtime_workbench::Action for ShowStructure {
    const KEY: &'static str = "code.show_structure";
    const LABEL: &'static str = "Structure";
}

/// Which text an outline is of: its document's source and language. Its
/// collapsed entries are remembered by it.
type Identity = (u64, u64);

/// What's kept of an outline Structure has stopped showing, to show it the
/// same way when it's back.
#[derive(Clone, Default)]
struct Remembered {
    collapsed: HashSet<u64>,
    selected: Option<u64>,
}

/// How many outlines' collapsed entries are remembered.
const REMEMBERED: usize = 24;

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
pub struct StructurePanel {
    /// Kinds of entry not shown: `symbols::Category::bit`s.
    #[reflect(default)]
    pub hidden: u16,
    /// Entries in order of name, rather than of where they are.
    #[reflect(default)]
    pub sort_by_name: bool,
    /// Don't select the entry the editor's caret is in.
    #[reflect(default)]
    pub ignore_caret: bool,
    /// The editor shown whichever is focused, while it's open.
    #[reflect(ignore)]
    pinned: Option<Entity>,
    /// What's shown: the editor and its analysis. Kept while a newer analysis
    /// of the same text runs, so the outline doesn't flicker as you type.
    #[reflect(ignore)]
    shown: Option<(Entity, Arc<Observation>)>,
    #[reflect(ignore)]
    collapsed: HashSet<u64>,
    #[reflect(ignore)]
    selected: Option<u64>,
    /// Outlines shown before, most recent last.
    #[reflect(ignore)]
    remembered: Vec<(Identity, Remembered)>,
    /// Only entries whose names match this (and their parents) are shown.
    #[reflect(ignore)]
    filter: String,
    /// The entry the caret was last seen in: a new one is selected.
    #[reflect(ignore)]
    caret: Option<u64>,
    /// An entry to scroll to.
    #[reflect(ignore)]
    scroll_to: Option<u64>,
    #[reflect(ignore)]
    details: symbols::Details,
    /// Give the filter field the keyboard (typing in the list starts one).
    #[reflect(ignore)]
    focus_filter: bool,
    /// The filter field had the keyboard last pass: egui takes it away as
    /// Escape is pressed, before the panel sees the key.
    #[reflect(ignore)]
    filter_had_focus: bool,
}

impl StructurePanel {
    /// The editor this panel shows: its pinned one, or the source.
    fn editor(&mut self, world: &World) -> Option<Entity> {
        if let Some(pinned) = self.pinned
            && !has_outline(world, pinned)
        {
            self.pinned = None;
        }
        self.pinned.or(world.resource::<StructureSource>().0)
    }

    fn refresh(&mut self, world: &World) {
        let source = self.editor(world);
        let shown = source.and_then(|editor| {
            if let Some(observation) = current_outline(world, editor) {
                return Some((editor, observation));
            }
            // Keep the last outline of this text while it's reanalysed.
            let (old_editor, old) = self.shown.as_ref()?;
            let document = world.get::<DocumentReference>(editor)?.document();
            let session = &world.get::<DocumentSyntax>(document.entity())?.session;
            let key = session.key()?;
            (*old_editor == editor
                && key.source == old.key.source
                && key.language_generation == old.key.language_generation
                && matches!(
                    session.status(),
                    AnalysisStatus::Pending | AnalysisStatus::Ready
                ))
            .then(|| (editor, old.clone()))
        });
        let identity = |shown: &Option<(Entity, Arc<Observation>)>| {
            shown
                .as_ref()
                .map(|(editor, o)| (*editor, o.key.source, o.key.language_generation))
        };
        if identity(&self.shown) != identity(&shown) {
            self.switch(
                identity(&self.shown).map(|(_, source, language)| (source, language)),
                identity(&shown).map(|(_, source, language)| (source, language)),
            );
        }
        self.shown = shown;
        self.follow_caret(world);
    }

    /// Put away how one outline was shown, and bring back another's.
    fn switch(&mut self, from: Option<Identity>, to: Option<Identity>) {
        if let Some(from) = from {
            self.remembered.retain(|(identity, _)| *identity != from);
            self.remembered.push((
                from,
                Remembered {
                    collapsed: std::mem::take(&mut self.collapsed),
                    selected: self.selected.take(),
                },
            ));
            if self.remembered.len() > REMEMBERED {
                self.remembered.remove(0);
            }
        }
        let back = to
            .and_then(|to| {
                let at = self.remembered.iter().position(|(i, _)| *i == to)?;
                Some(self.remembered.remove(at).1)
            })
            .unwrap_or_default();
        self.collapsed = back.collapsed;
        self.selected = back.selected;
        self.caret = None;
        self.scroll_to = self.selected;
    }

    /// Select the entry the caret has moved into, opening its parents.
    fn follow_caret(&mut self, world: &World) {
        if self.ignore_caret {
            return;
        }
        let Some((editor, observation)) = &self.shown else {
            return;
        };
        let Some(index) = caret_entry(world, *editor, observation) else {
            // Outside any entry, or the outline is being redone: wait.
            if current_outline(world, *editor).is_some_and(|o| o.key == observation.key) {
                self.caret = None;
            }
            return;
        };
        let id = observation.outline[index].id;
        if self.caret == Some(id) {
            return;
        }
        self.caret = Some(id);
        self.selected = Some(id);
        self.scroll_to = Some(id);
        for parent in symbols::ancestry(&observation.outline, index).skip(1) {
            self.collapsed.remove(&observation.outline[parent].id);
        }
    }
}

impl Panel for StructurePanel {
    const LABEL: &'static str = "Structure";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(mut state) = world.entity_mut(panel).take::<Self>() else {
            return;
        };
        state.refresh(world);
        let intents = view::draw(ui, world, context, panel, &mut state);
        view::apply(world, &mut state, intents);
        world.entity_mut(panel).insert(state);
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod ui_tests;
