//! Save, Undo and Redo for the document a panel references. The commands and
//! their default keys are always registered; the handlers are opt-in, so an
//! app can supply its own.
use crate::*;
use crate::{Workbench, WorkbenchWorldExt};
use bevy::prelude::*;

/// Something new where you are: the focused panel's own new thing if it has
/// one (a terminal makes another terminal, with an `ActionHandler<New>`),
/// else a new untitled document of the app's kind (`set_new_document`),
/// beside the focused document.
pub struct New;
impl Action for New {
    const KEY: &'static str = "document.new";
    const LABEL: &'static str = "New";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyN)];
}

#[cfg(feature = "files")]
pub trait NewAppExt {
    /// Make `New` (with no panel of its own to handle it) open a new untitled
    /// document of model `M`: beside the focused panel if it shows a
    /// document, else where documents open.
    fn set_new_document<M: bedtime_filesystem::FileModel + Default>(&mut self) -> &mut Self;
}

#[cfg(feature = "files")]
impl NewAppExt for App {
    fn set_new_document<M: bedtime_filesystem::FileModel + Default>(&mut self) -> &mut Self {
        self.add_action_handler::<New>(Handler::new(new_document::<M>))
    }
}

#[cfg(feature = "files")]
fn new_document<M: bedtime_filesystem::FileModel + Default>(
    world: &mut World,
    invocation: ActionInvocation,
) -> ActionResult {
    let focused = invocation
        .entity
        .filter(|&panel| world.get::<DocumentReference>(panel).is_some());
    let area = focused.and_then(|panel| world.get::<InArea>(panel).map(|area| area.0));
    world
        .workbench_mut()
        .new_untitled::<M>(OpenOptions {
            target: Target::New,
            place: area.map(Placement::In),
            ..default()
        })
        .map(|_| ())
}

pub struct SaveDocument;
impl Action for SaveDocument {
    const KEY: &'static str = "document.save";
    const LABEL: &'static str = "Save";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyS)];
}

pub struct UndoDocument;
impl Action for UndoDocument {
    const KEY: &'static str = "document.undo";
    const LABEL: &'static str = "Undo";
    const DEFAULT_KEYS: &'static [KeyChord] = &[KeyChord::primary(KeyCode::KeyZ)];
    const REPEATS: bool = true;
}

pub struct RedoDocument;
impl Action for RedoDocument {
    const KEY: &'static str = "document.redo";
    const LABEL: &'static str = "Redo";
    const DEFAULT_KEYS: &'static [KeyChord] = &[
        KeyChord::new(Modifiers::PRIMARY.union(Modifiers::SHIFT), KeyCode::KeyZ),
        KeyChord::primary(KeyCode::KeyY),
    ];
    const REPEATS: bool = true;
}

pub(crate) fn install(app: &mut App) {
    app.register_action::<New>()
        .register_action::<SaveDocument>()
        .register_action::<UndoDocument>()
        .register_action::<RedoDocument>();
}

impl Workbench {
    /// Handle Save, Undo and Redo globally, for the focused panel's document.
    pub fn register_document_handlers(&mut self) {
        let world = &mut self.world;
        let mut registry = world.resource_mut::<ActionRegistry>();
        registry.set_global_handler::<SaveDocument>(Some(Handler::new(save).available_when(
            |world, panel| {
                info(world, panel).is_some_and(|info| info.dirty && info.writable && !info.saving)
            },
        )));
        registry.set_global_handler::<UndoDocument>(Some(Handler::new(undo).available_when(
            |world, panel| info(world, panel).is_some_and(|i| i.undo_label.is_some()),
        )));
        registry.set_global_handler::<RedoDocument>(Some(Handler::new(redo).available_when(
            |world, panel| info(world, panel).is_some_and(|i| i.redo_label.is_some()),
        )));
    }
}

/// The state of the document `panel` references.
fn info(world: &World, panel: Option<Entity>) -> Option<DocumentInfo> {
    let reference = world.get::<DocumentReference>(panel?)?;
    world
        .workbench()
        .inspect_document(reference.document())
        .ok()
}

fn target(world: &World, invocation: &ActionInvocation) -> Result<DocumentId, String> {
    let panel = invocation.entity.ok_or("No document is focused")?;
    let reference = world
        .get::<DocumentReference>(panel)
        .ok_or("The focused panel has no document")?;
    Ok(reference.document())
}

fn save(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let document = target(world, &invocation)?;
    world
        .workbench_mut()
        .save_document(document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn undo(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let document = target(world, &invocation)?;
    world
        .workbench_mut()
        .undo_document(document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn redo(world: &mut World, invocation: ActionInvocation) -> ActionResult {
    let document = target(world, &invocation)?;
    world
        .workbench_mut()
        .redo_document(document)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(all(test, feature = "files"))]
mod tests {
    use super::*;
    use crate::tests::layout;
    use bedtime_document::TextBuffer;

    #[bedtime_ui::panel]
    #[derive(Default)]
    struct TextView;
    impl Panel for TextView {
        const LABEL: &'static str = "Text";
        fn draw(
            _: &mut bedtime_ui::egui::Ui,
            _: &mut World,
            _: &bedtime_ui::UiContext<'_>,
            _: Entity,
        ) {
        }
    }

    /// A panel with its own New: it counts them.
    #[bedtime_ui::panel]
    #[derive(Default)]
    #[require(ActionHandler<New> = ActionHandler::new(Handler::new(count)))]
    struct Console;
    impl Panel for Console {
        const LABEL: &'static str = "Console";
        fn draw(
            _: &mut bedtime_ui::egui::Ui,
            _: &mut World,
            _: &bedtime_ui::UiContext<'_>,
            _: Entity,
        ) {
        }
    }

    #[derive(Resource, Default)]
    struct Consoles(usize);

    fn count(world: &mut World, _: ActionInvocation) -> ActionResult {
        world.resource_mut::<Consoles>().0 += 1;
        Ok(())
    }

    fn new(app: &mut App) {
        app.world_mut()
            .resource_mut::<ActionRequests>()
            .submit(ActionRequest {
                action: ActionId::of::<New>(),
                context: InvocationContext::Focused,
                source: None,
            });
        app.update();
    }

    fn viewers(world: &mut World) -> Vec<Entity> {
        let layout = world.layouts().active().unwrap();
        world
            .layouts()
            .panels(layout)
            .into_iter()
            .filter(|&panel| world.get::<TextView>(panel).is_some())
            .collect()
    }

    #[test]
    fn new_is_the_focused_panels_own_else_a_document_beside_the_focused_one() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugins)
            .init_resource::<Consoles>()
            .register_viewer::<TextBuffer, TextView>("Text")
            .set_new_document::<TextBuffer>();
        layout(app.world_mut());
        app.update();

        new(&mut app);
        let world = app.world_mut();
        let first = viewers(world);
        assert_eq!(first.len(), 1, "an untitled document");

        // Beside it: in its area, though another is larger.
        let area = world.get::<InArea>(first[0]).unwrap().0;
        world
            .layouts_mut()
            .split_area(area, SplitAxis::Horizontal, 2500, [100, 100])
            .unwrap();
        world.layouts_mut().focus_panel(first[0]).unwrap();
        new(&mut app);
        let world = app.world_mut();
        let both = viewers(world);
        assert_eq!(both.len(), 2);
        assert!(
            both.iter()
                .all(|&panel| world.get::<InArea>(panel).unwrap().0 == area)
        );

        // A focused panel with its own New handles it.
        let console = world.open_panel(Console);
        world.layouts_mut().focus_panel(console).unwrap();
        new(&mut app);
        let world = app.world_mut();
        assert_eq!(world.resource::<Consoles>().0, 1);
        assert_eq!(viewers(world).len(), 2);
    }
}
