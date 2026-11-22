//! Ending the session: quitting, or an app's own exit (switching workspace).
//!
//! An exit settles what would be lost, finishes background work, then runs
//! its last step: `AppExit` for a quit. A restart (`restart_session`,
//! switching workspace) runs `OnWorkspaceClosing`, finishes only the
//! workspace's work (tasks with no owner, or owned by an `InWorkspace`
//! entity), runs its step, then the session goes on: work is accepted again.
//!
//! What would be lost is the business of [`ExitGate`]s, which hold an exit
//! until they're settled, or cancel it. Documents add one (unsaved documents
//! are saved or asked about); an app without them has none, and exits once
//! its work is done.
use crate::*;
use crate::{Bedtime, BedtimeRef};
use bevy::prelude::*;

/// What a gate says of an exit, each frame it's asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Nothing of its is in the way.
    Open,
    /// Something is being settled: a question is up, or a save is running.
    Waiting,
    /// The user called the exit off.
    Cancelled,
}

/// Something that may hold an exit: unsaved work, say.
#[derive(Clone, Copy)]
pub struct ExitGate {
    /// Whether an exit may start at all; an error says why not ("Finish
    /// typing in … first").
    pub may_start: fn(&World) -> Result<(), String>,
    /// An exit called `name` ("Quit", "Switch workspace") has started: begin
    /// settling what's in its way.
    pub begin: fn(&mut World, &str),
    /// Asked each frame until every gate is open, and again before
    /// background work is stopped.
    pub poll: fn(&mut World) -> Gate,
}

#[derive(Resource, Default)]
pub(crate) struct ExitGates(Vec<ExitGate>);

pub trait ExitAppExt {
    /// Hold exits until `gate` is open.
    fn add_exit_gate(&mut self, gate: ExitGate) -> &mut Self;
}

impl ExitAppExt for App {
    fn add_exit_gate(&mut self, gate: ExitGate) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<ExitGates>()
            .0
            .push(gate);
        self
    }
}

/// When an exit moves on. What settles an exit's gates runs between the two.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExitSystems {
    /// Quit requests become exits.
    Request,
    /// The exit goes on, if its gates are open.
    Advance,
}

/// An exit's last step, once what's in its way is settled and background
/// work has finished.
type ExitStep = Box<dyn FnOnce(&mut World) + Send + Sync>;

/// The exit under way, if any.
#[derive(Resource, Default)]
struct PendingExit(Option<Exit>);

struct Exit {
    /// Taken when it runs.
    then: Option<ExitStep>,
    /// The session goes on after `then`.
    restart: bool,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<PendingExit>()
        .init_resource::<ExitGates>()
        .configure_sets(Update, (ExitSystems::Request, ExitSystems::Advance).chain())
        .add_systems(
            Update,
            (
                request_quit.in_set(ExitSystems::Request),
                advance.in_set(ExitSystems::Advance),
            ),
        );
}

fn gates(world: &World) -> Vec<ExitGate> {
    world.resource::<ExitGates>().0.clone()
}

/// What the gates say together: cancelled if any is, else waiting if any is.
fn poll(world: &mut World) -> Gate {
    let mut all = Gate::Open;
    for gate in gates(world) {
        match (gate.poll)(world) {
            Gate::Cancelled => return Gate::Cancelled,
            Gate::Waiting => all = Gate::Waiting,
            Gate::Open => {}
        }
    }
    all
}

impl BedtimeRef {
    /// Whether an exit (a quit, or `restart_session`) is under way: settling
    /// unsaved work, or finishing background work. False again once it's
    /// cancelled or done.
    pub fn is_exiting(&self) -> bool {
        self.world.resource::<PendingExit>().0.is_some()
    }

    pub fn work_finished(&self) -> bool {
        self.world.resource::<ManagedWork>().is_finished()
    }
}

impl Bedtime {
    /// End the session, as quitting does: what would be lost is settled
    /// (unsaved documents are saved or asked about; `name` titles the
    /// prompt, "Quit" or "Switch workspace"), background work finishes, then
    /// `then` runs. Quitting's `then` sends `AppExit`. If the user cancels,
    /// nothing happens and the session goes on. Refused while another exit is
    /// under way, or while a gate says it can't start (an editor has
    /// uncommitted input).
    pub fn request_exit(
        &mut self,
        name: impl Into<String>,
        then: impl FnOnce(&mut World) + Send + Sync + 'static,
    ) -> Result<(), String> {
        self.end_session(name.into(), Box::new(then), false)
    }

    /// End the session as `request_exit` does, run `then`, and go on: a new
    /// session in the same app (switching workspace). `then` replaces what
    /// the session had; background work is accepted again after it.
    pub fn restart_session(
        &mut self,
        name: impl Into<String>,
        then: impl FnOnce(&mut World) + Send + Sync + 'static,
    ) -> Result<(), String> {
        self.end_session(name.into(), Box::new(then), true)
    }

    fn end_session(&mut self, name: String, then: ExitStep, restart: bool) -> Result<(), String> {
        let world = &mut self.world;
        if world.resource::<PendingExit>().0.is_some() {
            return Err("Already exiting".into());
        }
        let gates = gates(world);
        for gate in &gates {
            (gate.may_start)(world)?;
        }
        world.resource_mut::<PendingExit>().0 = Some(Exit {
            then: Some(then),
            restart,
        });
        for gate in &gates {
            (gate.begin)(world, &name);
        }
        Ok(())
    }

    /// Stop background work now, without an exit: for headless hosts and
    /// tests. Refused while a gate has something to settle (a save in
    /// flight, uncommitted input), which stopping would lose; an exit waits
    /// for those.
    pub fn shutdown_work(&mut self) -> Result<(), String> {
        let world = &mut self.world;
        for gate in gates(world) {
            (gate.may_start)(world)?;
        }
        if poll(world) != Gate::Open {
            return Err("Work is still being saved".into());
        }
        world.resource_mut::<ManagedWork>().shutdown();
        Ok(())
    }
}

fn request_quit(
    world: &mut World,
    mut cursor: Local<bevy::ecs::message::MessageCursor<CloseRequested>>,
) {
    let requested = cursor
        .read(world.resource::<Messages<CloseRequested>>())
        .count()
        > 0;
    if !requested || world.bedtime().is_exiting() {
        return;
    }
    if let Err(error) = world.bedtime_mut().request_exit("Quit", |world| {
        world.write_message(AppExit::Success);
    }) {
        report_error(world, "Quit", &error, None);
    }
}

fn advance(world: &mut World) {
    let Some(mut exit) = world.resource_mut::<PendingExit>().0.take() else {
        return;
    };
    if !world.resource::<ManagedWork>().is_closing() {
        // Its gates: asked until they're open, and once more now that they
        // are (a save may have started since).
        match poll(world) {
            Gate::Cancelled => return,
            Gate::Waiting => {
                world.resource_mut::<PendingExit>().0 = Some(exit);
                return;
            }
            Gate::Open => {}
        }
        if exit.restart {
            // The session goes on: stop only the workspace's work.
            world.run_schedule(crate::OnWorkspaceClosing);
            let in_workspace: Vec<Entity> = world
                .query_filtered::<Entity, With<crate::InWorkspace>>()
                .iter(world)
                .collect();
            world
                .resource_mut::<ManagedWork>()
                .shutdown_where(|owner| owner.is_none_or(|o| in_workspace.contains(&o)));
        } else {
            world.resource_mut::<ManagedWork>().shutdown();
        }
    }
    if world.resource::<ManagedWork>().is_closing()
        && world.resource::<ManagedWork>().is_finished()
        && let Some(then) = exit.then.take()
    {
        then(world);
        if exit.restart {
            world.resource_mut::<ManagedWork>().reopen();
            return;
        }
    }
    world.resource_mut::<PendingExit>().0 = Some(exit);
}

/// Explain a failure with an error dialog when there is UI, otherwise log it.
pub(crate) fn report_error(world: &mut World, title: &str, message: &str, details: Option<String>) {
    world.bedtime_mut().request_error_dialog(ErrorDialog {
        title: title.into(),
        message: message.into(),
        details,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU8, Ordering};

    fn exited(app: &App) -> bool {
        !app.world().resource::<Messages<AppExit>>().is_empty()
    }

    /// An app without documents: nothing is in a quit's way.
    #[test]
    fn quitting_without_documents_exits_once_work_is_done() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, crate::AppCorePlugins));
        app.update();
        assert!(!app.world().bedtime().is_exiting());
        app.world_mut()
            .write_message(CloseRequested { window: None });
        for _ in 0..4 {
            app.update();
        }
        assert!(exited(&app));
        // No documents: nothing of theirs is there to act on.
        #[cfg(feature = "documents")]
        assert!(
            app.world()
                .resource::<ActionRegistry>()
                .label(&ActionId::of::<crate::New>())
                .is_none()
        );
    }

    /// What the test's gate says: 0 open, 1 waiting, 2 cancelled.
    static GATE: AtomicU8 = AtomicU8::new(0);

    #[test]
    fn a_gate_holds_an_exit_or_calls_it_off() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, crate::AppCorePlugins))
            .add_exit_gate(ExitGate {
                may_start: |_| Ok(()),
                begin: |_, _| {},
                poll: |_| match GATE.load(Ordering::Relaxed) {
                    0 => Gate::Open,
                    1 => Gate::Waiting,
                    _ => Gate::Cancelled,
                },
            });
        app.update();
        let quit = |app: &mut App| {
            app.world_mut()
                .write_message(CloseRequested { window: None });
            for _ in 0..4 {
                app.update();
            }
        };

        // Held: exiting, but not exited.
        GATE.store(1, Ordering::Relaxed);
        quit(&mut app);
        assert!(app.world().bedtime().is_exiting());
        assert!(!exited(&app));

        // Called off: the session goes on, and can be quit again.
        GATE.store(2, Ordering::Relaxed);
        app.update();
        assert!(!app.world().bedtime().is_exiting());
        assert!(!exited(&app));

        GATE.store(0, Ordering::Relaxed);
        quit(&mut app);
        assert!(exited(&app));
    }
}
