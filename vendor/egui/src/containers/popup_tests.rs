use super::*;
use crate::{Color32, Event, RawInput, Shape};

struct Snapshot {
    logical: Rect,
    painted: Rect,
    clip: Rect,
    alpha: u8,
    text: epaint::TextShape,
    text_clip: Rect,
}

fn render(
    ctx: &Context,
    time: f64,
    open: bool,
    kind: PopupKind,
    anchor: Rect,
    align: RectAlign,
    events: Vec<Event>,
) -> Option<Snapshot> {
    let mut logical = None;
    let fill = Color32::from_rgb(31, 61, 91);
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            time: Some(time),
            events,
            ..Default::default()
        },
        |ui| {
            let popup = Popup::new(Id::new("opening"), ctx.clone(), anchor, ui.layer_id())
                .kind(kind)
                .align(align)
                .gap(4.0)
                .open(open)
                .frame(Frame::NONE.fill(fill));
            if let Some(response) = popup.show(|ui| {
                ui.set_min_size(vec2(180.0, 80.0));
                ui.label("Ordinary popup text");
            }) {
                logical = Some(response.response.rect);
            }
        },
    );
    output.textures_delta.clear();
    let text = output.shapes.iter().find_map(|shape| match &shape.shape {
        Shape::Text(text) if text.galley.job.text == "Ordinary popup text" => {
            Some((text.clone(), shape.clip_rect))
        }
        _ => None,
    });
    output
        .shapes
        .into_iter()
        .find_map(|shape| match shape.shape {
            Shape::Rect(rect) if rect.fill.a() > 0 => Some(Snapshot {
                logical: logical.unwrap(),
                painted: rect.rect,
                clip: shape.clip_rect,
                alpha: rect.fill.a(),
                text: text.as_ref().unwrap().0.clone(),
                text_clip: text.as_ref().unwrap().1,
            }),
            _ => None,
        })
}

fn context() -> Context {
    let ctx = Context::default();
    ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::new(1).unwrap());
    ctx.global_style_mut(|s| s.animation_time = 0.15);
    ctx
}

#[test]
fn popup_frames_grow_from_their_anchor_while_contents_only_fade() {
    for kind in [PopupKind::Menu, PopupKind::Tooltip, PopupKind::Popup] {
        for align in [
            RectAlign::BOTTOM_START,
            RectAlign::TOP_END,
            RectAlign::LEFT,
            RectAlign::RIGHT_START,
        ] {
            let ctx = context();
            let anchor = Rect::from_min_size(emath::pos2(300.0, 220.0), vec2(100.0, 30.0));
            assert!(
                render(&ctx, 0.0, true, kind, anchor, align, vec![]).is_none(),
                "Sizing is invisible"
            );
            let opening = render(&ctx, 0.075, true, kind, anchor, align, vec![]).unwrap();
            let settled = render(&ctx, 0.2, true, kind, anchor, align, vec![]).unwrap();
            assert_eq!(
                opening.logical, settled.logical,
                "Input and layout keep final coordinates"
            );
            let alpha = emath::easing::cubic_out(0.5);
            let scale = 0.96 + 0.04 * alpha;
            let (_, source) = align.pivot_pos(&anchor, 4.0);
            let pivot = source.clamp(settled.logical.min, settled.logical.max);
            let transform = emath::TSTransform::new(pivot.to_vec2() * (1.0 - scale), scale);
            let expected = transform * settled.painted;
            assert!((opening.painted.min - expected.min).length() < 0.001);
            assert!((opening.painted.max - expected.max).length() < 0.001);
            let expected_clip = transform * settled.clip;
            assert!((opening.clip.min - expected_clip.min).length() < 0.001);
            assert!((opening.clip.max - expected_clip.max).length() < 0.001);
            assert_eq!(opening.alpha, (255.0 * alpha).round() as u8);
            assert_eq!(settled.alpha, 255);
            assert_eq!(opening.text.pos, settled.text.pos);
            assert_eq!(opening.text.galley.rect, settled.text.galley.rect);
            assert_eq!(
                opening.text.galley.mesh_bounds,
                settled.text.galley.mesh_bounds
            );
            assert_eq!(opening.text_clip, settled.text_clip);
            assert_eq!(opening.text.fallback_color.a(), opening.alpha);
            assert_eq!(settled.text.fallback_color.a(), 255);
        }
    }
}

#[test]
fn a_popup_flipped_at_the_window_edge_grows_from_its_placed_source() {
    let ctx = context();
    let anchor = Rect::from_min_size(emath::pos2(760.0, 560.0), vec2(30.0, 20.0));
    let paint = |time| {
        render(
            &ctx,
            time,
            true,
            PopupKind::Menu,
            anchor,
            RectAlign::BOTTOM_START,
            vec![],
        )
    };
    paint(0.0);
    let opening = paint(0.075).unwrap();
    let settled = paint(0.2).unwrap();
    assert!(settled.logical.max.y < anchor.min.y);
    assert!((settled.logical.max.x - anchor.max.x).abs() < 0.001);
    let (_, source) = RectAlign::TOP_END.pivot_pos(&anchor, 4.0);
    let pivot = source.clamp(settled.logical.min, settled.logical.max);
    let scale = 0.96 + 0.04 * emath::easing::cubic_out(0.5);
    let expected =
        emath::TSTransform::new(pivot.to_vec2() * (1.0 - scale), scale) * settled.painted;
    assert!((opening.painted.min - expected.min).length() < 0.001);
    assert!((opening.painted.max - expected.max).length() < 0.001);
}

#[test]
fn reopening_restarts_growth_and_reduced_motion_settles_immediately() {
    let ctx = context();
    let anchor = Rect::from_min_size(emath::pos2(250.0, 150.0), vec2(100.0, 30.0));
    let paint = |time, open| {
        render(
            &ctx,
            time,
            open,
            PopupKind::Menu,
            anchor,
            RectAlign::BOTTOM_START,
            vec![],
        )
    };
    paint(0.0, true);
    let first = paint(0.075, true).unwrap();
    let full = paint(0.2, true).unwrap();
    assert!(first.painted.width() < full.painted.width());
    assert!(paint(0.3, false).is_none());
    assert!(paint(0.4, true).is_none());
    let reopened = paint(0.475, true).unwrap();
    assert!((first.painted.width() - reopened.painted.width()).abs() < 0.001);
    ctx.global_style_mut(|s| s.animation_time = 0.0);
    let reduced = paint(0.48, true).unwrap();
    assert_eq!(reduced.painted, full.painted);
    assert_eq!(reduced.alpha, 255);
}

#[test]
fn interacting_settles_growth() {
    for keyboard in [false, true] {
        let ctx = context();
        let anchor = Rect::from_min_size(emath::pos2(250.0, 150.0), vec2(100.0, 30.0));
        let paint = |time, events| {
            render(
                &ctx,
                time,
                true,
                PopupKind::Menu,
                anchor,
                RectAlign::BOTTOM_START,
                events,
            )
        };
        paint(0.0, vec![Event::PointerMoved(emath::pos2(20.0, 20.0))]);
        let opening = paint(0.02, vec![]).unwrap();
        assert!(opening.painted.width() < opening.logical.width());
        let event = if keyboard {
            Event::Text("x".into())
        } else {
            Event::PointerMoved(opening.logical.center())
        };
        let interacted = paint(0.03, vec![event]).unwrap();
        assert_eq!(interacted.painted, opening.logical);
        assert!(
            interacted.alpha < 255,
            "Interaction settles scale, not fading"
        );
        let following = paint(0.04, vec![]).unwrap();
        assert_eq!(following.painted, opening.logical);
    }
}
