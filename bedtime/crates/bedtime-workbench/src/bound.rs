//! Widgets bound to the world: a button that is an action, a switch that is
//! a setting. They show what the world says (the action's label, whether it
//! can run, its keys; the setting's value) and ask for the change
//! themselves, so the panel that adds one writes nothing else.
//!
//! They read the world as it's drawn (`bedtime_ui::draw`): add them in a
//! panel drawn by a system, or inside `draw::with_world`.
use crate::actions::InvocationContext;
use crate::actions::{Action, ActionId, ActionRequest, ActionRequests, ActionsWorldExt};
use bedtime_ui::{Button, IconSource, Size, Variant, draw, egui};
use bevy::prelude::*;
use std::marker::PhantomData;

/// A button that runs action `A`: its label, greyed when it can't run, its
/// keys when it's pointed at.
///
/// ```ignore
/// ui.add(ActionButton::<Save>::new());
/// ui.add(ActionButton::<Show<Settings>>::new().ghost().text("Settings…"));
/// ```
#[must_use = "a button does nothing until it's added to a Ui"]
pub struct ActionButton<A: Action> {
    context: InvocationContext,
    text: Option<String>,
    icon: Option<IconSource>,
    variant: Variant,
    size: Size,
    action: PhantomData<fn() -> A>,
}

impl<A: Action> ActionButton<A> {
    /// For what's focused, as the action's keys are.
    pub fn new() -> Self {
        Self {
            context: InvocationContext::Focused,
            text: None,
            icon: None,
            variant: Variant::default(),
            size: Size::default(),
            action: PhantomData,
        }
    }

    /// For this entity (the panel the button is in), whatever's focused.
    pub fn on(mut self, entity: Entity) -> Self {
        self.context = InvocationContext::Entity(entity);
        self
    }
    /// For no entity: the action's global handler.
    pub fn global(mut self) -> Self {
        self.context = InvocationContext::Global;
        self
    }
    /// Its text, in place of the action's label.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }
    pub fn icon(mut self, icon: impl Into<IconSource>) -> Self {
        self.icon = Some(icon.into());
        self
    }
    pub fn variant(mut self, variant: Variant) -> Self {
        self.variant = variant;
        self
    }
    pub fn primary(self) -> Self {
        self.variant(Variant::Primary)
    }
    pub fn ghost(self) -> Self {
        self.variant(Variant::Ghost)
    }
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }
}

impl<A: Action> Default for ActionButton<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Action> egui::Widget for ActionButton<A> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let id = ActionId::of::<A>();
        let actions = draw::world(ui).actions();
        let available = actions.available(&id, self.context);
        let keys = actions
            .shortcut_for(&id, self.context)
            .map(|chord| chord.label());
        let mut button = Button::new(self.text.unwrap_or_else(|| A::LABEL.into()))
            .variant(self.variant)
            .size(self.size);
        if let Some(icon) = self.icon {
            button = button.icon(icon);
        }
        let mut response = ui.add_enabled(available, button);
        if let Some(keys) = keys {
            response = response.on_hover_text(keys);
        }
        if response.clicked() {
            let context = self.context;
            draw::queue(ui, move |world: &mut World| {
                world
                    .resource_mut::<ActionRequests>()
                    .submit(ActionRequest {
                        action: id,
                        context,
                        source: None,
                    });
            });
        }
        response
    }
}

#[cfg(feature = "persistence")]
pub use setting::SettingToggle;

#[cfg(feature = "persistence")]
mod setting {
    use crate::BedtimeWorldExt as _;
    use crate::settings::Setting;
    use bedtime_ui::{Checkbox, Toggle, draw, egui};
    use bevy::prelude::*;
    use std::marker::PhantomData;

    /// A switch that is setting `S`: it shows the setting's value and, when
    /// flipped, sets it, in the layer the value came from (the lowest layer
    /// when it's still the default).
    ///
    /// ```ignore
    /// ui.add(SettingToggle::<Autosave>::new());
    /// ui.add(SettingToggle::<WordWrap>::new().checkbox().label("Wrap long lines"));
    /// ```
    #[must_use = "a toggle does nothing until it's added to a Ui"]
    pub struct SettingToggle<S: Setting<Value = bool>> {
        label: Option<String>,
        checkbox: bool,
        setting: PhantomData<fn() -> S>,
    }

    impl<S: Setting<Value = bool>> SettingToggle<S> {
        /// A switch, with the setting's own label.
        pub fn new() -> Self {
            Self {
                label: None,
                checkbox: false,
                setting: PhantomData,
            }
        }
        /// Its label, in place of the setting's.
        pub fn label(mut self, label: impl Into<String>) -> Self {
            self.label = Some(label.into());
            self
        }
        /// As a checkbox, for a form.
        pub fn checkbox(mut self) -> Self {
            self.checkbox = true;
            self
        }
    }

    impl<S: Setting<Value = bool>> Default for SettingToggle<S> {
        fn default() -> Self {
            Self::new()
        }
    }

    impl<S: Setting<Value = bool>> egui::Widget for SettingToggle<S> {
        fn ui(self, ui: &mut egui::Ui) -> egui::Response {
            let settings = draw::world(ui).bedtime().settings();
            let mut value = settings.get::<S>();
            let layer = settings
                .source::<S>()
                .or_else(|| settings.layers().find(|layer| S::allowed_in(*layer)));
            #[cfg(feature = "ui")]
            let label = self.label.unwrap_or_else(|| S::label().into_owned());
            #[cfg(not(feature = "ui"))]
            let label = self.label.unwrap_or_default();
            let response = if self.checkbox {
                ui.add_enabled(layer.is_some(), Checkbox::new(&mut value, label))
            } else {
                ui.add_enabled(layer.is_some(), Toggle::new(&mut value, label))
            };
            if response.changed()
                && let Some(layer) = layer
            {
                draw::queue(ui, move |world: &mut World| {
                    let set = world.bedtime_mut().settings_mut().set::<S>(layer, value);
                    if let Err(error) = set {
                        warn!("A setting couldn't be changed: {error}");
                    }
                });
            }
            response
        }
    }
}
