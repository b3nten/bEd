//! The system's file pickers (`native` feature): asking for a folder, a file
//! to open, or where to save. A picker doesn't block: the answer comes to a
//! closure, some frames later. One is open at a time: asking again answers
//! the one before with nothing, as dismissing it does.
use crate::{Bedtime, BedtimeRef};
use bevy::prelude::*;
use std::{future::Future, path::PathBuf, pin::Pin};

/// What a picker asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// A folder that exists.
    Folder,
    /// A file that exists.
    File,
    /// Where to save a file: `name` is what it's called unless changed.
    SaveAs { name: String },
}

/// A picker to show: `world.bedtime_mut().pick_path(picker, then)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathPicker {
    pub title: String,
    pub pick: Pick,
    /// The folder it opens in; the system's choice without one.
    pub directory: Option<PathBuf>,
}

impl PathPicker {
    pub fn new(title: impl Into<String>, pick: Pick) -> Self {
        Self {
            title: title.into(),
            pick,
            directory: None,
        }
    }

    pub fn in_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.directory = Some(directory.into());
        self
    }
}

type Answer = Box<dyn FnOnce(&mut World, Option<PathBuf>)>;
type Picking = Pin<Box<dyn Future<Output = Option<rfd::FileHandle>>>>;

/// The picker that's open, and who to tell. The system owns the dialog, so
/// its future is polled on the main thread; nothing blocks while it's open.
#[derive(Default)]
struct Open(Option<(Picking, Answer)>);

pub(crate) fn install(app: &mut App) {
    app.init_non_send::<Open>().add_systems(Update, poll);
}

impl BedtimeRef {
    /// Whether a file picker is open.
    pub fn is_picking(&self) -> bool {
        self.world
            .get_non_send::<Open>()
            .is_some_and(|open| open.0.is_some())
    }
}

impl Bedtime {
    /// Show the system's file picker; `then` gets the path chosen, or
    /// nothing if it was dismissed (or another picker took its place).
    pub fn pick_path(
        &mut self,
        picker: PathPicker,
        then: impl FnOnce(&mut World, Option<PathBuf>) + 'static,
    ) {
        let world = &mut self.world;
        let mut dialog = rfd::AsyncFileDialog::new().set_title(&picker.title);
        if let Some(directory) = &picker.directory {
            dialog = dialog.set_directory(directory);
        }
        let picking: Picking = match picker.pick {
            Pick::Folder => Box::pin(dialog.pick_folder()),
            Pick::File => Box::pin(dialog.pick_file()),
            Pick::SaveAs { name } => Box::pin(dialog.set_file_name(name).save_file()),
        };
        let previous = world
            .non_send_mut::<Open>()
            .0
            .replace((picking, Box::new(then)));
        if let Some((_, answer)) = previous {
            answer(world, None);
        }
    }
}

fn poll(world: &mut World) {
    let mut open = world.non_send_mut::<Open>();
    let Some((picking, _)) = open.0.as_mut() else {
        return;
    };
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(chosen) = picking.as_mut().poll(&mut context) else {
        return;
    };
    let Some((_, answer)) = open.0.take() else {
        return;
    };
    answer(world, chosen.map(|file| file.path().to_owned()));
}
