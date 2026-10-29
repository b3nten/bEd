use super::*;
use crate::{Workbench, WorkbenchWorldExt};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(WorkbenchCorePlugin);
    crate::tests::layout(app.world_mut());
    app
}

#[test]
fn last_layout_can_be_deleted() {
    let mut app = app();
    let layout = app.world_mut().workbench_mut().active_layout().unwrap();
    app.world_mut()
        .workbench_mut()
        .delete_layout(layout)
        .unwrap();
    assert_eq!(app.world().workbench().active_layout(), None);
    app.update();
}

#[derive(Component, PartialEq, Debug)]
struct Draft(String);

#[test]
fn switching_layouts_retains_panel_instances_and_their_state() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let first = w.active_layout().unwrap();
    let panel = w.create_panel(first, "Notes").unwrap();
    let second = w.create_layout("Other").unwrap();
    w.world
        .entity_mut(panel)
        .insert(Draft("keep this draft".into()));
    w.activate_layout(second).unwrap();
    assert_eq!(w.active_layout().unwrap(), second);
    w.activate_layout(first).unwrap();
    assert_eq!(
        w.world.get::<Draft>(panel),
        Some(&Draft("keep this draft".into()))
    );
    assert_eq!(w.panels(first), vec![panel]);
}

#[test]
fn panels_can_split_regroup_and_close_without_retiring_other_panels() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let layout = w.active_layout().unwrap();
    let first = w.create_panel(layout, "First").unwrap();
    let second = w.create_panel(layout, "Second").unwrap();
    let third = w.create_panel(layout, "Third").unwrap();
    w.world.entity_mut(first).insert(Draft("first".into()));
    w.move_panel(second, first, Some(SplitAxis::Horizontal))
        .unwrap();
    w.move_panel(third, second, Some(SplitAxis::Vertical))
        .unwrap();
    w.move_panel(first, third, None).unwrap();
    assert_eq!(w.panels(layout).len(), 3);
    w.close_panel(third).unwrap();
    w.close_panel(second).unwrap();
    assert!(w.world.get_entity(third).is_err());
    assert_eq!(w.world.get::<Draft>(first), Some(&Draft("first".into())));
    assert_eq!(w.panels(layout), vec![first]);
    w.focus_panel(first).unwrap();
    assert_eq!(w.focused_panel(layout), Some(first));
}

#[test]
fn deletion_retires_owned_panels_and_selects_a_surviving_layout() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let first = w.active_layout().unwrap();
    let panel = w.create_panel(first, "Disposable").unwrap();
    let second = w.create_layout("Survives").unwrap();
    w.delete_layout(first).unwrap();
    assert_eq!(w.active_layout().unwrap(), second);
    assert!(w.world.get_entity(panel).is_err());
}

#[test]
fn rejected_moves_preserve_layouts() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let layout = w.active_layout().unwrap();
    let first = w.create_panel(layout, "First").unwrap();
    let other = w.create_layout("Other").unwrap();
    let second = w.create_panel(other, "Second").unwrap();
    assert!(w.move_panel(first, second, None).is_err());
    assert_eq!(w.panels(layout), vec![first]);
    assert!(w.world().get::<PanelInstance>(second).is_some());
}

#[test]
fn a_border_resize_is_refused_once_that_border_is_gone() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let layout = w.active_layout().unwrap();
    let first = w.create_panel(layout, "First").unwrap();
    let second = w.create_panel(layout, "Second").unwrap();
    w.move_panel(second, first, Some(SplitAxis::Horizontal))
        .unwrap();
    let resize = LayoutEdit::ResizeBorder {
        axis: SplitAxis::Horizontal,
        at: 5000,
        span: [0, LAYOUT_EXTENT],
        to: 8000,
        minimum: 1,
    };
    w.move_panel(second, first, Some(SplitAxis::Vertical))
        .unwrap();
    let before: Vec<_> = w
        .panels(layout)
        .iter()
        .map(|&p| {
            w.world()
                .get::<Area>(w.world().get::<InArea>(p).unwrap().0)
                .unwrap()
                .rect()
        })
        .collect();
    assert!(w.edit(layout, &resize).is_err());
    let after: Vec<_> = w
        .panels(layout)
        .iter()
        .map(|&p| {
            w.world()
                .get::<Area>(w.world().get::<InArea>(p).unwrap().0)
                .unwrap()
                .rect()
        })
        .collect();
    assert_eq!(before, after);
}
#[test]
fn closing_content_retains_its_area_but_moving_the_last_tab_closes_its_footprint() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let layout = w.active_layout().unwrap();
    let first = w.destination_area(layout).unwrap();
    let second = w
        .split_area(first, SplitAxis::Horizontal, 5000, [100, 100])
        .unwrap();
    let panel = w.create_panel_in_area(second, "Second").unwrap();
    let rect = w.world.get::<Area>(second).unwrap().rect();
    w.close_panel(panel).unwrap();
    assert_eq!(w.world.get::<Area>(second).unwrap().rect(), rect);
    assert!(w.panels_in_area(second).is_empty());
    let panel = w.create_panel_in_area(second, "Moved").unwrap();
    w.world.entity_mut(panel).insert(Draft("retained".into()));
    w.move_tab(panel, first, 0).unwrap();
    assert!(w.world.get_entity(second).is_err());
    assert_eq!(w.world.get::<InArea>(panel).unwrap().0, first);
    assert_eq!(
        w.world.get::<Area>(first).unwrap().rect(),
        AreaRect {
            min: [0, 0],
            max: [LAYOUT_EXTENT; 2]
        }
    );
    assert_eq!(w.world.get::<Draft>(panel), Some(&Draft("retained".into())));
}

#[test]
fn per_item_tab_style_does_not_change_preview_keep_or_move_semantics() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let layout = w.active_layout().unwrap();
    let area = w.destination_area(layout).unwrap();
    let preview = w.create_panel_in_area(area, "Preview").unwrap();
    let permanent = w.create_panel_in_area(area, "Permanent").unwrap();
    assert_eq!(
        PanelTabStyle::default(),
        PanelTabStyle {
            italic: false,
            text_opacity: 1.0,
        }
    );
    assert!(w.world.get::<PanelTabStyle>(preview).is_none());
    let permanent_style = PanelTabStyle {
        italic: true,
        text_opacity: 0.6,
    };
    w.world.get_mut::<PanelInstance>(preview).unwrap().preview = true;
    // A normal-looking preview remains a preview; an italic permanent tab does not become one.
    w.world.entity_mut(preview).insert(PanelTabStyle::default());
    w.world.entity_mut(permanent).insert(permanent_style);
    validate_workbench(w.world_mut()).unwrap();
    assert!(w.world.get::<PanelInstance>(preview).unwrap().preview);
    assert!(!w.world.get::<PanelInstance>(permanent).unwrap().preview);

    w.edit(layout, &LayoutEdit::KeepPanel(preview)).unwrap();
    assert!(!w.world.get::<PanelInstance>(preview).unwrap().preview);
    assert_eq!(
        w.world.get::<PanelTabStyle>(preview),
        Some(&PanelTabStyle::default())
    );
    w.world.get_mut::<PanelInstance>(permanent).unwrap().preview = true;
    w.move_tab(permanent, area, 0).unwrap();
    assert!(!w.world.get::<PanelInstance>(permanent).unwrap().preview);
    assert_eq!(
        w.world.get::<PanelTabStyle>(permanent),
        Some(&permanent_style)
    );
    assert_eq!(w.panels_in_area(area), &[permanent, preview]);

    w.world.get_mut::<PanelInstance>(preview).unwrap().preview = true;
    w.world.entity_mut(preview).remove::<PanelTabStyle>();
    validate_workbench(w.world_mut()).unwrap();
    assert!(w.world.get::<PanelInstance>(preview).unwrap().preview);
    assert!(w.world.get::<PanelTabStyle>(preview).is_none());
    assert_eq!(
        w.world.get::<PanelTabStyle>(permanent),
        Some(&permanent_style)
    );
}

#[test]
fn tab_reordering_uses_destination_order_after_removing_the_source() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let layout = w.active_layout().unwrap();
    let area = w.destination_area(layout).unwrap();
    let a = w.create_panel_in_area(area, "A").unwrap();
    let b = w.create_panel_in_area(area, "B").unwrap();
    let c = w.create_panel_in_area(area, "C").unwrap();
    w.world.get_mut::<PanelInstance>(a).unwrap().preview = true;
    w.move_tab(a, area, 2).unwrap();
    assert_eq!(w.panels_in_area(area), &[b, c, a]);
    assert!(!w.world.get::<PanelInstance>(a).unwrap().preview);
    w.move_tab(c, area, 0).unwrap();
    assert_eq!(w.panels_in_area(area), &[c, b, a]);
    assert_eq!(w.focused_panel(layout), Some(c));
}

#[test]
fn all_four_tab_edges_split_without_recreating_the_panel_and_keep_an_empty_source() {
    for axis in [SplitAxis::Horizontal, SplitAxis::Vertical] {
        for high in [false, true] {
            let mut app = app();
            let w = app.world_mut().workbench_mut();
            let layout = w.active_layout().unwrap();
            let original = w.destination_area(layout).unwrap();
            let panel = w.create_panel_in_area(original, "One").unwrap();
            w.world.entity_mut(panel).insert(Draft("canonical".into()));
            let destination = w
                .split_tab(panel, original, axis, high, [100, 100])
                .unwrap();
            assert_eq!(
                w.world.get::<LayoutAreas>(layout).unwrap().entities().len(),
                2
            );
            assert!(w.panels_in_area(original).is_empty());
            assert_eq!(w.world.get::<InArea>(panel).unwrap().0, destination);
            assert_eq!(
                w.world.get::<Draft>(panel),
                Some(&Draft("canonical".into()))
            );
            let r = w.world.get::<Area>(destination).unwrap().rect();
            let i = axis.index();
            assert_eq!(if high { r.min[i] } else { r.max[i] }, 5000);
        }
    }
}

#[test]
fn partial_area_join_preserves_the_target_tail_and_its_live_content() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let layout = w.active_layout().unwrap();
    let left = w.destination_area(layout).unwrap();
    let right = w
        .split_area(left, SplitAxis::Horizontal, 5000, [100, 100])
        .unwrap();
    let lower = w
        .split_area(left, SplitAxis::Vertical, 5000, [100, 100])
        .unwrap();
    let source = w.create_panel_in_area(lower, "Lower").unwrap();
    let target = w.create_panel_in_area(right, "Right").unwrap();
    w.join_areas(lower, right, [100, 100]).unwrap();
    assert_eq!(
        w.world.get::<Area>(lower).unwrap().rect(),
        AreaRect {
            min: [0, 5000],
            max: [10000, 10000]
        }
    );
    assert_eq!(
        w.world.get::<Area>(right).unwrap().rect(),
        AreaRect {
            min: [5000, 0],
            max: [10000, 5000]
        }
    );
    assert_eq!(w.world.get::<InArea>(source).unwrap().0, lower);
    assert_eq!(w.world.get::<InArea>(target).unwrap().0, right);
    assert_eq!(w.focused_panel(layout), Some(source));
}

#[test]
fn an_edit_for_a_layout_that_is_no_longer_active_is_refused() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let layout = w.active_layout().unwrap();
    let area = w.destination_area(layout).unwrap();
    let other = w.create_layout("Other").unwrap();
    w.activate_layout(other).unwrap();
    let before = w.world.get::<Area>(area).unwrap().rect();
    assert!(
        w.edit(
            layout,
            &LayoutEdit::SplitArea {
                area,
                axis: SplitAxis::Horizontal,
                at: 5000,
                minimum: [100, 100]
            }
        )
        .is_err()
    );
    assert_eq!(w.world.get::<Area>(area).unwrap().rect(), before);
}

#[test]
fn invalid_creation_drop_does_not_call_the_application_factory_or_modify_geometry() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let layout = w.active_layout().unwrap();
    let area = w.destination_area(layout).unwrap();
    let factory = PanelFactory::new("Must not run", |_, _| {
        panic!("Invalid drop invoked factory")
    });
    let original = w.world.get::<Area>(area).unwrap().rect();
    assert!(
        w.edit(
            layout,
            &LayoutEdit::DropPanel {
                area,
                factory,
                index: 0,
                split: Some((SplitAxis::Horizontal, false)),
                minimum: [6000, 6000]
            }
        )
        .is_err()
    );
    assert_eq!(w.world.get::<Area>(area).unwrap().rect(), original);
    assert!(w.panels(layout).is_empty());
}

#[cfg(feature = "ui")]
#[test]
fn user_closure_without_unsaved_documents_closes_immediately() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let layout = w.active_layout().unwrap();
    let panel = w.create_panel(layout, "Nothing to save").unwrap();
    w.user_edit(layout, &LayoutEdit::ClosePanels(vec![panel]))
        .unwrap();
    assert!(w.world.get_entity(panel).is_err());

    // An edit made in a layout that is no longer active is refused.
    let panel = w.create_panel(layout, "Stale request").unwrap();
    let other = w.create_layout("Other").unwrap();
    w.activate_layout(other).unwrap();
    assert!(
        w.user_edit(layout, &LayoutEdit::ClosePanels(vec![panel]))
            .is_err()
    );
    assert!(w.world.get_entity(panel).is_ok());
}

#[test]
fn moving_between_layouts_preserves_shared_document_and_local_state() {
    let mut app = app();
    let w = app.world_mut().workbench_mut();
    let first = w.active_layout().unwrap();
    let panel = w.create_panel(first, "Viewer").unwrap();
    let document = w
        .world_mut()
        .workbench_mut()
        .open_document(MemorySource::new("File", TextBuffer::from("text")))
        .unwrap();
    w.world_mut()
        .entity_mut(panel)
        .insert(DocumentReference::new(document.id()));
    w.world_mut()
        .entity_mut(panel)
        .insert(Draft("view state".into()));
    let second = w.create_layout("Other").unwrap();
    let area = w.destination_area(second).unwrap();
    w.relocate_panel(panel, area).unwrap();
    assert_eq!(
        w.world()
            .get::<DocumentReference>(panel)
            .unwrap()
            .document(),
        document.id()
    );
    assert_eq!(
        w.world().get::<Draft>(panel),
        Some(&Draft("view state".into()))
    );
}

/// Bare test panels; applications spawn their own panel components instead.
pub(crate) trait TestPanels {
    fn create_panel(&mut self, layout: Entity, title: &str) -> Result<Entity, String>;
    fn create_panel_in_area(&mut self, area: Entity, title: &str) -> Result<Entity, String>;
}

impl TestPanels for Workbench {
    fn create_panel(&mut self, layout: Entity, title: &str) -> Result<Entity, String> {
        let area = self.destination_area(layout)?;
        self.create_panel_in_area(area, title)
    }
    fn create_panel_in_area(&mut self, area: Entity, title: &str) -> Result<Entity, String> {
        self.world()
            .get::<Area>(area)
            .ok_or("Area is unavailable")?;
        let instance = PanelInstance {
            title: title.into(),
            ..default()
        };
        Ok(self.world_mut().spawn((instance, InArea(area))).id())
    }
}

/// A layout fixture; core installation itself deliberately creates no layout.
pub(crate) fn layout(world: &mut World) -> Entity {
    let layout = crate::layout::new_layout(world, "Test".into());
    if world.workbench().active_layout().is_none() {
        world.entity_mut(layout).insert(ActiveLayout);
    }
    layout
}
