//! Native draw-data regressions for transient opening motion.
use super::*;
use dear_imgui_rs::{Condition, FramePrepareOptions, WindowFlags};
use std::path::PathBuf;

const MARKER: u32 = 0xff4d_99e6;

#[derive(Clone, Debug)]
struct Geometry {
    id: u32,
    pos: [f32; 2],
    size: [f32; 2],
    vertices: Vec<sys::ImDrawVert>,
    clips: Vec<[f32; 4]>,
}

struct FrameGeometry {
    popup: u32,
    before: Vec<Geometry>,
    after: Vec<Geometry>,
}

fn context() -> Context {
    let mut context = Context::create();
    context.set_ini_filename(None::<PathBuf>).unwrap();
    context
        .font_atlas()
        .try_claim_legacy_renderer()
        .unwrap()
        .build();
    context.io_mut().add_mouse_pos_event([20.0, 20.0]);
    context
}

fn geometry(binding: &ContextBinding, ids: &[u32]) -> Vec<Geometry> {
    binding.with_bound_context(|| unsafe {
        ids.iter()
            .filter_map(|&id| {
                let window = sys::igFindWindowByID(id);
                if window.is_null() || !(*window).Active || (*window).Hidden {
                    return None;
                }
                let window = &*window;
                let list = &*window.DrawList;
                let vertices = (0..list.VtxBuffer.Size as usize)
                    .map(|index| *list.VtxBuffer.Data.add(index))
                    .collect();
                let clips = (0..list.CmdBuffer.Size as usize)
                    .filter_map(|index| {
                        let command = &*list.CmdBuffer.Data.add(index);
                        (command.ElemCount > 0).then(|| {
                            let clip = command.ClipRect;
                            [clip.x, clip.y, clip.z, clip.w]
                        })
                    })
                    .collect();
                Some(Geometry {
                    id,
                    pos: [window.Pos.x, window.Pos.y],
                    size: [window.Size.x, window.Size.y],
                    vertices,
                    clips,
                })
            })
            .collect()
    })
}

fn marker(ui: &Ui) {
    let point = ui.cursor_screen_pos();
    ui.get_window_draw_list()
        .add_rect(point, [point[0] + 24.0, point[1] + 18.0], MARKER)
        .filled(true)
        .build();
    ui.dummy([28.0, 22.0]);
}

fn popup_frame(
    context: &mut Context,
    animations: &mut UiAnimations,
    delta: f32,
    open: bool,
    close: bool,
) -> FrameGeometry {
    let binding = context.binding();
    context.prepare_frame(FramePrepareOptions::new([640.0, 480.0], delta));
    let ui = context.frame();
    animations.begin(ui, true);
    let mut ids = Vec::new();
    let mut popup = 0;
    ui.window("Popup test host")
        .position([10.0, 10.0], Condition::Always)
        .size([500.0, 400.0], Condition::Always)
        .flags(WindowFlags::NO_FOCUS_ON_APPEARING)
        .build(|| {
            animations.exclude_current(ui);
            if open {
                ui.open_popup("Animated popup");
            }
            ui.with_bound_context(|| unsafe {
                sys::igSetNextWindowPos(
                    [120.0, 100.0].into(),
                    sys::ImGuiCond_Always,
                    [0.0; 2].into(),
                );
                sys::igSetNextWindowSize([240.0, 180.0].into(), sys::ImGuiCond_Always);
            });
            if let Some(_popup) = ui.begin_popup("Animated popup") {
                popup = ui.with_bound_context(|| unsafe { (*sys::igGetCurrentWindowRead()).ID });
                ids.push(popup);
                marker(ui);
                ui.child_window("Clipped child")
                    .size([160.0, 90.0])
                    .border(true)
                    .build(ui, || {
                        ids.push(
                            ui.with_bound_context(|| unsafe {
                                (*sys::igGetCurrentWindowRead()).ID
                            }),
                        );
                        ui.with_clip_rect([140.0, 140.0], [220.0, 190.0], true, || marker(ui));
                        ui.text("Child content");
                    });
                if close {
                    ui.close_current_popup();
                }
            }
        });
    animations.end(ui, true);
    let before = geometry(&binding, &ids);
    drop(context.render_legacy());
    let after = geometry(&binding, &ids);
    FrameGeometry {
        popup,
        before,
        after,
    }
}

fn visible_frame(context: &mut Context, animations: &mut UiAnimations) -> FrameGeometry {
    for index in 0..4 {
        let frame = popup_frame(context, animations, 1.0 / 60.0, index == 0, false);
        if frame.before.len() == 2 && animations.state.animations.contains_key(&frame.popup) {
            return frame;
        }
    }
    panic!("popup and descendant must become visible");
}

fn marker_alphas(frame: &FrameGeometry) -> Vec<u8> {
    frame
        .after
        .iter()
        .flat_map(|window| window.vertices.iter())
        .filter(|vertex| vertex.col & 0x00ff_ffff == MARKER & 0x00ff_ffff)
        .map(|vertex| (vertex.col >> 24) as u8)
        .collect()
}

fn near(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
}

#[test]
fn popup_and_child_geometry_share_motion_while_native_layout_stays_fixed() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = context();
    let mut animations = UiAnimations::new(&mut context);
    let frame = visible_frame(&mut context, &mut animations);
    let animation = &animations.state.animations[&frame.popup];
    let now = context
        .binding()
        .with_bound_context(|| unsafe { (*sys::igGetCurrentContext()).Time });
    let progress = ((now - animation.began) / POPOVER_SECONDS) as f32;
    let alpha = 1.0 - (1.0 - progress).powi(3);
    let scale = INITIAL_SCALE + (1.0 - INITIAL_SCALE) * alpha;
    assert!(scale > INITIAL_SCALE && scale < 1.0);
    let popup = frame
        .before
        .iter()
        .find(|window| window.id == frame.popup)
        .unwrap();
    let anchor = animation.identity.anchor.map(f32::from_bits);
    let pivot = [
        anchor[0].clamp(popup.pos[0], popup.pos[0] + popup.size[0]),
        anchor[1].clamp(popup.pos[1], popup.pos[1] + popup.size[1]),
    ];
    for before in &frame.before {
        let after = frame
            .after
            .iter()
            .find(|window| window.id == before.id)
            .unwrap();
        assert_eq!(
            after.pos, before.pos,
            "visual motion must preserve hit testing"
        );
        assert_eq!(
            after.size, before.size,
            "visual motion must preserve layout"
        );
        assert_eq!(after.vertices.len(), before.vertices.len());
        assert_eq!(after.clips.len(), before.clips.len());
        assert!(!before.vertices.is_empty() && !before.clips.is_empty());
        for (old, new) in before.vertices.iter().zip(&after.vertices) {
            near(new.pos.x, pivot[0] + (old.pos.x - pivot[0]) * scale);
            near(new.pos.y, pivot[1] + (old.pos.y - pivot[1]) * scale);
            assert_eq!(new.uv, old.uv);
            assert_eq!(new.col & 0x00ff_ffff, old.col & 0x00ff_ffff);
            assert_eq!(
                new.col >> 24,
                ((old.col >> 24) as f32 * alpha).round() as u32
            );
        }
        for (old, new) in before.clips.iter().zip(&after.clips) {
            for component in 0..4 {
                let axis = component % 2;
                near(
                    new[component],
                    pivot[axis] + (old[component] - pivot[axis]) * scale,
                );
            }
        }
    }
}

#[test]
fn repeatedly_opening_an_existing_popup_settles_and_closing_does_not_flash() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = context();
    let mut animations = UiAnimations::new(&mut context);
    let mut frame = visible_frame(&mut context, &mut animations);
    assert!(marker_alphas(&frame).iter().any(|&alpha| alpha < 255));
    for _ in 0..20 {
        frame = popup_frame(&mut context, &mut animations, 1.0 / 60.0, true, false);
    }
    let before_close = animations.state.animations[&frame.popup].began;
    assert!(!marker_alphas(&frame).is_empty());
    assert!(marker_alphas(&frame).iter().all(|&alpha| alpha == 255));
    let frame = popup_frame(&mut context, &mut animations, 1.0 / 60.0, false, true);
    assert_eq!(
        animations.state.animations[&frame.popup].began,
        before_close
    );
    assert!(!marker_alphas(&frame).is_empty());
    assert!(marker_alphas(&frame).iter().all(|&alpha| alpha == 255));
    for (before, after) in frame.before.iter().zip(&frame.after) {
        assert_eq!(
            before.vertices, after.vertices,
            "closing cannot restart opening motion"
        );
    }
}

#[test]
fn text_input_settles_popup_growth_immediately() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut context = context();
    let mut animations = UiAnimations::new(&mut context);
    visible_frame(&mut context, &mut animations);
    context.io_mut().add_input_character('界');
    let frame = popup_frame(&mut context, &mut animations, 1.0 / 240.0, false, false);
    assert!(animations.state.animations[&frame.popup].grow_settled);
    assert!(
        marker_alphas(&frame).iter().any(|&alpha| alpha < 255),
        "input settles geometry while fade continues"
    );
    for (before, after) in frame.before.iter().zip(&frame.after) {
        for (old, new) in before.vertices.iter().zip(&after.vertices) {
            assert_eq!(
                old.pos, new.pos,
                "input must restore final geometry this frame"
            );
        }
        assert_eq!(before.clips, after.clips);
    }
}

#[test]
fn animation_hook_is_context_local_and_can_drop_before_or_after_its_context() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut owner = context();
    let animations = UiAnimations::new(&mut owner);
    let hook = animations.hook;
    // Context creation requires no native context to be active. Each safe
    // operation below binds its own still-live context through ContextBinding.
    unsafe { sys::igSetCurrentContext(std::ptr::null_mut()) };
    let mut other = context();
    other.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
    let ui = other.frame();
    let mut id = 0;
    ui.window("Unrelated context window")
        .size([200.0, 120.0], Condition::Always)
        .build(|| {
            id = ui.with_bound_context(|| unsafe { (*sys::igGetCurrentWindowRead()).ID });
            marker(ui);
        });
    let before = geometry(&other.binding(), &[id]);
    drop(other.render_legacy());
    let after = geometry(&other.binding(), &[id]);
    assert_eq!(before[0].vertices, after[0].vertices);
    assert!(animations.state.applied_frame.is_none());
    other.binding().with_bound_context(|| unsafe {
        let current = sys::igGetCurrentContext();
        drop(animations);
        assert_eq!(
            sys::igGetCurrentContext(),
            current,
            "cleanup restores the caller's context"
        );
    });
    owner.binding().with_bound_context(|| unsafe {
        let native = &*sys::igGetCurrentContext();
        for index in 0..native.Hooks.Size as usize {
            let candidate = &*native.Hooks.Data.add(index);
            assert!(
                candidate.HookId != hook
                    || candidate.Type == sys::ImGuiContextHookType_PendingRemoval_
            );
        }
    });
    // Starting a frame requires the owning context to be current explicitly.
    unsafe { sys::igSetCurrentContext(owner.as_raw()) };
    owner.prepare_frame(FramePrepareOptions::new([640.0, 480.0], 1.0 / 60.0));
    owner.frame().window("After hook removal").build(|| {});
    drop(owner.render_legacy());
    let animations = UiAnimations::new(&mut owner);
    drop(owner);
    drop(animations);
}

#[test]
fn popup_fade_uses_elapsed_time_across_frame_rates() {
    let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
    let mut samples = Vec::new();
    for rate in [30.0, 60.0, 120.0] {
        let mut context = context();
        let mut animations = UiAnimations::new(&mut context);
        let mut frame = popup_frame(&mut context, &mut animations, 1.0 / rate, true, false);
        for _ in 0..4 {
            if animations.state.animations.contains_key(&frame.popup) {
                break;
            }
            frame = popup_frame(&mut context, &mut animations, 1.0 / rate, false, false);
        }
        assert!(
            animations.state.animations.contains_key(&frame.popup),
            "popup must become visible"
        );
        let target = animations.state.animations[&frame.popup].began + POPOVER_SECONDS * 0.5;
        loop {
            let now = context
                .binding()
                .with_bound_context(|| unsafe { (*sys::igGetCurrentContext()).Time });
            let remaining = target - now;
            if remaining <= 0.000_001 {
                break;
            }
            frame = popup_frame(
                &mut context,
                &mut animations,
                remaining.min(1.0 / f64::from(rate)) as f32,
                false,
                false,
            );
        }
        let alphas = marker_alphas(&frame);
        assert!(!alphas.is_empty());
        assert!(alphas.iter().all(|&alpha| alpha == alphas[0]));
        samples.push(alphas[0]);
    }
    assert!(
        samples.iter().all(|&alpha| alpha.abs_diff(223) <= 1),
        "halfway fade must match at each rate: {samples:?}"
    );
}
