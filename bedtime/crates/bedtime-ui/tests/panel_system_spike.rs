//! Spike (throwaway): a panel's draw as a read-only Bevy system.
//!
//! Proves three things, headlessly:
//! 1. a `ReadOnlySystem` takes a borrowed `&mut egui::Ui` (and the panel's
//!    state) as its input, next to ordinary system params;
//! 2. while it runs, widgets read the `World` ambiently (`world(ui)`);
//! 3. changes are queued (the system's `Commands`, and an ambient queue for
//!    widgets) and applied after the draw.
use bedtime_ui::egui;
use bevy::ecs::system::{BoxedReadOnlySystem, ReadOnlySystem, SystemInput};
use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use std::cell::{Cell, RefCell};

// --- The framework side ------------------------------------------------------

/// A panel draw system's input: the `Ui` to draw in, the panel, and its state.
pub struct PanelUi<'a, P: Component> {
    pub ui: &'a mut egui::Ui,
    pub entity: Entity,
    pub state: &'a P,
}

impl<P: Component> SystemInput for PanelUi<'_, P> {
    type Param<'i> = PanelUi<'i, P>;
    type Inner<'i> = PanelUi<'i, P>;
    fn wrap(this: Self::Inner<'_>) -> Self::Param<'_> {
        this
    }
}

impl<P: Component> PanelUi<'_, P> {
    /// Change the panel's state after the draw.
    pub fn update(&self, change: impl FnOnce(&mut P) + Send + 'static)
    where
        P: Component<Mutability = bevy::ecs::component::Mutable>,
    {
        let entity = self.entity;
        queue(self.ui, move |world: &mut World| {
            if let Some(mut state) = world.get_mut::<P>(entity) {
                change(&mut state);
            }
        });
    }
}

thread_local! {
    static WORLD: Cell<*const World> = const { Cell::new(std::ptr::null()) };
    static QUEUE: RefCell<CommandQueue> = RefCell::new(CommandQueue::default());
}

/// The world, for the length of a draw; restores the outer one when dropped.
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

/// The world being drawn. The borrow is tied to the `Ui`, which can't outlive
/// the draw it was made in.
pub fn world(_ui: &egui::Ui) -> &World {
    let world = WORLD.get();
    assert!(!world.is_null(), "world(ui) outside a panel draw");
    // SAFETY: set from a `&World` that outlives the draw (`Ambient`), and the
    // draw system is read-only, so nothing holds `&mut` to any of it.
    unsafe { &*world }
}

/// Queue a change, applied after the draw.
pub fn queue(_ui: &egui::Ui, command: impl Command<Out = ()>) {
    assert!(!WORLD.get().is_null(), "queue(ui) outside a panel draw");
    QUEUE.with_borrow_mut(|queue| queue.push(command));
}

/// A registered panel draw. `ReadOnlySystem` is what refuses `ResMut` and
/// `Query<&mut T>` at compile time.
pub struct PanelDraw<P: Component>(BoxedReadOnlySystem<PanelUi<'static, P>>);

impl<P: Component> PanelDraw<P> {
    pub fn new<M, S>(world: &mut World, system: S) -> Self
    where
        S: IntoSystem<PanelUi<'static, P>, (), M>,
        S::System: ReadOnlySystem,
    {
        let mut system: BoxedReadOnlySystem<_> = Box::new(IntoSystem::into_system(system));
        system.initialize(world);
        Self(system)
    }

    pub fn draw(&mut self, world: &mut World, ui: &mut egui::Ui, entity: Entity) {
        {
            let world: &World = world;
            let Some(state) = world.get::<P>(entity) else {
                return;
            };
            let _ambient = Ambient::enter(world);
            self.0
                .run_readonly(PanelUi { ui, entity, state }, world)
                .expect("panel draw");
        }
        self.0.apply_deferred(world);
        QUEUE.take().apply(world);
    }
}

// --- An app's side -------------------------------------------------------------

#[derive(Component)]
struct Counter {
    count: i32,
}

#[derive(Resource)]
struct Step(i32);

#[derive(Resource, Default)]
struct Sent(Vec<String>);

/// A widget bound to the world: reads `Step` and queues a change itself.
struct StepButton;
impl egui::Widget for StepButton {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let step = world(ui).resource::<Step>().0;
        let response = ui.button(format!("Step is {step}"));
        // (A click, in real life.)
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
    PanelUi { ui, entity, state }: PanelUi<Counter>,
    step: Res<Step>,
    counters: Query<&Counter>,
    mut commands: Commands,
) {
    let total: i32 = counters.iter().map(|counter| counter.count).sum();
    ui.horizontal(|ui| {
        // A plain egui closure: no context threaded in, the widget still works.
        ui.label(format!("{} of {total}", state.count));
        ui.add(StepButton);
    });
    let (step, total) = (step.0, total);
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

/// `panel.update`, without destructuring the input.
fn doubling(panel: PanelUi<Counter>) {
    panel.ui.label(panel.state.count.to_string());
    panel.update(|counter| counter.count *= 2);
}

fn pass(mut draw: impl FnMut(&mut egui::Ui)) {
    let mut output = egui::Context::default().run_ui(default(), |ui| draw(ui));
    output.textures_delta.clear();
}

#[test]
fn a_panel_draws_as_a_read_only_system() {
    let mut world = World::new();
    world.insert_resource(Step(5));
    world.init_resource::<Sent>();
    let mut draw = PanelDraw::new(&mut world, counter);
    // Spawned after the system was initialised: its query must still see them.
    let panel = world.spawn(Counter { count: 1 }).id();
    world.spawn(Counter { count: 10 });

    pass(|ui| draw.draw(&mut world, ui, panel));
    assert_eq!(world.get::<Counter>(panel).unwrap().count, 6);
    assert_eq!(
        world.resource::<Sent>().0,
        ["system saw total 11", "widget saw step 5"]
    );

    // A second pass sees the first one's changes.
    pass(|ui| draw.draw(&mut world, ui, panel));
    assert_eq!(world.get::<Counter>(panel).unwrap().count, 11);
    assert_eq!(world.resource::<Sent>().0[2], "system saw total 16");

    let mut draw = PanelDraw::new(&mut world, doubling);
    pass(|ui| draw.draw(&mut world, ui, panel));
    assert_eq!(world.get::<Counter>(panel).unwrap().count, 22);
}

#[test]
#[should_panic(expected = "outside a panel draw")]
fn the_world_is_only_ambient_during_a_draw() {
    pass(|ui| {
        world(ui);
    });
}
