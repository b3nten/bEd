//! Short messages that need no answer: a setting that couldn't be saved, an
//! action that failed. The application shows them (in a status bar, say) and
//! each is logged. Anything that could lose work asks with a dialog instead.
use crate::Workbench;
use bevy::prelude::*;
use std::{collections::VecDeque, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub level: NotificationLevel,
    pub text: String,
    /// When it was last sent. A repeat of the latest notification refreshes
    /// this instead of adding another.
    pub at: Instant,
}

impl Notification {
    pub fn new(level: NotificationLevel, text: impl Into<String>) -> Self {
        Self {
            level,
            text: text.into(),
            at: Instant::now(),
        }
    }
    pub fn info(text: impl Into<String>) -> Self {
        Self::new(NotificationLevel::Info, text)
    }
    pub fn warning(text: impl Into<String>) -> Self {
        Self::new(NotificationLevel::Warning, text)
    }
    pub fn error(text: impl Into<String>) -> Self {
        Self::new(NotificationLevel::Error, text)
    }
}

/// Recent notifications, oldest first.
#[derive(Resource, Default)]
pub struct Notifications(VecDeque<Notification>);

impl Notifications {
    /// How many are kept.
    pub const CAPACITY: usize = 50;

    pub fn latest(&self) -> Option<&Notification> {
        self.0.back()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Notification> {
        self.0.iter()
    }

    fn push(&mut self, notification: Notification) -> bool {
        if let Some(latest) = self.0.back_mut()
            && latest.level == notification.level
            && latest.text == notification.text
        {
            latest.at = notification.at;
            return false;
        }
        if self.0.len() == Self::CAPACITY {
            self.0.pop_front();
        }
        self.0.push_back(notification);
        true
    }
}

impl Workbench {
    /// Tell the user something that needs no answer.
    pub fn notify(&mut self, notification: Notification) {
        let (level, text) = (notification.level, notification.text.clone());
        if self
            .world
            .get_resource_or_init::<Notifications>()
            .push(notification)
        {
            match level {
                NotificationLevel::Info => info!("{text}"),
                NotificationLevel::Warning => warn!("{text}"),
                NotificationLevel::Error => error!("{text}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{WorkbenchCorePlugins, WorkbenchWorldExt};

    #[test]
    fn repeats_refresh_the_latest_and_old_ones_fall_off() {
        let mut app = App::new();
        app.add_plugins(WorkbenchCorePlugins);
        let workbench = app.world_mut().workbench_mut();
        workbench.notify(Notification::error("disk full"));
        let first = workbench
            .world()
            .resource::<Notifications>()
            .latest()
            .unwrap()
            .at;
        workbench.notify(Notification::error("disk full"));
        let notifications = workbench.world().resource::<Notifications>();
        assert_eq!(notifications.iter().count(), 1);
        assert!(notifications.latest().unwrap().at >= first);
        for n in 0..Notifications::CAPACITY {
            workbench.notify(Notification::info(format!("{n}")));
        }
        let notifications = workbench.world().resource::<Notifications>();
        assert_eq!(notifications.iter().count(), Notifications::CAPACITY);
        assert_eq!(notifications.iter().next().unwrap().text, "0");
    }
}
