use crate::{epaint::Shape, *};

#[derive(Clone, Copy, Debug)]
enum Control {
    Button,
    Checkbox,
    Radio,
    Slider,
    Drag,
    Menu,
    Combo,
    Collapsing,
    Link,
}

fn context() -> Context {
    let ctx = Context::default();
    ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    ctx.global_style_mut(|s| {
        s.animation_time = 0.16;
        s.visuals.override_text_color = None;
        for (v, fill, ink) in [
            (
                &mut s.visuals.widgets.inactive,
                Color32::from_rgb(20, 40, 60),
                Color32::from_gray(100),
            ),
            (
                &mut s.visuals.widgets.hovered,
                Color32::from_rgb(100, 140, 180),
                Color32::from_gray(200),
            ),
            (
                &mut s.visuals.widgets.active,
                Color32::from_rgb(180, 80, 40),
                Color32::WHITE,
            ),
        ] {
            v.bg_fill = fill;
            v.weak_bg_fill = fill;
            v.bg_stroke = Stroke::new(1.0, ink);
            v.fg_stroke = Stroke::new(1.0, ink);
            v.expansion = 0.0;
            v.corner_radius = CornerRadius::same(4);
        }
    });
    ctx
}

fn render(
    ctx: &Context,
    time: f64,
    events: Vec<Event>,
    mut draw: impl FnMut(&mut Ui) -> Response,
) -> (Response, Vec<Shape>) {
    let mut response = None;
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(640.0, 480.0))),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| {
            let r = draw(ui);
            // Allow release/exit to return to idle in these pointer fixtures.
            ctx.memory_mut(|m| m.surrender_focus(r.id));
            response = Some(r);
        },
    );
    output.textures_delta.clear();
    (
        response.unwrap(),
        output.shapes.into_iter().map(|s| s.shape).collect(),
    )
}

fn draw(control: Control, ui: &mut Ui) -> Response {
    match control {
        Control::Button => ui.button("Button"),
        Control::Checkbox => ui.checkbox(&mut false, "Checkbox"),
        Control::Radio => ui.radio(false, "Radio"),
        Control::Slider => ui.add(Slider::new(&mut 0.5, 0.0..=1.0).show_value(false)),
        Control::Drag => ui.add(DragValue::new(&mut 0.5)),
        Control::Menu => ui.menu_button("Menu", |_| {}).response,
        Control::Combo => {
            ComboBox::from_label("Combo")
                .selected_text("Choice")
                .show_ui(ui, |_| {})
                .response
        }
        Control::Collapsing => {
            CollapsingHeader::new("Header")
                .show(ui, |_| {})
                .header_response
        }
        Control::Link => ui.link("Link"),
    }
}

#[test]
fn native_controls_fade_hover_and_press_without_changing_callsites() {
    for control in [
        Control::Button,
        Control::Checkbox,
        Control::Radio,
        Control::Slider,
        Control::Drag,
        Control::Menu,
        Control::Combo,
        Control::Collapsing,
        Control::Link,
    ] {
        let ctx = context();
        let outside = pos2(600.0, 400.0);
        let paint = |time, events| render(&ctx, time, events, |ui| draw(control, ui));
        paint(0.0, vec![Event::PointerMoved(outside)]);
        let (r, idle) = paint(0.1, vec![]);
        let inside = r.rect.center();
        let (enter, early) = paint(0.12, vec![Event::PointerMoved(inside)]);
        let (mid, moving) = paint(0.16, vec![]);
        let (end, hover) = paint(0.4, vec![]);
        assert!(
            early == idle,
            "{control:?}: entering starts at the displayed colors"
        );
        assert!(
            moving != idle && moving != hover,
            "{control:?}: hover must have an intermediate shade"
        );
        assert_eq!(enter.rect, mid.rect);
        assert_eq!(mid.rect, end.rect, "Color motion must not affect layout");
        if !matches!(control, Control::Link) {
            paint(
                0.42,
                vec![Event::PointerButton {
                    pos: inside,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                }],
            );
            let (_, moving) = paint(0.46, vec![]);
            let (_, pressed) = paint(0.7, vec![]);
            assert!(
                moving != hover && moving != pressed,
                "{control:?}: press must fade"
            );
        }
    }
}

#[test]
fn color_retargeting_is_continuous_clears_and_discards_hidden_widgets() {
    let ctx = context();
    let id = Id::new("color");
    let a = Color32::from_rgba_unmultiplied(20, 60, 100, 40);
    let b = Color32::from_rgb(200, 100, 50);
    let mut displayed = a;
    let mut paint = |time, target, seconds| {
        render(&ctx, time, vec![], |ui| {
            displayed = ctx.animate_color_with_time(id, target, seconds);
            ui.label("Color fixture")
        });
        displayed
    };
    assert_eq!(paint(0.0, a, 0.16), a);
    assert_eq!(paint(0.1, b, 0.16), a);
    let mid = paint(0.14, b, 0.16);
    assert!(mid != a && mid != b);
    assert_eq!(paint(0.14, a, 0.16), mid, "Reversing must not jump");
    assert_eq!(paint(0.4, a, 0.16), a);
    assert_eq!(paint(0.41, b, 0.0), b, "Reduced motion settles immediately");
    ctx.clear_animations();
    assert_eq!(
        paint(0.42, a, 0.16),
        a,
        "Clear must include widget animations"
    );
    render(&ctx, 0.43, vec![], |ui| ui.label("Hidden"));
    assert_eq!(paint(0.44, b, 0.16), b, "Reappearing widgets start settled");
}

#[test]
fn checkbox_selection_fades_while_its_value_and_click_change_immediately() {
    let ctx = context();
    let mut checked = false;
    let mut paint =
        |time, events| render(&ctx, time, events, |ui| ui.checkbox(&mut checked, "Check"));
    let (r, idle) = paint(0.0, vec![]);
    let p = r.rect.center();
    let button = |pressed| Event::PointerButton {
        pos: p,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    paint(0.1, vec![Event::PointerMoved(p), button(true)]);
    let (r, _) = paint(0.3, vec![button(false)]);
    assert!(r.clicked() && r.changed());
    let (_, moving) = paint(0.34, vec![]);
    let (_, settled) = paint(0.6, vec![]);
    assert!(checked);
    assert!(moving != idle && moving != settled);
    let mark_alpha = |shapes: &[Shape]| {
        shapes
            .iter()
            .find_map(|s| match s {
                Shape::Path(p) if p.points.len() == 3 => match p.stroke.color {
                    epaint::ColorMode::Solid(c) => Some(c.a()),
                    _ => None,
                },
                _ => None,
            })
            .expect("Checkmark path")
    };
    assert!(mark_alpha(&moving) > 0 && mark_alpha(&moving) < mark_alpha(&settled));
}

#[test]
fn text_focus_border_fades_while_keyboard_editing_is_immediate() {
    let ctx = context();
    let id = Id::new("text");
    let mut text = String::new();
    let mut paint = |time, events| {
        render(&ctx, time, events, |ui| {
            ui.add(TextEdit::singleline(&mut text).id(id))
        })
    };
    let (r, idle) = paint(0.0, vec![]);
    // render surrenders focus after drawing; reassert it for each fixture frame.
    ctx.memory_mut(|m| m.request_focus(id));
    let (_, entering) = paint(0.1, vec![Event::Text("a".into())]);
    ctx.memory_mut(|m| m.request_focus(id));
    let (mid, moving) = paint(0.14, vec![]);
    ctx.memory_mut(|m| m.request_focus(id));
    let (_, settled) = paint(0.4, vec![]);
    assert_eq!(text, "a");
    let border = |shapes: &[Shape]| {
        shapes
            .iter()
            .find_map(|s| match s {
                Shape::Rect(r) => Some(r.stroke.color),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(border(&entering), border(&idle));
    assert_ne!(border(&moving), border(&idle));
    assert_ne!(border(&moving), border(&settled));
    assert_eq!(r.rect, mid.rect);
}

#[test]
fn selected_buttons_fade_and_explicit_fills_keep_precedence() {
    for explicit in [None, Some(Color32::from_rgb(45, 90, 135))] {
        let ctx = context();
        let paint = |time, selected| {
            render(&ctx, time, vec![], |ui| {
                let button = Button::new("Choice").selected(selected);
                ui.add(if let Some(fill) = explicit {
                    button.fill(fill)
                } else {
                    button
                })
            })
        };
        let (r, idle) = paint(0.0, false);
        let (_, early) = paint(0.1, true);
        let (mid, moving) = paint(0.14, true);
        let (_, settled) = paint(0.4, true);
        let fill = |shapes: &[Shape]| {
            shapes
                .iter()
                .find_map(|s| match s {
                    Shape::Rect(r) => Some(r.fill),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(fill(&early), fill(&idle));
        if let Some(explicit) = explicit {
            assert_eq!(fill(&moving), explicit);
            assert_eq!(fill(&settled), explicit);
        } else {
            assert_ne!(fill(&moving), fill(&idle));
            assert_ne!(fill(&moving), fill(&settled));
            assert_eq!(fill(&settled), ctx.global_style().visuals.selection.bg_fill);
        }
        assert_eq!(r.rect, mid.rect);
    }
}

#[test]
fn floating_scrollbar_opacity_fades_in_and_out() {
    let ctx = context();
    ctx.global_style_mut(|s| {
        s.spacing.scroll = style::ScrollStyle::floating();
        s.spacing.scroll.active_handle_opacity = 0.0;
        s.spacing.scroll.interact_handle_opacity = 1.0;
    });
    let paint = |time, events| {
        render(&ctx, time, events, |ui| {
            let output = ScrollArea::vertical().show(ui, |ui| {
                ui.allocate_space(vec2(100.0, 2000.0));
            });
            ctx.read_response(output.id.with(1_usize)).unwrap()
        })
    };
    paint(0.0, vec![Event::PointerMoved(pos2(600.0, 600.0))]);
    paint(0.1, vec![]);
    let (r, idle) = paint(0.4, vec![]);
    let point = pos2(r.rect.right() - 1.0, r.rect.top() + 10.0);
    let alpha = |shapes: &[Shape]| {
        shapes
            .iter()
            .rev()
            .find_map(|s| match s {
                Shape::Rect(rect) if rect.rect.contains(point) => Some(rect.fill.a()),
                _ => None,
            })
            .expect("Scrollbar handle")
    };
    assert_eq!(alpha(&idle), 0);
    paint(0.5, vec![Event::PointerMoved(point)]);
    let (_, moving) = paint(0.54, vec![]);
    let (_, settled) = paint(0.8, vec![]);
    assert!(alpha(&moving) > 0 && alpha(&moving) < alpha(&settled));
    paint(0.9, vec![Event::PointerMoved(pos2(600.0, 600.0))]);
    let (_, leaving) = paint(0.94, vec![]);
    assert!(alpha(&leaving) > 0 && alpha(&leaving) < alpha(&settled));
    assert_eq!(alpha(&paint(1.2, vec![]).1), 0);
}
