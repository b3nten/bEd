//! Drawing settings: the rows of settings declared in a section, and the
//! pieces of a settings panel (layer tabs, sections, rows).
//!
//! A panel is usually [`layer_tabs`] then [`sections`]. An app that wants
//! something else (search, another layout) reads [`SettingsUi`] and draws the
//! rows itself; [`row`], [`columns`] and the markers are the parts.
use crate::{Setting, Settings, SettingsError, SettingsWorldExt};
use bedtime_persistence::StoreId;
use bedtime_ui::{BuiltinIcon, UiContext, egui};
use bevy::prelude::*;
use std::{ops::RangeInclusive, sync::Arc};

/// The width of the label column, so every section's controls line up.
pub const LABEL_WIDTH: f32 = 210.0;
/// The width of combo boxes, sliders and text fields.
pub const CONTROL_WIDTH: f32 = 240.0;

/// Draws a whole row for the layer being edited, for settings that need more
/// than an editor (extra buttons, a note).
pub type RowFn = fn(&mut egui::Ui, &mut World, &UiContext<'_>, StoreId);

type EditFn<V> = dyn Fn(&mut egui::Ui, &UiContext<'_>, &mut V) -> bool + Send + Sync;
type DrawRow = Arc<dyn Fn(&mut egui::Ui, &mut World, &UiContext<'_>, StoreId) + Send + Sync>;

/// How a setting is edited.
pub enum SettingUi<V> {
    /// A control for the value, in the standard row. Returns whether it changed
    /// the value.
    Editor(Box<EditFn<V>>),
    /// A row of its own.
    Row(RowFn),
}

impl<V> SettingUi<V> {
    pub fn editor(
        edit: impl Fn(&mut egui::Ui, &UiContext<'_>, &mut V) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self::Editor(Box::new(edit))
    }
}

/// The editors `ui:` entries name.
pub mod editors {
    use super::*;
    use bevy::reflect::{
        FromReflect, PartialReflect, ReflectRef, TypeInfo, Typed,
        enums::{DynamicEnum, DynamicVariant},
    };

    pub fn toggle() -> SettingUi<bool> {
        SettingUi::editor(|ui, context, value| context.frame(ui).toggle(value, "").changed())
    }

    pub fn text() -> SettingUi<String> {
        SettingUi::editor(|ui, _, value| {
            ui.add(egui::TextEdit::singleline(value).desired_width(CONTROL_WIDTH))
                .changed()
        })
    }

    pub fn drag<N: egui::emath::Numeric + Send + Sync + 'static>() -> SettingUi<N> {
        SettingUi::editor(|ui, _, value| ui.add(egui::DragValue::new(value)).changed())
    }

    /// A slider over `range`, showing its value to two decimals by default.
    pub fn slider<N: egui::emath::Numeric>(range: RangeInclusive<N>) -> Slider<N> {
        Slider {
            range,
            suffix: "",
            decimals: 2,
        }
    }

    /// A combo box of a fieldless enum's variants (it must derive `Reflect`).
    pub fn choice<V: Typed + FromReflect + PartialReflect + Send + Sync + 'static>() -> SettingUi<V>
    {
        SettingUi::editor(|ui, _, value: &mut V| {
            let TypeInfo::Enum(info) = V::type_info() else {
                return false;
            };
            let ReflectRef::Enum(current) = value.reflect_ref() else {
                return false;
            };
            let current = current.variant_name().to_owned();
            let mut chosen = current.clone();
            combo(ui, std::any::type_name::<V>(), words(&current), |ui| {
                for name in info.variant_names() {
                    ui.selectable_value(&mut chosen, (*name).to_owned(), words(name));
                }
            });
            if chosen == current {
                return false;
            }
            match V::from_reflect(&DynamicEnum::new(chosen, DynamicVariant::Unit)) {
                Some(new) => {
                    *value = new;
                    true
                }
                None => false,
            }
        })
    }

    /// A row of its own.
    pub fn custom<V>(row: RowFn) -> SettingUi<V> {
        SettingUi::Row(row)
    }

    /// See [`slider`].
    pub struct Slider<N> {
        range: RangeInclusive<N>,
        suffix: &'static str,
        decimals: usize,
    }

    impl<N> Slider<N> {
        /// Text after the value (" pt").
        pub fn suffix(self, suffix: &'static str) -> Self {
            Self { suffix, ..self }
        }
        /// How many decimals the value is shown with.
        pub fn decimals(self, decimals: usize) -> Self {
            Self { decimals, ..self }
        }
    }

    impl<N: egui::emath::Numeric + Send + Sync + 'static> From<Slider<N>> for SettingUi<N> {
        fn from(slider: Slider<N>) -> Self {
            SettingUi::editor(move |ui, _, value| {
                super::slider(
                    ui,
                    value,
                    slider.range.clone(),
                    slider.suffix,
                    slider.decimals,
                )
            })
        }
    }
}

/// Picks an editor from a setting's value type, for `settings!`: a toggle for
/// `bool`, a text field for `String`, a drag value for numbers, a combo box for
/// a fieldless enum that derives `Reflect`, else nothing.
#[doc(hidden)]
pub mod infer {
    use super::{SettingUi, editors};
    use bevy::reflect::{FromReflect, PartialReflect, TypeInfo, Typed};
    use std::marker::PhantomData;

    pub struct Infer<T>(PhantomData<T>);
    impl<T> Infer<T> {
        #[allow(clippy::new_without_default)]
        pub fn new() -> Self {
            Self(PhantomData)
        }
    }

    pub trait InferType<T> {
        fn infer(&self) -> Option<SettingUi<T>>;
    }
    pub trait InferEnum<T> {
        fn infer(&self) -> Option<SettingUi<T>>;
    }
    pub trait InferNone<T> {
        fn infer(&self) -> Option<SettingUi<T>>;
    }

    impl InferType<bool> for &&Infer<bool> {
        fn infer(&self) -> Option<SettingUi<bool>> {
            Some(editors::toggle())
        }
    }
    impl InferType<String> for &&Infer<String> {
        fn infer(&self) -> Option<SettingUi<String>> {
            Some(editors::text())
        }
    }
    macro_rules! numbers {
        ($($n:ty),*) => {$(
            impl InferType<$n> for &&Infer<$n> {
                fn infer(&self) -> Option<SettingUi<$n>> {
                    Some(editors::drag())
                }
            }
        )*};
    }
    numbers!(f32, f64, i8, i16, i32, i64, u8, u16, u32, u64, usize, isize);

    impl<T: Typed + FromReflect + PartialReflect + Send + Sync + 'static> InferEnum<T> for &Infer<T> {
        fn infer(&self) -> Option<SettingUi<T>> {
            let TypeInfo::Enum(info) = T::type_info() else {
                return None;
            };
            info.iter()
                .all(|variant| matches!(variant, bevy::reflect::enums::VariantInfo::Unit(_)))
                .then(editors::choice)
        }
    }

    impl<T> InferNone<T> for Infer<T> {
        fn infer(&self) -> Option<SettingUi<T>> {
            None
        }
    }
}

/// One setting row the panel can draw.
pub struct SettingRow {
    pub key: String,
    pub section: &'static str,
    pub label: String,
    pub description: Option<&'static str>,
    allowed_in: fn(StoreId) -> bool,
    visible: fn(&World, StoreId) -> bool,
    draw: DrawRow,
}

impl SettingRow {
    /// Whether it's shown while editing `layer`.
    pub fn shown(&self, world: &World, layer: StoreId) -> bool {
        (self.allowed_in)(layer) && (self.visible)(world, layer)
    }

    /// Draw its row for `layer`.
    pub fn draw(
        &self,
        ui: &mut egui::Ui,
        world: &mut World,
        context: &UiContext<'_>,
        layer: StoreId,
    ) {
        (self.draw.clone())(ui, world, context, layer)
    }
}

/// The settings declared in a section, in the order they were registered.
#[derive(Resource, Default)]
pub struct SettingsUi {
    rows: Vec<Arc<SettingRow>>,
    /// Section order: the app's, then first registration.
    order: Vec<&'static str>,
}

impl SettingsUi {
    /// The sections in order, each with its rows.
    pub fn sections(&self) -> Vec<(&'static str, Vec<Arc<SettingRow>>)> {
        let mut sections: Vec<&'static str> = self.order.clone();
        for row in &self.rows {
            if !sections.contains(&row.section) {
                sections.push(row.section);
            }
        }
        sections
            .into_iter()
            .map(|section| {
                let rows = self
                    .rows
                    .iter()
                    .filter(|row| row.section == section)
                    .cloned()
                    .collect();
                (section, rows)
            })
            .filter(|(_, rows): &(_, Vec<_>)| !rows.is_empty())
            .collect()
    }
}

pub trait SettingsUiAppExt {
    /// The order of the settings panel's sections. Sections not listed follow,
    /// in the order their first setting was registered.
    fn settings_sections(&mut self, order: impl IntoIterator<Item = &'static str>) -> &mut Self;
}

impl SettingsUiAppExt for App {
    fn settings_sections(&mut self, order: impl IntoIterator<Item = &'static str>) -> &mut Self {
        self.world_mut().get_resource_or_init::<SettingsUi>().order = order.into_iter().collect();
        self
    }
}

pub(crate) fn register<S: Setting>(app: &mut App) {
    let Some(section) = S::SECTION else {
        return;
    };
    let draw: DrawRow =
        match S::ui().or_else(S::inferred_ui) {
            Some(SettingUi::Row(row)) => Arc::new(row),
            Some(SettingUi::Editor(edit)) => Arc::new(
                move |ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, layer| {
                    self::row::<S>(ui, world, context, layer, None, |ui, value| {
                        edit(ui, context, value)
                    })
                },
            ),
            None => Arc::new(
                |ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, layer| {
                    self::row::<S>(ui, world, context, layer, None, |ui, _| {
                        ui.weak("Edit in the settings file");
                        false
                    })
                },
            ),
        };
    app.world_mut()
        .get_resource_or_init::<SettingsUi>()
        .rows
        .push(Arc::new(SettingRow {
            key: S::key().into_owned(),
            section,
            label: S::label().into_owned(),
            description: S::description(),
            allowed_in: S::allowed_in,
            visible: S::visible,
            draw,
        }));
}

/// Tabs for choosing which layer to edit, shown when there's more than one.
/// `text` gives each tab's text from its layer and label. The choice is kept
/// between frames.
pub fn layer_tabs(
    ui: &mut egui::Ui,
    world: &World,
    text: impl Fn(StoreId, &str) -> String,
) -> Option<StoreId> {
    let settings = world.settings();
    let layers: Vec<_> = settings.layers().collect();
    let first = *layers.first()?;
    let id = ui.id().with("settings-layer");
    let mut chosen = ui
        .data(|data| data.get_temp::<StoreId>(id))
        .filter(|layer| layers.contains(layer))
        .unwrap_or(first);
    if layers.len() > 1 {
        // Wrap rather than overflow: an overflowing row would widen everything
        // after it, and the panel would stop scrolling sideways.
        ui.horizontal_wrapped(|ui| {
            for &layer in &layers {
                let label = settings.label(layer).unwrap_or_default();
                ui.selectable_value(&mut chosen, layer, text(layer, label));
            }
        });
    }
    ui.data_mut(|data| data.insert_temp(id, chosen));
    Some(chosen)
}

/// Every section with a row shown for `layer`, in order.
pub fn sections(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, layer: StoreId) {
    let Some(registry) = world.get_resource::<SettingsUi>() else {
        return;
    };
    let sections = registry.sections();
    // Leaves room for the value box beside each slider.
    ui.spacing_mut().slider_width = CONTROL_WIDTH - 76.0;
    for (section, rows) in sections {
        let shown: Vec<_> = rows.iter().filter(|row| row.shown(world, layer)).collect();
        if shown.is_empty() {
            continue;
        }
        heading(ui, section);
        for row in shown {
            row.draw(ui, world, context, layer);
        }
    }
}

/// A section's heading.
pub fn heading(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.heading(title);
    ui.add_space(2.0);
}

/// One row: a label (then `marker`, such as an override warning), a control
/// and then `state`, each in its own column at a fixed height so rows line up
/// across sections.
pub fn columns<R>(
    ui: &mut egui::Ui,
    label: &str,
    description: Option<&str>,
    marker: impl FnOnce(&mut egui::Ui),
    control: impl FnOnce(&mut egui::Ui) -> R,
    state: impl FnOnce(&mut egui::Ui),
) -> R {
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    let row_height = ui.spacing().interact_size.y;
    let row = egui::vec2(ui.available_width(), row_height);
    ui.allocate_ui_with_layout(row, layout, |ui| {
        ui.set_min_height(row_height);
        ui.allocate_ui_with_layout(egui::vec2(LABEL_WIDTH, row_height), layout, |ui| {
            ui.set_min_size(egui::vec2(LABEL_WIDTH, row_height));
            ui.spacing_mut().item_spacing.x = 6.0;
            // Leave room for the marker after a long label.
            ui.scope(|ui| {
                ui.set_max_width(LABEL_WIDTH - 28.0);
                let label = ui.add(egui::Label::new(label).truncate());
                if let Some(description) = description {
                    label.on_hover_text(description);
                }
            });
            marker(ui);
        });
        let inner = ui
            .allocate_ui_with_layout(egui::vec2(CONTROL_WIDTH, row_height), layout, |ui| {
                ui.set_min_size(egui::vec2(CONTROL_WIDTH, row_height));
                control(ui)
            })
            .inner;
        // As wide as it needs: past the panel's edge it scrolls sideways.
        state(ui);
        inner
    })
    .inner
}

/// Setting `S`'s row for `layer`, showing the layer's own value (what it is
/// without the layers above). `edit` draws the control and returns whether it
/// changed the value; `note` follows the row (why a value isn't used, say).
pub fn row<S: Setting>(
    ui: &mut egui::Ui,
    world: &mut World,
    context: &UiContext<'_>,
    layer: StoreId,
    note: Option<String>,
    edit: impl FnOnce(&mut egui::Ui, &mut S::Value) -> bool,
) {
    let settings = world.settings();
    let mut value = settings.get_at::<S>(layer);
    let set_here = settings.get_in::<S>(layer).is_some();
    let marks = Marks::new(settings, layer, settings.overridden_above::<S>(layer));
    let label = S::label();
    let mut reset = false;
    let changed = columns(
        ui,
        &label,
        S::description(),
        |ui| marks.overridden(ui, context),
        |ui| edit(ui, &mut value),
        |ui| {
            reset = set_here && marks.reset(ui, context);
            if let Some(note) = note {
                ui.weak(note);
            }
        },
    );
    if changed && let Err(error) = world.settings_mut().set::<S>(layer, value) {
        world.write_message(SettingsError(format!("Couldn't change {label}: {error}")));
    }
    if reset && let Err(error) = world.settings_mut().clear::<S>(layer) {
        world.write_message(SettingsError(format!("Couldn't reset {label}: {error}")));
    }
}

/// A row's markers for one layer: the warning after its label when a layer
/// above overrides it, and the button that clears the layer's value. Made
/// before drawing, so drawing the row can change the world.
pub struct Marks {
    above: Option<String>,
    below: Option<String>,
}

impl Marks {
    /// `above` is the layer whose value is used instead of `layer`'s, if any
    /// (`Settings::overridden_above`).
    pub fn new(settings: &Settings, layer: StoreId, above: Option<StoreId>) -> Self {
        let label = |store| settings.label(store).map(str::to_owned);
        Self {
            above: above.map(|above| label(above).unwrap_or_else(|| "higher".into())),
            below: settings.below(layer).and_then(label),
        }
    }

    /// After a label: a warning icon when a layer above sets this differently,
    /// so changes here won't show until it's cleared there.
    pub fn overridden(&self, ui: &mut egui::Ui, context: &UiContext<'_>) {
        let Some(above) = &self.above else {
            return;
        };
        let hint = format!(
            "Set differently in the {} settings, which are used instead. \
             Clear it on the {above} tab to use this one.",
            above.to_lowercase()
        );
        let tint = bedtime_ui::theme::color(context.theme.palette.warning);
        let mut frame = context.frame(ui);
        let side = context.metrics.font_size;
        let icon = frame.icon(BuiltinIcon::CircleAlert, side, tint);
        icon.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "Overridden"));
        frame.tooltip(&icon, hint);
    }

    /// A button that clears the layer's value (back to the default, or to the
    /// layer below's). Show it only when the layer holds a value. Returns
    /// whether it was clicked.
    pub fn reset(&self, ui: &mut egui::Ui, context: &UiContext<'_>) -> bool {
        let hint = match &self.below {
            Some(below) => format!("Use the {} value", below.to_lowercase()),
            None => "Reset to default".to_owned(),
        };
        let mut frame = context.frame(ui);
        let response = frame.icon_button_sized(BuiltinIcon::RotateCcw, 14.0);
        frame.tooltip(&response, hint);
        response.clicked()
    }
}

/// A slider with its value shown to `decimals` places. Only the text is
/// rounded: egui writes a slider's value back every frame, and rounding it
/// there would change a setting just by showing it.
pub fn slider<N: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut N,
    range: RangeInclusive<N>,
    suffix: &str,
    decimals: usize,
) -> bool {
    let slider = egui::Slider::new(value, range)
        .suffix(suffix)
        .custom_formatter(move |value, _| format!("{value:.decimals$}"));
    ui.add(slider).changed()
}

/// A combo box filling the control column.
pub fn combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    selected: impl Into<egui::WidgetText>,
    options: impl FnOnce(&mut egui::Ui),
) {
    egui::ComboBox::from_id_salt(id)
        .width(CONTROL_WIDTH)
        .selected_text(selected)
        .show_ui(ui, options);
}

/// A type name in words: `FontSize` is "Font size".
pub fn words(name: &str) -> String {
    let mut words = String::new();
    for (index, c) in name.char_indices() {
        if c.is_uppercase() && index > 0 {
            words.push(' ');
            words.extend(c.to_lowercase());
        } else {
            words.push(c);
        }
    }
    words
}
