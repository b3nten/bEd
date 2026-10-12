//! Explicit input steps advance with UI frames, rather than wall-clock sleeps.
use crate::{DesktopContext, DesktopFrameId, DesktopFrameInfo, DesktopGpu, DesktopInputEvents};
use bevy::prelude::*;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex},
};
use winit::{
    event::MouseButton,
    keyboard::{Key, KeyCode},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DesktopModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_key: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub enum DesktopInjectedEvent {
    PointerMoved([f32; 2]),
    PointerButton {
        button: MouseButton,
        pressed: bool,
    },
    /// Physical/logical key state only. Use `Text` for characters exactly once.
    Key {
        physical: KeyCode,
        logical: Key,
        pressed: bool,
    },
    Modifiers(DesktopModifiers),
    Text(String),
    /// Horizontal/vertical scroll in lines, matching native line-wheel input.
    Scroll([f32; 2]),
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesktopInputStep {
    pub events: Vec<DesktopInjectedEvent>,
}
impl From<Vec<DesktopInjectedEvent>> for DesktopInputStep {
    fn from(events: Vec<DesktopInjectedEvent>) -> Self {
        Self { events }
    }
}
#[derive(Clone, Debug)]
pub struct DesktopInjectedInputEvent {
    pub event: DesktopInjectedEvent,
    pub consumed: bool,
    pub gameplay_consumed: bool,
}
/// Hosts can intercept injected semantic events in `InterceptInput`, just like
/// native events. Logical pointer coordinates use the current window origin.
#[derive(Resource, Default)]
pub struct DesktopInjectedInputEvents(pub Vec<DesktopInjectedInputEvent>);
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DesktopInputId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopInputStatus {
    Pending,
    Complete,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DesktopInputError {
    InvalidEvent,
    FocusLost,
    Cancelled,
    DesktopStopped,
}
impl std::fmt::Display for DesktopInputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DesktopInputError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopInputReceipt {
    pub last_ui_frame: DesktopFrameId,
    pub steps: u32,
}
struct Sequence {
    steps: Vec<DesktopInputStep>,
    next: usize,
    prepared_after: Option<DesktopFrameId>,
    cleaning_up: bool,
    cancel: bool,
    failure: Option<DesktopInputError>,
    result: Option<Result<DesktopInputReceipt, DesktopInputError>>,
}
#[derive(Default)]
struct InputState {
    next: u64,
    sequences: BTreeMap<DesktopInputId, Sequence>,
}
#[derive(Resource, Clone, Default)]
pub(crate) struct InputQueue(Arc<Mutex<InputState>>);
#[derive(Resource, Clone, Default)]
pub struct DesktopInputRequests {
    queue: InputQueue,
}
#[derive(Resource, Clone, Default)]
pub struct DesktopInputResults {
    queue: InputQueue,
}
impl DesktopInputRequests {
    /// Each step occupies one produced UI frame. Held inputs are released in a
    /// final cleanup frame before the receipt completes. Text never comes from keys.
    pub fn submit(&mut self, steps: Vec<DesktopInputStep>) -> DesktopInputId {
        let mut state = self.queue.0.lock().expect("input queue poisoned");
        state.next += 1;
        let id = DesktopInputId(state.next);
        state.sequences.insert(
            id,
            Sequence {
                steps,
                next: 0,
                prepared_after: None,
                cleaning_up: false,
                cancel: false,
                failure: None,
                result: None,
            },
        );
        id
    }
    pub fn cancel(&mut self, id: DesktopInputId) {
        if let Some(sequence) = self
            .queue
            .0
            .lock()
            .expect("input queue poisoned")
            .sequences
            .get_mut(&id)
        {
            sequence.cancel = true;
        }
    }
}
impl DesktopInputResults {
    pub fn status(&self, id: DesktopInputId) -> Option<DesktopInputStatus> {
        self.queue
            .0
            .lock()
            .expect("input queue poisoned")
            .sequences
            .get(&id)
            .map(|sequence| {
                if sequence.result.is_some() {
                    DesktopInputStatus::Complete
                } else {
                    DesktopInputStatus::Pending
                }
            })
    }
    pub fn take(
        &mut self,
        id: DesktopInputId,
    ) -> Option<Result<DesktopInputReceipt, DesktopInputError>> {
        let mut state = self.queue.0.lock().expect("input queue poisoned");
        state.sequences.get(&id)?.result.as_ref()?;
        state.sequences.remove(&id)?.result
    }
}
#[derive(Resource, Default)]
pub(crate) struct InjectedState {
    keys: HashMap<KeyCode, Key>,
    buttons: HashSet<MouseButton>,
    modifiers: DesktopModifiers,
}
impl InputQueue {
    pub(crate) fn install(&self, world: &mut World) {
        world.insert_resource(DesktopInputRequests {
            queue: self.clone(),
        });
        world.insert_resource(DesktopInputResults {
            queue: self.clone(),
        });
        world.init_resource::<DesktopInjectedInputEvents>();
        world.init_resource::<InjectedState>();
    }
    pub(crate) fn stop(&self) {
        for sequence in self
            .0
            .lock()
            .expect("input queue poisoned")
            .sequences
            .values_mut()
        {
            if sequence.result.is_none() {
                sequence.result = Some(Err(DesktopInputError::DesktopStopped));
            }
        }
    }
}
impl InjectedState {
    fn release(&self) -> Vec<DesktopInjectedEvent> {
        let mut events: Vec<_> = self
            .keys
            .iter()
            .map(|(&physical, logical)| DesktopInjectedEvent::Key {
                physical,
                logical: logical.clone(),
                pressed: false,
            })
            .collect();
        events.extend(
            self.buttons
                .iter()
                .map(|&button| DesktopInjectedEvent::PointerButton {
                    button,
                    pressed: false,
                }),
        );
        events.push(DesktopInjectedEvent::Modifiers(DesktopModifiers::default()));
        events
    }
    pub(crate) fn observe(&mut self, event: &DesktopInjectedEvent) {
        match event {
            DesktopInjectedEvent::Key {
                physical,
                logical,
                pressed,
            } => {
                if *pressed {
                    self.keys.insert(*physical, logical.clone());
                } else {
                    self.keys.remove(physical);
                }
            }
            DesktopInjectedEvent::PointerButton { button, pressed } => {
                if *pressed {
                    self.buttons.insert(*button);
                } else {
                    self.buttons.remove(button);
                }
            }
            DesktopInjectedEvent::Modifiers(modifiers) => self.modifiers = *modifiers,
            _ => {}
        }
    }
}

pub(crate) fn prepare(world: &mut World) {
    world.resource_mut::<DesktopInjectedInputEvents>().0.clear();
    let queue = world.resource::<InputQueue>().clone();
    let mut state = queue.0.lock().expect("input queue poisoned");
    let Some(sequence) = state
        .sequences
        .values_mut()
        .find(|sequence| sequence.result.is_none())
    else {
        return;
    };
    let lost_focus = world
        .resource::<DesktopInputEvents>()
        .0
        .iter()
        .any(|event| matches!(event.event.event, winit::event::WindowEvent::Focused(false)));
    if lost_focus {
        sequence.failure = Some(DesktopInputError::FocusLost);
    }
    if sequence.cancel {
        sequence.failure = Some(DesktopInputError::Cancelled);
    }
    if !world.contains_resource::<DesktopGpu>() || sequence.prepared_after.is_some() {
        return;
    }
    if sequence.next == 0
        && sequence
            .steps
            .iter()
            .flat_map(|step| &step.events)
            .any(|event| match event {
                DesktopInjectedEvent::PointerMoved(position)
                | DesktopInjectedEvent::Scroll(position) => {
                    position.iter().any(|value| !value.is_finite())
                }
                DesktopInjectedEvent::Text(text) => text.chars().any(char::is_control),
                _ => false,
            })
    {
        sequence.failure = Some(DesktopInputError::InvalidEvent);
    }
    let events = if sequence.failure.is_some() || sequence.next >= sequence.steps.len() {
        sequence.cleaning_up = true;
        world.resource::<InjectedState>().release()
    } else {
        sequence.steps[sequence.next].events.clone()
    };
    sequence.prepared_after = Some(world.resource::<DesktopFrameInfo>().frame);
    world.resource_mut::<DesktopInjectedInputEvents>().0 = events
        .into_iter()
        .map(|event| DesktopInjectedInputEvent {
            event,
            consumed: false,
            gameplay_consumed: false,
        })
        .collect();
}

pub(crate) fn receipt(info: Res<DesktopFrameInfo>, queue: Res<InputQueue>) {
    let mut state = queue.0.lock().expect("input queue poisoned");
    let Some(sequence) = state
        .sequences
        .values_mut()
        .find(|sequence| sequence.result.is_none())
    else {
        return;
    };
    if sequence
        .prepared_after
        .is_none_or(|before| info.frame <= before)
    {
        return;
    }
    sequence.prepared_after = None;
    if sequence.cleaning_up {
        sequence.result = Some(sequence.failure.clone().map_or_else(
            || {
                Ok(DesktopInputReceipt {
                    last_ui_frame: info.frame,
                    steps: sequence.next as u32,
                })
            },
            Err,
        ));
    } else {
        sequence.next += 1;
    }
}

/// Feed ImGui without constructing Winit KeyEvent (which has platform-private state).
pub(crate) fn feed(context: &mut DesktopContext, event: &DesktopInjectedEvent) {
    context
        .platform
        .handle_injected(&mut context.context, event);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_requires_a_new_ui_frame_and_cleanup_before_next_sequence() {
        let mut app = App::new();
        let queue = InputQueue::default();
        queue.install(app.world_mut());
        app.insert_resource(queue.clone())
            .init_resource::<DesktopFrameInfo>();
        let first = app
            .world_mut()
            .resource_mut::<DesktopInputRequests>()
            .submit(vec![DesktopInputStep::default()]);
        let second = app
            .world_mut()
            .resource_mut::<DesktopInputRequests>()
            .submit(vec![DesktopInputStep::default()]);
        {
            let mut state = queue.0.lock().unwrap();
            state.sequences.get_mut(&first).unwrap().prepared_after = Some(DesktopFrameId(0));
        }
        app.add_systems(Update, receipt);
        app.update();
        assert_eq!(
            app.world().resource::<DesktopInputResults>().status(first),
            Some(DesktopInputStatus::Pending)
        );
        app.world_mut().resource_mut::<DesktopFrameInfo>().frame = DesktopFrameId(1);
        app.update();
        {
            let state = queue.0.lock().unwrap();
            assert_eq!(state.sequences[&first].next, 1);
            assert_eq!(state.sequences[&second].next, 0);
        }
        {
            let mut state = queue.0.lock().unwrap();
            let sequence = state.sequences.get_mut(&first).unwrap();
            sequence.cleaning_up = true;
            sequence.prepared_after = Some(DesktopFrameId(1));
        }
        app.world_mut().resource_mut::<DesktopFrameInfo>().frame = DesktopFrameId(2);
        app.update();
        assert_eq!(
            app.world_mut()
                .resource_mut::<DesktopInputResults>()
                .take(first)
                .unwrap()
                .unwrap(),
            DesktopInputReceipt {
                last_ui_frame: DesktopFrameId(2),
                steps: 1
            }
        );
        assert_eq!(
            app.world().resource::<DesktopInputResults>().status(second),
            Some(DesktopInputStatus::Pending)
        );
    }
}
