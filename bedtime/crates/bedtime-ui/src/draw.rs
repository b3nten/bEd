//! Drawing a panel with a system.
//!
//! A panel's draw can be an ordinary Bevy system: what it reads is in its
//! parameters, and what it changes is queued (its `Commands`, or
//! `PanelUi::update`) and applied when the draw is done.
//!
//! ```ignore
//! impl Panel for Counter {
//!     const LABEL: &'static str = "Counter";
//!     fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
//!         draw::with_system(ui, world, context, panel, counter);
//!     }
//! }
//!
//! fn counter(PanelUi { ui, state, .. }: PanelUi<Counter>, step: Res<Step>, mut commands: Commands) {
//!     ui.add(Text::heading(state.count.to_string()));
//! }
//! ```
//!
//! A panel with a lot of state of its own that drawing changes (what's
//! selected and hovered, what it has laid out) lends that to its system
//! instead, as `&mut`: `with_view` and `ViewUi`. The world is shared all the
//! same.
//!
//! The system must be read-only (`Res`, `Query<&T>`, `Commands`; not
//! `ResMut` or `Query<&mut T>`), which the compiler checks. That is what
//! lets widgets read the world while it runs (`draw::world(ui)`) and queue
//! their own changes (`draw::queue(ui, …)`), with nothing passed to them.
use crate::UiContext;
use bevy::ecs::system::{BoxedReadOnlySystem, ReadOnlySystem, SystemInput};
use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// A panel draw system's input: the `Ui` to draw in, the panel and its
/// state, and the surface's `UiContext` (for what still asks for one).
///
/// Take it apart in the system's signature, to use the `Ui` and the state
/// side by side: `PanelUi { ui, state, .. }: PanelUi<Counter>`.
pub struct PanelUi<'a, P: Component> {
    pub ui: &'a mut egui::Ui,
    pub entity: Entity,
    pub state: &'a P,
    pub context: UiContext<'a>,
}

impl<P: Component> SystemInput for PanelUi<'_, P> {
    type Param<'i> = PanelUi<'i, P>;
    type Inner<'i> = PanelUi<'i, P>;
    fn wrap(this: Self::Inner<'_>) -> Self::Param<'_> {
        this
    }
}

impl<P: Component<Mutability = bevy::ecs::component::Mutable>> PanelUi<'_, P> {
    /// Change the panel's state when the draw is done.
    pub fn update(&self, change: impl FnOnce(&mut P) + Send + 'static) {
        update(self.ui, self.entity, change);
    }
}

/// Change `panel`'s state when the draw is done (as `PanelUi::update`, for
/// where the input's been taken apart).
pub fn update<P: Component<Mutability = bevy::ecs::component::Mutable>>(
    ui: &egui::Ui,
    panel: Entity,
    change: impl FnOnce(&mut P) + Send + 'static,
) {
    queue(ui, move |world: &mut World| {
        if let Some(mut state) = world.get_mut::<P>(panel) {
            change(&mut state);
        }
    });
}

/// A draw system's input when the panel lends it its view state (see
/// `with_view`): the `Ui`, the panel, the view to read and change, and the
/// surface's `UiContext`.
pub struct ViewUi<'a, V: 'static> {
    pub ui: &'a mut egui::Ui,
    pub entity: Entity,
    pub view: &'a mut V,
    pub context: UiContext<'a>,
}

impl<V: 'static> SystemInput for ViewUi<'_, V> {
    type Param<'i> = ViewUi<'i, V>;
    type Inner<'i> = ViewUi<'i, V>;
    fn wrap(this: Self::Inner<'_>) -> Self::Param<'_> {
        this
    }
}

thread_local! {
    static WORLD: Cell<*const World> = const { Cell::new(std::ptr::null()) };
    static QUEUE: RefCell<CommandQueue> = RefCell::new(CommandQueue::default());
}

/// The world, for the length of a draw; the one before it when dropped.
struct Ambient(*const World);
impl Ambient {
    fn enter(world: &World) -> Self {
        Self(WORLD.replace(world))
    }
}
impl Drop for Ambient {
    fn drop(&mut self) {
        WORLD.set(self.0);
    }
}

/// Whether a system is drawing: `world` and `queue` work.
pub fn is_drawing() -> bool {
    !WORLD.get().is_null()
}

/// The world being drawn, for a widget that shows something of it. Only
/// while a system draws (`with_system`): panics otherwise.
///
/// The borrow is tied to the `Ui`, which can't outlive the draw it's made in.
pub fn world(_ui: &egui::Ui) -> &World {
    let world = WORLD.get();
    assert!(
        !world.is_null(),
        "draw::world(ui) outside a panel drawn by a system"
    );
    // SAFETY: set from a `&World` that outlives the draw (`Ambient`), and the
    // draw system is read-only, so nothing holds `&mut` to any of it.
    unsafe { &*world }
}

/// Queue a change to the world, made when the draw is done. Only while a
/// system draws (`with_system`): panics otherwise.
pub fn queue(_ui: &egui::Ui, command: impl Command<Out = ()>) {
    assert!(
        is_drawing(),
        "draw::queue(ui) outside a panel drawn by a system"
    );
    QUEUE.with_borrow_mut(|queue| queue.push(command));
}

/// Draw `content` with the world shared, for a panel that isn't drawn by a
/// system (it has `&mut World`) to use widgets that read the world
/// (`draw::world`) and queue changes (`draw::queue`). The changes are made
/// when `content` is done; it can't use `world` itself meanwhile.
pub fn with_world<R>(
    ui: &mut egui::Ui,
    world: &mut World,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let result = {
        let _ambient = Ambient::enter(world);
        content(ui)
    };
    QUEUE.take().apply(world);
    result
}

/// The draw systems, initialised, by their type.
#[derive(Resource, Default)]
struct DrawSystems(HashMap<TypeId, Box<dyn Any + Send + Sync>>);

/// `system`, initialised: the one kept from the last draw, or a new one.
fn take_system<I, M, S>(world: &mut World, system: S) -> BoxedReadOnlySystem<I>
where
    I: SystemInput + 'static,
    S: IntoSystem<I, (), M> + 'static,
    S::System: ReadOnlySystem,
{
    let kept = world
        .get_resource_or_init::<DrawSystems>()
        .0
        .remove(&TypeId::of::<S>())
        .and_then(|system| system.downcast::<BoxedReadOnlySystem<I>>().ok());
    match kept {
        Some(system) => *system,
        None => {
            let mut system: BoxedReadOnlySystem<I> = Box::new(IntoSystem::into_system(system));
            system.initialize(world);
            system
        }
    }
}

/// After `system` drew: make the changes it and its widgets queued, and keep
/// it for the next draw.
fn finish<I, S>(
    world: &mut World,
    mut system: BoxedReadOnlySystem<I>,
    result: Option<Result<(), bevy::ecs::system::RunSystemError>>,
) where
    I: SystemInput + 'static,
    S: 'static,
{
    system.apply_deferred(world);
    QUEUE.take().apply(world);
    if let Some(Err(error)) = result {
        warn_once!("A panel couldn't be drawn: {error}");
    }
    world
        .resource_mut::<DrawSystems>()
        .0
        .insert(TypeId::of::<S>(), Box::new(system));
}

/// Draw `panel` with `system`: a `Panel::draw` that's this one call.
///
/// The system runs with the world shared (so it must be read-only), then
/// what it and its widgets queued is applied. A panel that's gone, or isn't
/// a `P`, isn't drawn.
pub fn with_system<P, M, S>(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    panel: Entity,
    system: S,
) where
    P: Component,
    S: IntoSystem<PanelUi<'static, P>, (), M> + 'static,
    S::System: ReadOnlySystem,
{
    let mut system = take_system(world, system);
    let result = {
        let world: &World = world;
        world.get::<P>(panel).map(|state| {
            let _ambient = Ambient::enter(world);
            let input = PanelUi {
                ui,
                entity: panel,
                state,
                context: *context,
            };
            system.run_readonly(input, world)
        })
    };
    finish::<_, S>(world, system, result);
}

/// Draw `panel` with `system`, lending it the panel's view state: what
/// `view` picks out of the panel (`|panel: &mut GraphPanel| &mut panel.view`,
/// written in the call, where its lifetimes are inferred).
///
/// The view is taken out of the panel for the draw (its `Default` stands in)
/// and put back after, so the system has it as `&mut` while the world, the
/// panel included, is shared. For a panel whose drawing changes a lot of
/// state that is only its own; what belongs to the world is still queued.
/// What's queued is applied once the view is back, so it can change the
/// view too.
pub fn with_view<P, V, M, S>(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    panel: Entity,
    view: impl Fn(&mut P) -> &mut V,
    system: S,
) where
    P: Component<Mutability = bevy::ecs::component::Mutable>,
    V: Default + 'static,
    S: IntoSystem<ViewUi<'static, V>, (), M> + 'static,
    S::System: ReadOnlySystem,
{
    let Some(lent) = world
        .get_mut::<P>(panel)
        .map(|mut state| std::mem::take(view(&mut state)))
    else {
        return;
    };
    with_lent(ui, world, context, panel, lent, system, |world, lent| {
        if let Some(mut state) = world.get_mut::<P>(panel) {
            *view(&mut state) = lent;
        }
    });
}

/// Draw `panel` with `system`, lending it `lent`: anything the caller took
/// out of the world for the draw (a resource and the panel's state together,
/// say), which the system has as `&mut` while the world is shared.
///
/// `restore` puts it back, and can keep something of it for the caller (what
/// the draw asked for): that's what's returned. It runs before what was
/// queued is applied, so a queued change to what was lent is made to the
/// real thing and kept. `with_view` is this for a part of the panel itself.
pub fn with_lent<V, R, M, S>(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    panel: Entity,
    mut lent: V,
    system: S,
    restore: impl FnOnce(&mut World, V) -> R,
) -> R
where
    V: 'static,
    S: IntoSystem<ViewUi<'static, V>, (), M> + 'static,
    S::System: ReadOnlySystem,
{
    let mut system = take_system(world, system);
    let result = {
        let world: &World = world;
        let _ambient = Ambient::enter(world);
        let input = ViewUi {
            ui,
            entity: panel,
            view: &mut lent,
            context: *context,
        };
        system.run_readonly(input, world)
    };
    let kept = restore(world, lent);
    finish::<_, S>(world, system, Some(result));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Metrics, MotionPolicy, Theme, icon_cache::IconCache};
    use std::sync::Mutex;

    #[derive(Component)]
    struct Counter {
        count: i32,
    }

    #[derive(Resource)]
    struct Step(i32);

    #[derive(Resource)]
    struct Absent;

    #[derive(Resource, Default)]
    struct Sent(Vec<String>);

    /// A widget bound to the world: reads `Step` and queues a change itself.
    struct StepButton;
    impl egui::Widget for StepButton {
        fn ui(self, ui: &mut egui::Ui) -> egui::Response {
            let step = world(ui).resource::<Step>().0;
            let response = ui.button(format!("Step is {step}"));
            queue(ui, move |world: &mut World| {
                world
                    .resource_mut::<Sent>()
                    .0
                    .push(format!("widget saw step {step}"));
            });
            response
        }
    }

    fn counter(
        PanelUi {
            ui, entity, state, ..
        }: PanelUi<Counter>,
        step: Res<Step>,
        counters: Query<&Counter>,
        mut commands: Commands,
    ) {
        let total: i32 = counters.iter().map(|counter| counter.count).sum();
        ui.horizontal(|ui| {
            // A plain egui closure: nothing passed in, the widget still works.
            ui.label(format!("{} of {total}", state.count));
            ui.add(StepButton);
        });
        let step = step.0;
        commands.queue(move |world: &mut World| {
            world
                .resource_mut::<Sent>()
                .0
                .push(format!("system saw total {total}"));
            if let Some(mut counter) = world.get_mut::<Counter>(entity) {
                counter.count += step;
            }
        });
    }

    fn doubling(panel: PanelUi<Counter>) {
        panel.ui.label(panel.state.count.to_string());
        panel.update(|counter| counter.count *= 2);
    }

    fn needs_absent(_: PanelUi<Counter>, _: Res<Absent>, mut commands: Commands) {
        commands.queue(|world: &mut World| world.resource_mut::<Sent>().0.push("ran".into()));
    }

    /// Draw `panel` once with `system`, in a bare egui pass.
    fn pass<M, S>(world: &mut World, panel: Entity, system: S)
    where
        S: IntoSystem<PanelUi<'static, Counter>, (), M> + Copy + 'static,
        S::System: ReadOnlySystem,
    {
        let icons = Mutex::new(IconCache::default());
        let (theme, metrics) = (Theme::dark(), Metrics::default());
        let mut output = egui::Context::default().run_ui(default(), |ui| {
            let context = UiContext {
                view: panel,
                surface: panel,
                theme: &theme,
                metrics: &metrics,
                motion: MotionPolicy::default(),
                icons: &icons,
            };
            with_system(ui, world, &context, panel, system);
        });
        output.textures_delta.clear();
    }

    fn world_with_counter() -> (World, Entity) {
        let mut world = World::new();
        world.insert_resource(Step(5));
        world.init_resource::<Sent>();
        let panel = world.spawn(Counter { count: 1 }).id();
        (world, panel)
    }

    #[test]
    fn a_panel_draws_as_a_read_only_system_and_its_changes_are_made_after() {
        let (mut world, panel) = world_with_counter();
        pass(&mut world, panel, counter);
        assert_eq!(world.get::<Counter>(panel).unwrap().count, 6);
        assert_eq!(
            world.resource::<Sent>().0,
            ["system saw total 1", "widget saw step 5"]
        );

        // Spawned after the system was first run: its query still sees it,
        // and the pass sees the last one's changes.
        world.spawn(Counter { count: 10 });
        pass(&mut world, panel, counter);
        assert_eq!(world.get::<Counter>(panel).unwrap().count, 11);
        assert_eq!(world.resource::<Sent>().0[2], "system saw total 16");

        pass(&mut world, panel, doubling);
        assert_eq!(world.get::<Counter>(panel).unwrap().count, 22);
    }

    #[test]
    fn a_panel_that_is_gone_or_another_kind_is_not_drawn() {
        let (mut world, panel) = world_with_counter();
        let other = world.spawn_empty().id();
        pass(&mut world, other, counter);
        world.despawn(panel);
        pass(&mut world, panel, counter);
        assert!(world.resource::<Sent>().0.is_empty());
    }

    #[test]
    fn a_system_whose_parameters_are_missing_does_not_run_or_panic() {
        let (mut world, panel) = world_with_counter();
        pass(&mut world, panel, needs_absent);
        assert!(world.resource::<Sent>().0.is_empty());
        world.insert_resource(Absent);
        pass(&mut world, panel, needs_absent);
        assert_eq!(world.resource::<Sent>().0, ["ran"]);
    }

    #[derive(Component, Default)]
    struct Graph {
        view: Box<GraphView>,
    }

    #[derive(Default)]
    struct GraphView {
        hovered: Option<usize>,
        draws: u32,
        close: bool,
        /// Changed by what the draw queued.
        queued: u32,
    }

    fn graph(
        ViewUi {
            ui, entity, view, ..
        }: ViewUi<Box<GraphView>>,
        step: Res<Step>,
        graphs: Query<&Graph>,
        mut commands: Commands,
    ) {
        // The view is the system's to change; the world is read, the panel
        // too (its view is out, a default in its place).
        view.draws += 1;
        view.hovered = Some(step.0 as usize);
        assert_eq!(graphs.get(entity).unwrap().view.draws, 0);
        ui.add(StepButton);
        // A queued change to the view: made to the one that's back.
        commands.queue(move |world: &mut World| {
            if let Some(mut graph) = world.get_mut::<Graph>(entity) {
                graph.view.queued += 1;
            }
        });
        if view.close {
            commands.entity(entity).despawn();
        }
    }

    #[test]
    fn a_panel_lends_its_view_to_the_system_and_has_it_back() {
        let (mut world, _) = world_with_counter();
        let panel = world.spawn(Graph::default()).id();
        let draw = |world: &mut World| {
            let icons = Mutex::new(IconCache::default());
            let (theme, metrics) = (Theme::dark(), Metrics::default());
            let mut output = egui::Context::default().run_ui(default(), |ui| {
                let context = UiContext {
                    view: panel,
                    surface: panel,
                    theme: &theme,
                    metrics: &metrics,
                    motion: MotionPolicy::default(),
                    icons: &icons,
                };
                with_view(
                    ui,
                    world,
                    &context,
                    panel,
                    |graph: &mut Graph| &mut graph.view,
                    graph,
                );
            });
            output.textures_delta.clear();
        };
        draw(&mut world);
        draw(&mut world);
        let view = &world.get::<Graph>(panel).unwrap().view;
        assert_eq!((view.draws, view.hovered), (2, Some(5)));
        assert_eq!(view.queued, 2, "queued changes to the view are kept");
        assert_eq!(
            world.resource::<Sent>().0.len(),
            2,
            "its widgets queued too"
        );

        // A draw that closes its own panel: nothing to put the view back in.
        world.get_mut::<Graph>(panel).unwrap().view.close = true;
        draw(&mut world);
        assert!(world.get_entity(panel).is_err());
        draw(&mut world);
    }

    #[test]
    #[should_panic(expected = "outside a panel drawn by a system")]
    fn the_world_is_only_there_while_a_system_draws() {
        let mut output = egui::Context::default().run_ui(default(), |ui| {
            world(ui);
        });
        output.textures_delta.clear();
    }
}
