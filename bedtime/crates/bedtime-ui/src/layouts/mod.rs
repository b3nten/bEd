//! Layouts: areas tiling a window, each holding panels as tabs, that users
//! split, join, resize and rearrange by dragging.
//!
//! A `Layout` holds `Area`s (`InLayout`), and an area holds panels (`InArea`)
//! as tabs. ECS owns the tiling: geometry snapshots are temporary
//! calculations, never a second live model, and only this module commits
//! membership and topology changes. `world.layouts()` reads and
//! `world.layouts_mut()` changes them. On a window, `LayoutSurface` draws the
//! active layout under the window's chrome (`Titlebar`); an app can draw its
//! own body around `view::show` instead (a status bar, say).
//!
//! Nothing here knows what a panel holds. Apps add rules through hooks:
//! `check_panel_close` refuses closes, `on_panel_close` takes over closes the
//! user asks for (to ask about unsaved work, say), `PanelModified` marks tabs,
//! and `add_layout_validation` checks more of the world.
mod chrome;
mod dialogs;
mod geometry;
mod membership;
#[cfg(feature = "persistence")]
mod persistence;
mod placement;
mod surface;
#[cfg(feature = "persistence")]
mod templates;
#[cfg(test)]
pub(crate) mod tests;
mod validation;
pub mod view;

pub use bedtime_ui_macros::panel;
pub use chrome::*;
pub use dialogs::{ErrorDialog, ErrorDialogWorldExt, ErrorDialogs, ModalAppExt, ModalFn};
pub use placement::{LayoutFallback, PanelsCommandsExt, PanelsWorldExt, Place, Placement};
pub use surface::{AreaBackground, LayoutCamera, LayoutSurface, SurfaceBody, TabBarVisibility};
#[cfg(feature = "persistence")]
pub use templates::LayoutTemplate;
pub use validation::PanelTabStyle;
pub(crate) use validation::validate_layouts;
pub use view::{
    FileDrops, PanelFactoryPayload, SurfacePresentation, SurfaceShell, Titlebar, TitlebarOwnership,
    panel_drag_source,
};

#[cfg(feature = "persistence")]
use bedtime_persistence::ReflectPersist;
#[cfg(feature = "persistence")]
use bevy::ecs::reflect::ReflectMapEntities;
use bevy::prelude::*;
pub use geometry::EXTENT as LAYOUT_EXTENT;
pub use geometry::Rect as AreaRect;
use std::collections::HashMap;

/// Reading layouts: `world.layouts()`.
#[repr(transparent)]
pub struct LayoutsRef {
    world: World,
}

/// Changing layouts: `world.layouts_mut()`. It reads too. Changes made here
/// are the app's own and skip the user's permissions and close hooks; a user's
/// edit goes through `user_edit`.
#[repr(transparent)]
pub struct Layouts {
    world: World,
}

impl LayoutsRef {
    pub fn world(&self) -> &World {
        &self.world
    }
}

impl Layouts {
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }
}

impl std::ops::Deref for Layouts {
    type Target = LayoutsRef;
    fn deref(&self) -> &LayoutsRef {
        view_of(&self.world)
    }
}

fn view_of(world: &World) -> &LayoutsRef {
    // SAFETY: LayoutsRef is a repr(transparent) wrapper of World.
    unsafe { &*(world as *const World).cast::<LayoutsRef>() }
}

/// `world.layouts()` and `world.layouts_mut()`.
pub trait LayoutsWorldExt {
    fn layouts(&self) -> &LayoutsRef;
    fn layouts_mut(&mut self) -> &mut Layouts;
}

impl LayoutsWorldExt for World {
    fn layouts(&self) -> &LayoutsRef {
        view_of(self)
    }
    fn layouts_mut(&mut self) -> &mut Layouts {
        // SAFETY: Layouts is a repr(transparent) wrapper of World.
        unsafe { &mut *(self as *mut World).cast::<Layouts>() }
    }
}

/// Refuses closing these panels, for direct and user closes alike (input not
/// yet committed, say).
pub type CloseCheck = fn(&World, &[Entity]) -> Result<(), String>;

/// Runs before a user's edit closes `panels` in `layout`. `Ok(true)` takes the
/// close over: the hook applies the edit later (`layouts_mut().apply_edit`),
/// or not at all (the user cancelled). `Err` refuses it.
pub type CloseHook = fn(&mut World, Entity, &LayoutEdit, &[Entity]) -> Result<bool, String>;

#[derive(Resource, Default)]
pub(crate) struct CloseHooks {
    checks: Vec<CloseCheck>,
    hooks: Vec<CloseHook>,
}

/// Whether a panel's tab shows it's modified (unsaved, say). Not modified
/// until the app says.
#[derive(Resource, Clone, Copy)]
pub struct PanelModified(pub fn(&World, Entity) -> bool);

impl Default for PanelModified {
    fn default() -> Self {
        Self(|_, _| false)
    }
}

/// More checks of the world, run with the layout validation (in debug builds
/// every frame, and around persistence).
pub type ExtraValidation = fn(&mut World) -> Result<(), String>;

#[derive(Resource, Default)]
pub(crate) struct Validations(pub(crate) Vec<ExtraValidation>);

pub trait LayoutsAppExt {
    /// Refuse closing panels for which `check` errs.
    fn check_panel_close(&mut self, check: CloseCheck) -> &mut Self;
    /// Let `hook` take over closes the user asks for.
    fn on_panel_close(&mut self, hook: CloseHook) -> &mut Self;
    /// Check more of the world along with the layouts.
    fn add_layout_validation(&mut self, validation: ExtraValidation) -> &mut Self;
}

impl LayoutsAppExt for App {
    fn check_panel_close(&mut self, check: CloseCheck) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<CloseHooks>()
            .checks
            .push(check);
        self
    }
    fn on_panel_close(&mut self, hook: CloseHook) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<CloseHooks>()
            .hooks
            .push(hook);
        self
    }
    fn add_layout_validation(&mut self, validation: ExtraValidation) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<Validations>()
            .0
            .push(validation);
        self
    }
}

/// Stable layout order. Entity allocation and archetype iteration are not order.
#[derive(Resource, Default)]
#[cfg_attr(
    feature = "persistence",
    derive(Reflect, Clone, bevy::ecs::entity::MapEntities)
)]
#[cfg_attr(feature = "persistence", reflect(Resource, MapEntities, Persist))]
pub(crate) struct LayoutOrder(#[cfg_attr(feature = "persistence", entities)] pub Vec<Entity>);
impl LayoutsRef {
    pub fn all(&self) -> &[Entity] {
        let world = &self.world;
        &world.resource::<LayoutOrder>().0
    }

    /// Find the selected layout from its authoritative marker.
    pub fn active(&self) -> Option<Entity> {
        let world = &self.world;
        let mut query = world.try_query_filtered::<Entity, With<ActiveLayout>>()?;
        let mut layouts = query.iter(world);
        let selected = layouts.next();
        assert!(layouts.next().is_none(), "Multiple ActiveLayout entities");
        selected
    }

    /// Bevy removes a relationship target when its last source is detached. An
    /// absent target is the ordinary representation of an empty area's tab list.
    pub fn panels_in_area(&self, area: Entity) -> &[Entity] {
        let world = &self.world;
        world.get::<AreaPanels>(area).map_or(&[], |p| p.entities())
    }
}

/// Selected layout in this World. Zero or one may be selected.
#[derive(Component, Default)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, Persist))]
#[require(Layout)]
pub struct ActiveLayout;

#[derive(Component, Debug)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, Persist))]
#[require(LayoutAreas)]
#[component(on_add = crate::layouts::membership::layout_added, on_remove = crate::layouts::membership::layout_removed)]
pub struct Layout {
    pub name: String,
    #[cfg_attr(feature = "persistence", entities)]
    pub focused_area: Option<Entity>,
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            name: "Layout".into(),
            focused_area: None,
        }
    }
}
impl Layout {
    pub fn focused_area(&self) -> Option<Entity> {
        self.focused_area
    }
}

#[derive(Component, Debug)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, Persist))]
#[require(AreaPresentation, AreaLocation)]
pub struct Area {
    pub rect: AreaRect,
    #[cfg_attr(feature = "persistence", entities)]
    pub selected: Option<Entity>,
}
impl Default for Area {
    fn default() -> Self {
        Self {
            rect: AreaRect {
                min: [0; 2],
                max: [LAYOUT_EXTENT; 2],
            },
            selected: None,
        }
    }
}
impl Area {
    pub fn rect(&self) -> AreaRect {
        self.rect
    }
    pub fn selected_panel(&self) -> Option<Entity> {
        self.selected
    }
}

/// Not persisted: a panel's concrete component supplies it through `#[require]`
/// (title from `Panel::LABEL`), so restoring that component restores this too.
#[derive(Component, Debug, Default)]
#[require(PanelPresentation, PanelLocation)]
pub struct PanelInstance {
    pub title: String,
    pub preview: bool,
    /// Optional ordinary drawing capability, available as soon as composition applies.
    pub draw: Option<PanelDraw>,
    /// The panel's type (its `Panel` component), set by `of`: what
    /// `Placement::Auto` compares to find areas with panels like it.
    pub kind: Option<std::any::TypeId>,
}

pub type PanelDraw = fn(&mut crate::egui::Ui, &mut World, &crate::UiContext<'_>, Entity);

/// Last applied parent, used only to distinguish attachment from reinsertion.
#[derive(Component, Default)]
pub(crate) struct PanelLocation(pub Option<Entity>);
#[derive(Component, Default)]
pub(crate) struct AreaLocation(pub Option<Entity>);

impl PanelInstance {
    /// Local presentation supplied by a concrete panel's required components.
    pub fn of<P: Panel>() -> Self {
        Self {
            title: P::LABEL.into(),
            draw: Some(P::draw),
            kind: Some(std::any::TypeId::of::<P>()),
            ..default()
        }
    }
}

#[derive(Component, Debug)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, Persist))]
#[component(on_insert = crate::layouts::membership::area_inserted, on_discard = crate::layouts::membership::area_replaced, on_remove = crate::layouts::membership::area_removed)]
#[relationship(relationship_target = LayoutAreas)]
pub struct InLayout(#[cfg_attr(feature = "persistence", entities)] pub Entity);

/// Ordered area membership.
#[derive(Component, Default)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, Persist))]
#[relationship_target(relationship = InLayout, linked_spawn)]
pub struct LayoutAreas(#[cfg_attr(feature = "persistence", entities)] Vec<Entity>);

impl LayoutAreas {
    pub fn entities(&self) -> &[Entity] {
        &self.0
    }
}

/// How much bigger than the rest of the app a panel's content is drawn: its
/// text, spacing and controls (not its tab, or the menus and tooltips it
/// opens). No component is 100%. Saved with the panel.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "persistence", derive(Reflect))]
#[cfg_attr(feature = "persistence", reflect(Component, Persist))]
pub struct PanelZoom(pub f32);

impl PanelZoom {
    /// The zoom levels stepped through, as browsers do.
    pub const STEPS: [f32; 13] = [
        0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
    ];

    /// The next step up from `zoom`, if there's one.
    pub fn bigger(zoom: f32) -> Option<f32> {
        Self::STEPS.into_iter().find(|&step| step > zoom + 0.001)
    }

    /// The next step down from `zoom`, if there's one.
    pub fn smaller(zoom: f32) -> Option<f32> {
        Self::STEPS
            .into_iter()
            .rev()
            .find(|&step| step < zoom - 0.001)
    }

    /// `panel`'s zoom.
    pub fn of(world: &World, panel: Entity) -> f32 {
        world.get::<Self>(panel).map_or(1.0, |zoom| zoom.0)
    }

    /// Set `panel`'s zoom (within the steps' range); 100% removes the
    /// component. Returns the zoom it has now.
    pub fn set(world: &mut World, panel: Entity, zoom: f32) -> f32 {
        let zoom = if zoom.is_finite() {
            zoom.clamp(Self::STEPS[0], Self::STEPS[Self::STEPS.len() - 1])
        } else {
            1.0
        };
        let Ok(mut entity) = world.get_entity_mut(panel) else {
            return 1.0;
        };
        entity.insert(ZoomChanged(std::time::Instant::now()));
        if (zoom - 1.0).abs() < 0.001 {
            entity.remove::<Self>();
            1.0
        } else {
            entity.insert(Self(zoom));
            zoom
        }
    }
}

/// When a panel's zoom last changed: its new zoom shows in a corner for a moment.
#[derive(Component, Clone, Copy, Debug)]
pub struct ZoomChanged(pub std::time::Instant);

#[derive(Component, Debug)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, Persist))]
#[component(on_insert = crate::layouts::membership::panel_inserted, on_discard = crate::layouts::membership::panel_replaced, on_remove = crate::layouts::membership::panel_removed)]
#[relationship(relationship_target = AreaPanels)]
pub struct InArea(#[cfg_attr(feature = "persistence", entities)] pub Entity);

/// Tab order as well as reverse membership.
#[derive(Component, Default)]
#[cfg_attr(feature = "persistence", derive(Reflect, Clone))]
#[cfg_attr(feature = "persistence", reflect(Component, Persist))]
#[relationship_target(relationship = InArea, linked_spawn)]
pub struct AreaPanels(#[cfg_attr(feature = "persistence", entities)] Vec<Entity>);

impl AreaPanels {
    pub fn entities(&self) -> &[Entity] {
        &self.0
    }
}

#[derive(Component, Debug, Default, Clone, Copy)]
pub struct PanelPresentation {
    pub visible: bool,
    pub focused: bool,
    pub rect: Rect,
    pub tab_rect: Rect,
    pub close_rect: Rect,
}

#[derive(Component, Debug, Default, Clone, Copy)]
pub struct AreaPresentation {
    pub visible: bool,
    pub rect: Rect,
    pub surface: Rect,
    pub tab_bar: Rect,
    pub content: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

impl SplitAxis {
    pub(crate) fn index(self) -> usize {
        usize::from(self == Self::Vertical)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AreaPlacement {
    Replace,
    Split {
        axis: SplitAxis,
        high: bool,
        fraction: f32,
    },
}

impl AreaPlacement {
    fn geometry(self) -> Result<geometry::DockPlacement, String> {
        Ok(match self {
            Self::Replace => geometry::DockPlacement::Replace,
            Self::Split {
                axis,
                high,
                fraction,
            } => {
                if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
                    return Err("Invalid placement fraction".into());
                }
                geometry::DockPlacement::Split {
                    axis: axis.index(),
                    high,
                    fraction: (fraction * LAYOUT_EXTENT as f32).round() as i32,
                }
            }
        })
    }
}

#[derive(Debug, Clone)]
pub enum LayoutEdit {
    FocusArea(Entity),
    FocusPanel(Entity),
    KeepPanel(Entity),
    ClosePanels(Vec<Entity>),
    CloseArea(Entity),
    MoveTab {
        panel: Entity,
        area: Entity,
        index: usize,
    },
    SplitTab {
        panel: Entity,
        area: Entity,
        axis: SplitAxis,
        high: bool,
        minimum: [i32; 2],
    },
    SplitArea {
        area: Entity,
        axis: SplitAxis,
        at: i32,
        minimum: [i32; 2],
    },
    JoinAreas {
        source: Entity,
        target: Entity,
        minimum: [i32; 2],
    },
    DockArea {
        source: Entity,
        target: Entity,
        placement: AreaPlacement,
        minimum: [i32; 2],
    },
    ResizeBorder {
        axis: SplitAxis,
        at: i32,
        span: [i32; 2],
        to: i32,
        minimum: i32,
    },
    DropPanel {
        area: Entity,
        factory: PanelFactory,
        index: usize,
        split: Option<(SplitAxis, bool)>,
        minimum: [i32; 2],
    },
}

/// Layout edits from user gestures, made while drawing and applied before
/// keyboard routing in the same frame. Each names the layout it was made in.
#[derive(Resource, Default)]
pub(crate) struct PendingEdits(pub Vec<(Entity, LayoutEdit)>);

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutSystems {
    Apply,
}

/// Presentation contract for a concrete panel component. No catalog or initializer
/// is installed: require `PanelInstance::of::<Self>` on the concrete component.
pub trait Panel: Component {
    const LABEL: &'static str;
    fn draw(
        ui: &mut crate::egui::Ui,
        world: &mut World,
        context: &crate::UiContext<'_>,
        panel: Entity,
    );
}

/// An application-owned panel constructor for a creation drag. Validate
/// application-specific inputs before offering the factory. It must create
/// exactly one panel in the supplied area and return that entity, or, if it
/// can't after all (a file gone since), create nothing and return
/// `Entity::PLACEHOLDER`: the drop does nothing.
#[derive(Clone)]
pub struct PanelFactory {
    pub label: &'static str,
    pub create: std::sync::Arc<dyn Fn(&mut World, Entity) -> Entity + Send + Sync>,
}
impl std::fmt::Debug for PanelFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PanelFactory")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}
impl PanelFactory {
    pub fn new(
        label: &'static str,
        create: impl Fn(&mut World, Entity) -> Entity + Send + Sync + 'static,
    ) -> Self {
        Self {
            label,
            create: std::sync::Arc::new(create),
        }
    }
    pub fn of<P: Panel + Component + Default>() -> Self {
        Self::new(P::LABEL, |world, area| {
            world.spawn((P::default(), InArea(area))).id()
        })
    }
}

/// When an area was last focused (a count, not a time): the most recently
/// focused of several areas is the one opening prefers.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct AreaFocusedAt(pub u64);

#[derive(Resource, Default)]
struct FocusClock(u64);

/// Stamp each layout's focused area when it changes.
fn stamp_focused_areas(
    layouts: Query<&Layout, Changed<Layout>>,
    stamps: Query<&AreaFocusedAt>,
    mut clock: ResMut<FocusClock>,
    mut commands: Commands,
) {
    for layout in &layouts {
        let Some(area) = layout.focused_area else {
            continue;
        };
        // Already the latest: nothing changed but the layout's other fields.
        if stamps.get(area).is_ok_and(|stamp| stamp.0 == clock.0) {
            continue;
        }
        clock.0 += 1;
        if let Ok(mut entity) = commands.get_entity(area) {
            entity.try_insert(AreaFocusedAt(clock.0));
        }
    }
}

/// Layouts, and their drawing on `LayoutSurface` windows (which needs
/// `UiPlugin`). Window chrome is `ChromePlugin`.
pub struct LayoutsPlugin;

impl Plugin for LayoutsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LayoutOrder>()
            .init_resource::<FocusClock>()
            .init_resource::<PendingEdits>()
            .init_resource::<view::LayoutsUi>()
            .init_resource::<CloseHooks>()
            .init_resource::<PanelModified>()
            .init_resource::<Validations>()
            .init_resource::<LayoutFallback>()
            .configure_sets(
                PreUpdate,
                LayoutSystems::Apply.after(crate::UiSystems::Draw),
            )
            .add_systems(PreUpdate, apply_edits.in_set(LayoutSystems::Apply))
            .add_systems(PostUpdate, stamp_focused_areas)
            .add_systems(
                PreUpdate,
                surface::configure.before(crate::UiSystems::Configure),
            );
        #[cfg(debug_assertions)]
        app.add_systems(Last, validate);
        #[cfg(feature = "persistence")]
        persistence::install(app);
    }
}

#[cfg(debug_assertions)]
fn validate(world: &mut World) {
    validate_layouts(world).expect("Invalid layout graph");
}

pub(crate) fn new_layout(world: &mut World, name: String) -> Entity {
    let layout = world.spawn(Layout::default()).id();
    let area = world
        .spawn((
            Area {
                rect: AreaRect {
                    min: [0, 0],
                    max: [LAYOUT_EXTENT; 2],
                },
                selected: None,
            },
            InLayout(layout),
        ))
        .id();
    world.entity_mut(layout).insert(Layout {
        name,
        focused_area: Some(area),
    });
    layout
}

/// Whether the app's close checks allow closing `panels`. Check a whole batch
/// before making structural changes.
pub(crate) fn check_panel_close(
    world: &World,
    panels: impl IntoIterator<Item = Entity>,
) -> Result<(), String> {
    let Some(hooks) = world.get_resource::<CloseHooks>() else {
        return Ok(());
    };
    let panels: Vec<_> = panels.into_iter().collect();
    hooks
        .checks
        .iter()
        .try_for_each(|check| check(world, &panels))
}

/// A short-lived, ordered geometry calculation; IDs are local to this snapshot.
#[derive(Clone)]
pub(crate) struct GeometrySnapshot {
    pub geometry: geometry::Layout,
    pub entities: Vec<Entity>,
}

impl GeometrySnapshot {
    pub fn id(&self, entity: Entity) -> Result<u32, String> {
        self.entities
            .iter()
            .position(|&e| e == entity)
            .map(|i| i as u32 + 1)
            .ok_or("Area is unavailable".into())
    }
    pub fn entity(&self, id: u32) -> Option<Entity> {
        self.entities.get(id as usize - 1).copied()
    }
}

/// Preview and commit use the same final tiling, including the gap left by a
/// last-tab move. IDs belong to the supplied snapshot, not persistent entities.
pub(crate) fn plan_tab_geometry(
    geometry: &geometry::Layout,
    source: u32,
    target: u32,
    last_tab: bool,
    split: Option<(SplitAxis, bool)>,
    minimum: [i32; 2],
) -> Result<(geometry::Layout, u32), String> {
    let mut next = geometry.clone();
    let destination = if let Some((axis, high)) = split {
        let rect = next.area(target).rect;
        let a = axis.index();
        let id = next
            .split(target, a, (rect.min[a] + rect.max[a]) / 2, minimum[a])
            .ok_or("Area is too small to split")?;
        if !high {
            let low = next.area(target).rect;
            let upper = next.area(id).rect;
            next.areas.iter_mut().find(|v| v.id == target).unwrap().rect = upper;
            next.areas.iter_mut().find(|v| v.id == id).unwrap().rect = low;
        }
        id
    } else {
        target
    };
    if source != target && last_tab {
        // Splitting before closing preserves v1's gap-filling choice at
        // T-junctions. A same-area split deliberately leaves an empty half.
        assert!(
            next.close_area(source),
            "Cross-area move can close its source"
        );
    }
    Ok((next, destination))
}

impl LayoutsRef {
    pub fn panel_layout(&self, panel: Entity) -> Result<Entity, String> {
        self.world
            .get::<PanelInstance>(panel)
            .ok_or("Panel is unavailable")?;
        Ok(self
            .world
            .get::<InLayout>(self.world.get::<InArea>(panel).expect("Panel area").0)
            .expect("Area layout")
            .0)
    }

    pub fn panels(&self, layout: Entity) -> Vec<Entity> {
        self.world
            .get::<LayoutAreas>(layout)
            .map_or(&[][..], |a| a.entities())
            .iter()
            .flat_map(|&area| self.panels_in_area(area).iter().copied())
            .collect()
    }

    pub fn focused_panel(&self, layout: Entity) -> Option<Entity> {
        self.world
            .get::<Area>(self.world.get::<Layout>(layout)?.focused_area?)?
            .selected
    }

    pub fn destination_area(&self, layout: Entity) -> Result<Entity, String> {
        self.world
            .get::<Layout>(layout)
            .ok_or("Layout is unavailable")?;
        Ok(*self
            .world
            .get::<LayoutAreas>(layout)
            .map_or(&[][..], |a| a.entities())
            .iter()
            .max_by_key(|&&area| {
                let r = self.world.get::<Area>(area).unwrap().rect;
                (r.max[0] - r.min[0]) * (r.max[1] - r.min[1])
            })
            .ok_or("Layout has no areas")?)
    }

    /// Where a new panel goes: an area with a panel that `fits` (the focused
    /// one, else the most recently focused), else the largest area.
    pub fn area_for(
        &self,
        layout: Entity,
        fits: impl Fn(&World, Entity) -> bool,
    ) -> Result<Entity, String> {
        match self.area_holding(layout, fits, None) {
            Some(area) => Ok(area),
            None => self.destination_area(layout),
        }
    }

    /// An area of `layout` other than `except` with a panel that `fits`: the
    /// focused one, else the most recently focused.
    pub fn area_holding(
        &self,
        layout: Entity,
        fits: impl Fn(&World, Entity) -> bool,
        except: Option<Entity>,
    ) -> Option<Entity> {
        let world = &self.world;
        let focused = world.get::<Layout>(layout).and_then(Layout::focused_area);
        let candidates: Vec<Entity> = world
            .get::<LayoutAreas>(layout)
            .map_or(&[][..], |a| a.entities())
            .iter()
            .copied()
            .filter(|&area| Some(area) != except)
            .filter(|&area| self.panels_in_area(area).iter().any(|&p| fits(world, p)))
            .collect();
        candidates
            .iter()
            .copied()
            .find(|&area| Some(area) == focused)
            .or_else(|| {
                candidates.iter().copied().max_by_key(|&area| {
                    world
                        .get::<AreaFocusedAt>(area)
                        .copied()
                        .unwrap_or_default()
                })
            })
    }

    pub(crate) fn snapshot(&self, layout: Entity) -> Result<GeometrySnapshot, String> {
        self.world
            .get::<Layout>(layout)
            .ok_or("Layout is unavailable")?;
        let entities = self
            .world
            .get::<LayoutAreas>(layout)
            .map_or(&[][..], |a| a.entities())
            .to_vec();
        if entities.is_empty() {
            return Err("Layout has no areas".into());
        }
        let geometry = geometry::Layout::from_live(
            entities
                .iter()
                .enumerate()
                .map(|(i, &e)| geometry::Area {
                    id: i as u32 + 1,
                    rect: self.world.get::<Area>(e).expect("Owned area").rect,
                })
                .collect(),
        );
        Ok(GeometrySnapshot { geometry, entities })
    }

    fn plan_area_relocation(
        &self,
        source: Entity,
        target: Entity,
        placement: Option<AreaPlacement>,
        minimum: [i32; 2],
    ) -> Result<(Entity, GeometrySnapshot, geometry::DockPlan), String> {
        valid_minimum(minimum)?;
        let layout = self
            .world
            .get::<InLayout>(source)
            .ok_or("Source area is unavailable")?
            .0;
        if self
            .world
            .get::<InLayout>(target)
            .ok_or("Target area is unavailable")?
            .0
            != layout
        {
            return Err("Areas belong to different layouts".into());
        }
        let snapshot = self.snapshot(layout)?;
        let a = snapshot.id(source)?;
        let b = snapshot.id(target)?;
        let plan = if let Some(placement) = placement {
            snapshot
                .geometry
                .plan_dock(a, b, placement.geometry()?, minimum)
        } else {
            snapshot.geometry.plan_join(a, b, minimum)
        }
        .ok_or("Area placement cannot satisfy the tiling/minimum sizes")?;
        Ok((layout, snapshot, plan))
    }

    /// A deferred edit still applies if the layout it was made in is still
    /// active and everything it names is still in that layout. A border resize
    /// names a position instead, so that border must still be there.
    pub(crate) fn validate_edit(&self, layout: Entity, edit: &LayoutEdit) -> Result<(), String> {
        if self.active() != Some(layout) {
            return Err("The layout changed during the interaction".into());
        }
        let area_of =
            |world: &World, entity: Entity| world.get::<InArea>(entity).map_or(entity, |p| p.0);
        let targets: Vec<Entity> = match edit {
            LayoutEdit::FocusArea(e)
            | LayoutEdit::FocusPanel(e)
            | LayoutEdit::KeepPanel(e)
            | LayoutEdit::CloseArea(e) => vec![*e],
            LayoutEdit::ClosePanels(es) => es.clone(),
            LayoutEdit::MoveTab { panel, area, .. } | LayoutEdit::SplitTab { panel, area, .. } => {
                vec![*panel, *area]
            }
            LayoutEdit::SplitArea { area, .. } | LayoutEdit::DropPanel { area, .. } => vec![*area],
            LayoutEdit::JoinAreas { source, target, .. }
            | LayoutEdit::DockArea { source, target, .. } => vec![*source, *target],
            LayoutEdit::ResizeBorder { axis, at, span, .. } => {
                let a = axis.index();
                let border = self.snapshot(layout)?.geometry.areas.iter().any(|area| {
                    (area.rect.min[a] == *at || area.rect.max[a] == *at)
                        && area.rect.min[a ^ 1] < span[1]
                        && area.rect.max[a ^ 1] > span[0]
                });
                if !border {
                    return Err("The border moved during the interaction".into());
                }
                Vec::new()
            }
        };
        for e in targets {
            if self
                .world
                .get::<InLayout>(area_of(&self.world, e))
                .map(|r| r.0)
                != Some(layout)
            {
                return Err("The interaction's target is no longer in this layout".into());
            }
        }
        Ok(())
    }

    /// Submit a close intention for application policy. This never destroys data.
    /// The panels a destructive edit would remove, including those removed by
    /// joins/docks. Uses the same geometry planner as the edit itself.
    /// The panels `edit` would close.
    pub fn close_targets(&self, layout: Entity, edit: &LayoutEdit) -> Result<Vec<Entity>, String> {
        self.validate_edit(layout, edit)?;
        match edit {
            LayoutEdit::ClosePanels(panels) => Ok(panels.clone()),
            LayoutEdit::CloseArea(area) => Ok(self.panels_in_area(*area).to_vec()),
            LayoutEdit::JoinAreas {
                source,
                target,
                minimum,
            }
            | LayoutEdit::DockArea {
                source,
                target,
                minimum,
                ..
            } => {
                let placement = if let LayoutEdit::DockArea { placement, .. } = edit {
                    Some(*placement)
                } else {
                    None
                };
                let (_, snapshot, plan) =
                    self.plan_area_relocation(*source, *target, placement, *minimum)?;
                Ok(snapshot
                    .entities
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| {
                        !plan
                            .layout
                            .areas
                            .iter()
                            .any(|area| area.id == *index as u32 + 1)
                    })
                    .flat_map(|(_, &area)| self.panels_in_area(area).iter().copied())
                    .collect())
            }
            _ => Err("Not a destructive layout edit".into()),
        }
    }
}

impl Layouts {
    pub fn create(&mut self, name: impl Into<String>) -> Result<Entity, String> {
        Ok(new_layout(&mut self.world, name.into()))
    }

    pub fn activate(&mut self, layout: Entity) -> Result<(), String> {
        self.world
            .get::<Layout>(layout)
            .ok_or("Layout is unavailable")?;
        if self.active() != Some(layout) {
            if let Some(previous) = self.active() {
                self.world.entity_mut(previous).remove::<ActiveLayout>();
            }
            self.world.entity_mut(layout).insert(ActiveLayout);
        }
        Ok(())
    }

    pub fn focus_area(&mut self, area: Entity) -> Result<(), String> {
        self.world.get::<Area>(area).ok_or("Area is unavailable")?;
        let layout = self.world.get::<InLayout>(area).unwrap().0;
        self.world.get_mut::<Layout>(layout).unwrap().focused_area = Some(area);
        self.stamp(area);
        Ok(())
    }

    fn stamp(&mut self, area: Entity) {
        let mut clock = self.world.get_resource_or_insert_with(FocusClock::default);
        clock.0 += 1;
        let at = clock.0;
        self.world.entity_mut(area).insert(AreaFocusedAt(at));
    }

    /// Select a tab without changing the layout's focused area.
    pub fn select_panel(&mut self, panel: Entity) -> Result<(), String> {
        self.panel_layout(panel)?;
        let area = self.world.get::<InArea>(panel).unwrap().0;
        self.world.get_mut::<Area>(area).unwrap().selected = Some(panel);
        Ok(())
    }

    /// Explicit convenience for selecting a tab and focusing its area.
    pub fn focus_panel(&mut self, panel: Entity) -> Result<(), String> {
        self.select_panel(panel)?;
        self.focus_area(self.world.get::<InArea>(panel).unwrap().0)
    }

    pub fn close_panel(&mut self, panel: Entity) -> Result<(), String> {
        self.panel_layout(panel)?;
        check_panel_close(&self.world, [panel])?;
        let area = self.world.get::<InArea>(panel).unwrap().0;
        let index = self
            .panels_in_area(area)
            .iter()
            .position(|&p| p == panel)
            .unwrap();
        self.world.despawn(panel);
        let tabs = self.panels_in_area(area);
        let next = tabs.get(index.min(tabs.len().saturating_sub(1))).copied();
        if self.world.get::<Area>(area).unwrap().selected == Some(panel) {
            self.world.get_mut::<Area>(area).unwrap().selected = next;
        }
        Ok(())
    }

    fn commit_geometry(
        &mut self,
        layout: Entity,
        snapshot: &GeometrySnapshot,
        geometry: geometry::Layout,
    ) -> HashMap<u32, Entity> {
        let mut mapping = HashMap::new();
        for a in &geometry.areas {
            let entity = if let Some(entity) = snapshot.entity(a.id) {
                self.world.get_mut::<Area>(entity).unwrap().rect = a.rect;
                entity
            } else {
                self.world
                    .spawn((
                        Area {
                            rect: a.rect,
                            selected: None,
                        },
                        InLayout(layout),
                    ))
                    .id()
            };
            mapping.insert(a.id, entity);
        }
        for (i, &area) in snapshot.entities.iter().enumerate() {
            if !mapping.contains_key(&(i as u32 + 1)) {
                self.world.despawn(area);
            }
        }
        let focused = self.world.get::<Layout>(layout).unwrap().focused_area;
        if !mapping.values().any(|&e| Some(e) == focused) {
            let fallback = self.destination_area(layout).unwrap();
            self.world.get_mut::<Layout>(layout).unwrap().focused_area = Some(fallback);
        }
        mapping
    }

    pub fn split_area(
        &mut self,
        area: Entity,
        axis: SplitAxis,
        at: i32,
        minimum: [i32; 2],
    ) -> Result<Entity, String> {
        valid_minimum(minimum)?;
        let layout = self
            .world
            .get::<InLayout>(area)
            .ok_or("Area is unavailable")?
            .0;
        let snapshot = self.snapshot(layout)?;
        let mut next = snapshot.geometry.clone();
        let id = next
            .split(snapshot.id(area)?, axis.index(), at, minimum[axis.index()])
            .ok_or("Area is too small to split")?;
        Ok(self.commit_geometry(layout, &snapshot, next)[&id])
    }

    pub fn close_area(&mut self, area: Entity) -> Result<(), String> {
        let layout = self
            .world
            .get::<InLayout>(area)
            .ok_or("Area is unavailable")?
            .0;
        let snapshot = self.snapshot(layout)?;
        let mut next = snapshot.geometry.clone();
        if !next.close_area(snapshot.id(area)?) {
            return Err("Cannot close the final area".into());
        }
        check_panel_close(&self.world, self.panels_in_area(area).iter().copied())?;
        self.commit_geometry(layout, &snapshot, next);
        Ok(())
    }

    /// Move between layouts while retaining canonical document links and work.
    /// The old area remains, and relationship hooks maintain tab/focus fallbacks.
    pub fn relocate_panel(&mut self, panel: Entity, area: Entity) -> Result<(), String> {
        self.panel_layout(panel)?;
        self.world
            .get::<Area>(area)
            .ok_or("Destination area is unavailable")?;
        let layout = self
            .world
            .get::<InLayout>(area)
            .ok_or("Destination area has no layout")?
            .0;
        self.world
            .get::<Layout>(layout)
            .ok_or("Destination layout is unavailable")?;
        self.world.entity_mut(panel).insert(InArea(area));
        Ok(())
    }

    pub fn move_tab(&mut self, panel: Entity, area: Entity, index: usize) -> Result<(), String> {
        self.place_tab(panel, area, index, None, [1, 1]).map(|_| ())
    }

    pub fn split_tab(
        &mut self,
        panel: Entity,
        area: Entity,
        axis: SplitAxis,
        high: bool,
        minimum: [i32; 2],
    ) -> Result<Entity, String> {
        self.place_tab(panel, area, 0, Some((axis, high)), minimum)
    }

    fn place_tab(
        &mut self,
        panel: Entity,
        area: Entity,
        index: usize,
        split: Option<(SplitAxis, bool)>,
        minimum: [i32; 2],
    ) -> Result<Entity, String> {
        valid_minimum(minimum)?;
        let layout = self.panel_layout(panel)?;
        if self
            .world
            .get::<InLayout>(area)
            .ok_or("Area is unavailable")?
            .0
            != layout
        {
            return Err("Panels cannot move between layouts".into());
        }
        let source = self.world.get::<InArea>(panel).unwrap().0;
        if source == area && split.is_none() {
            // Reordering is not attachment: preserve the open tab and area focus.
            let mut tabs = self.panels_in_area(area).to_vec();
            tabs.retain(|&tab| tab != panel);
            tabs.insert(index.min(tabs.len()), panel);
            self.world.entity_mut(area).replace_related::<InArea>(&tabs);
            self.world.get_mut::<PanelInstance>(panel).unwrap().preview = false;
            return Ok(area);
        }
        let snapshot = self.snapshot(layout)?;
        let (next, id) = plan_tab_geometry(
            &snapshot.geometry,
            snapshot.id(source)?,
            snapshot.id(area)?,
            self.panels_in_area(source).len() == 1,
            split,
            minimum,
        )?;
        // All fallible planning precedes mutation. Detach the moving instance
        // before retiring its source so linked despawning cannot destroy it.
        // This synchronous transition restores ownership before returning.
        self.world.entity_mut(panel).remove::<InArea>();
        if self.world.get::<Area>(source).unwrap().selected == Some(panel) {
            let first = self.panels_in_area(source).first().copied();
            self.world.get_mut::<Area>(source).unwrap().selected = first;
        }
        let mapping = self.commit_geometry(layout, &snapshot, next);
        let destination = mapping[&id];
        let mut tabs = self.panels_in_area(destination).to_vec();
        tabs.insert(index.min(tabs.len()), panel);
        self.world.entity_mut(panel).insert(InArea(destination));
        self.world
            .entity_mut(destination)
            .replace_related::<InArea>(&tabs);
        self.world.get_mut::<PanelInstance>(panel).unwrap().preview = false;
        self.world.get_mut::<Area>(destination).unwrap().selected = Some(panel);
        Ok(destination)
    }

    pub fn move_panel(
        &mut self,
        panel: Entity,
        beside: Entity,
        split: Option<SplitAxis>,
    ) -> Result<(), String> {
        if panel == beside {
            return Err("A panel cannot move relative to itself".into());
        }
        self.panel_layout(beside)?;
        let area = self.world.get::<InArea>(beside).unwrap().0;
        if let Some(axis) = split {
            self.split_tab(panel, area, axis, true, [1, 1])?;
        } else {
            let index = self.panels_in_area(area).len();
            self.move_tab(panel, area, index)?;
        }
        Ok(())
    }

    pub fn join_areas(
        &mut self,
        source: Entity,
        target: Entity,
        minimum: [i32; 2],
    ) -> Result<(), String> {
        self.relocate_area(source, target, None, minimum)
    }

    pub fn dock_area(
        &mut self,
        source: Entity,
        target: Entity,
        placement: AreaPlacement,
        minimum: [i32; 2],
    ) -> Result<(), String> {
        self.relocate_area(source, target, Some(placement), minimum)
    }

    fn relocate_area(
        &mut self,
        source: Entity,
        target: Entity,
        placement: Option<AreaPlacement>,
        minimum: [i32; 2],
    ) -> Result<(), String> {
        let (layout, snapshot, plan) =
            self.plan_area_relocation(source, target, placement, minimum)?;
        // A join/dock can replace target areas and their panels. Validate every
        // removed area before changing geometry or focus.
        check_panel_close(
            &self.world,
            snapshot
                .entities
                .iter()
                .enumerate()
                .filter(|(index, _)| {
                    !plan
                        .layout
                        .areas
                        .iter()
                        .any(|area| area.id == *index as u32 + 1)
                })
                .flat_map(|(_, &area)| self.panels_in_area(area).iter().copied()),
        )?;
        self.commit_geometry(layout, &snapshot, plan.layout);
        self.focus_area(source)
    }

    pub fn resize_border(
        &mut self,
        layout: Entity,
        axis: SplitAxis,
        at: i32,
        span: [i32; 2],
        to: i32,
        minimum: i32,
    ) -> Result<(), String> {
        if !(1..LAYOUT_EXTENT).contains(&at)
            || minimum <= 0
            || minimum > LAYOUT_EXTENT
            || span[0] < 0
            || span[0] >= span[1]
            || span[1] > LAYOUT_EXTENT
        {
            return Err("Invalid border coordinates".into());
        }
        let snapshot = self.snapshot(layout)?;
        let mut next = snapshot.geometry.clone();
        next.move_border(axis.index(), at, span, to, minimum);
        self.commit_geometry(layout, &snapshot, next);
        Ok(())
    }

    pub fn delete(&mut self, layout: Entity) -> Result<(), String> {
        self.world
            .get::<Layout>(layout)
            .ok_or("Layout is unavailable")?;
        check_panel_close(&self.world, self.panels(layout))?;
        let replacement = self
            .world
            .query::<(Entity, &Layout)>()
            .iter(&self.world)
            .find_map(|(e, _)| (e != layout).then_some(e));
        if self.active() == Some(layout) {
            if let Some(next) = replacement {
                self.world.entity_mut(next).insert(ActiveLayout);
            }
        }
        self.world.despawn(layout);
        Ok(())
    }

    /// Apply `edit` as the app's own change: no permissions or close hooks
    /// (close checks still apply).
    pub fn apply_edit(&mut self, layout: Entity, edit: &LayoutEdit) -> Result<(), String> {
        self.validate_edit(layout, edit)?;
        match edit {
            LayoutEdit::FocusArea(e) => self.focus_area(*e),
            LayoutEdit::FocusPanel(e) => self.focus_panel(*e),
            LayoutEdit::KeepPanel(e) => {
                self.world
                    .get_mut::<PanelInstance>(*e)
                    .ok_or("Panel is unavailable")?
                    .preview = false;
                Ok(())
            }
            LayoutEdit::ClosePanels(es) => {
                if es
                    .iter()
                    .copied()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != es.len()
                {
                    return Err("Close request contains duplicate panels".into());
                }
                for &e in es {
                    self.panel_layout(e)?;
                }
                check_panel_close(&self.world, es.iter().copied())?;
                for &e in es {
                    self.close_panel(e)?;
                }
                Ok(())
            }
            LayoutEdit::CloseArea(e) => self.close_area(*e),
            LayoutEdit::MoveTab { panel, area, index } => self.move_tab(*panel, *area, *index),
            LayoutEdit::SplitTab {
                panel,
                area,
                axis,
                high,
                minimum,
            } => self
                .split_tab(*panel, *area, *axis, *high, *minimum)
                .map(|_| ()),
            LayoutEdit::SplitArea {
                area,
                axis,
                at,
                minimum,
            } => self.split_area(*area, *axis, *at, *minimum).map(|_| ()),
            LayoutEdit::JoinAreas {
                source,
                target,
                minimum,
            } => self.join_areas(*source, *target, *minimum),
            LayoutEdit::DockArea {
                source,
                target,
                placement,
                minimum,
            } => self.dock_area(*source, *target, *placement, *minimum),
            LayoutEdit::ResizeBorder {
                axis,
                at,
                span,
                to,
                minimum,
            } => self.resize_border(layout, *axis, *at, *span, *to, *minimum),
            LayoutEdit::DropPanel {
                area,
                factory,
                index,
                split,
                minimum,
            } => {
                valid_minimum(*minimum)?;
                if let Some((axis, _)) = split {
                    let rect = self.world.get::<Area>(*area).unwrap().rect;
                    if rect.max[axis.index()] - rect.min[axis.index()] < minimum[axis.index()] * 2 {
                        return Err("Area is too small to split".into());
                    }
                }
                // Geometry is validated before invoking the app factory.
                let panel = (factory.create)(&mut self.world, *area);
                if panel == Entity::PLACEHOLDER {
                    return Err("Nothing could be shown there".into());
                }
                assert!(
                    self.world.get::<PanelInstance>(panel).is_some()
                        && self.world.get::<InArea>(panel).map(|owner| owner.0) == Some(*area),
                    "PanelFactory must create a panel in its supplied destination"
                );
                if let Some((axis, high)) = split {
                    self.split_tab(panel, *area, *axis, *high, *minimum)
                        .map(|_| ())
                } else {
                    self.move_tab(panel, *area, *index)
                }
            }
        }
    }
}

fn valid_minimum(minimum: [i32; 2]) -> Result<(), String> {
    if minimum.iter().any(|&v| v <= 0 || v > LAYOUT_EXTENT) {
        Err("Invalid area minimum".into())
    } else {
        Ok(())
    }
}

fn apply_edits(world: &mut World) {
    let edits = std::mem::take(&mut world.resource_mut::<PendingEdits>().0);
    for (layout, edit) in edits {
        // A gesture overtaken by other changes is simply dropped.
        let _ = world.layouts_mut().user_edit(layout, &edit);
    }
}
