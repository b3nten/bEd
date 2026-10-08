//! Desktop-only opening motion, applied before ImGui assembles viewport draw data.
//!
//! Layout and input stay in ImGui's final coordinates. Editing the finished
//! vertices also fades Bed's custom editor/terminal painters, which bypass style
//! alpha. No native pointers are retained between frames.
use dear_imgui_rs::{Context, ContextBinding, Ui, sys};
use std::collections::{HashMap, HashSet};

const PANEL_SECONDS: f64 = 0.300;
const POPOVER_SECONDS: f64 = 0.150;
const INITIAL_SCALE: f32 = 0.96;

pub(crate) struct UiAnimations {
    binding: ContextBinding,
    hook: u32,
    // The callback's userdata must keep its address when Workbench moves.
    state: Box<State>,
}

impl UiAnimations {
    pub(crate) fn new(context: &mut Context) -> Self {
        let binding = context.binding();
        let mut state = Box::<State>::default();
        let hook = sys::ImGuiContextHook {
            Type: sys::ImGuiContextHookType_RenderPre,
            Callback: Some(render_pre),
            UserData: (&mut *state as *mut State).cast(),
            ..Default::default()
        };
        let hook = binding.with_bound_context(|| unsafe {
            // The owned box outlives the installed hook; Drop removes it first.
            sys::igAddContextHook(sys::igGetCurrentContext(), &hook)
        });
        Self {
            binding,
            hook,
            state,
        }
    }

    pub(crate) fn begin(&mut self, ui: &Ui, enabled: bool) {
        debug_assert_eq!(ui.context_id(), self.binding.id());
        self.state.frame = ui.frame_count() as i32;
        self.state.enabled = enabled;
        self.state.panels.clear();
        self.state.excluded.clear();
        self.state.text_input = ui
            .with_bound_context(|| unsafe { (*sys::igGetIO_Nil()).InputQueueCharacters.Size > 0 });
        self.state.first_order =
            ui.with_bound_context(|| unsafe { (*sys::igGetCurrentContext()).WindowsActiveCount });
        // A fallible render that never calls end must not capture host windows.
        self.state.end_order = self.state.first_order;
    }

    pub(crate) fn exclude_current(&mut self, ui: &Ui) {
        let id = ui.with_bound_context(|| unsafe { (*sys::igGetCurrentWindowRead()).ID });
        self.state.excluded.insert(id);
    }

    pub(crate) fn register_panel(&mut self, ui: &Ui) {
        let (id, first_vertex) = ui.with_bound_context(|| unsafe {
            let window = &*sys::igGetCurrentWindowRead();
            (window.ID, (*window.DrawList).VtxBuffer.Size as usize)
        });
        // Begin already drew the background and decorations. Fade only content.
        self.state.panels.entry(id).or_insert(first_vertex);
    }

    pub(crate) fn end(&mut self, ui: &Ui, enabled: bool) {
        self.state.enabled = enabled;
        self.state.end_order =
            ui.with_bound_context(|| unsafe { (*sys::igGetCurrentContext()).WindowsActiveCount });
    }
}

impl Drop for UiAnimations {
    fn drop(&mut self) {
        let _ = self.binding.try_with_bound_context(|| unsafe {
            // Removal immediately disables the callback. If the context has
            // already been destroyed, the binding declines to enter it.
            sys::igRemoveContextHook(sys::igGetCurrentContext(), self.hook);
        });
    }
}

#[derive(Default)]
struct State {
    frame: i32,
    first_order: i32,
    end_order: i32,
    applied_frame: Option<i32>,
    enabled: bool,
    text_input: bool,
    panels: HashMap<u32, usize>,
    excluded: HashSet<u32>,
    animations: HashMap<u32, Animation>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Panel,
    Popover,
    Dialog,
}

impl Kind {
    fn duration(self) -> f64 {
        if self == Self::Panel {
            PANEL_SECONDS
        } else {
            POPOVER_SECONDS
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Identity {
    popup: u32,
    parent: u32,
    source: u32,
    anchor: [u32; 2],
}

struct Animation {
    identity: Identity,
    kind: Kind,
    began: f64,
    last_visible: i32,
    open_frame: i32,
    grow_settled: bool,
}

#[derive(Clone, Copy)]
struct Effect {
    alpha: f32,
    scale: f32,
    pivot: [f32; 2],
}

struct Window {
    id: u32,
    parent: u32,
    flags: i32,
    draw_list: *mut sys::ImDrawList,
    pos: [f32; 2],
    size: [f32; 2],
    identity: Identity,
    open_frame: i32,
}

unsafe extern "C" fn render_pre(context: *mut sys::ImGuiContext, hook: *mut sys::ImGuiContextHook) {
    // SAFETY: ImGui calls synchronously on the context's thread. The owned box
    // stays valid until the hook is removed; no safe references overlap this call.
    unsafe {
        let state = &mut *((*hook).UserData.cast::<State>());
        state.apply(&*context);
    }
}

impl State {
    unsafe fn apply(&mut self, context: &sys::ImGuiContext) {
        let frame = context.FrameCount;
        if self.frame != frame
            || self.applied_frame == Some(frame)
            || self.end_order <= self.first_order
        {
            return;
        }
        self.applied_frame = Some(frame);
        let mut windows = Vec::new();
        // SAFETY: all native vectors and windows belong to this live context;
        // their pointers are consumed only within this callback.
        unsafe {
            for index in 0..context.Windows.Size {
                let window = &**context.Windows.Data.add(index as usize);
                if !window.Active
                    || window.Hidden
                    || window.Collapsed
                    || (window.DockIsActive() && !window.DockTabIsVisible())
                    || i32::from(window.BeginOrderWithinContext) < self.first_order
                    || i32::from(window.BeginOrderWithinContext) >= self.end_order
                {
                    continue;
                }
                let parent = if window.ParentWindow.is_null() {
                    0
                } else {
                    (*window.ParentWindow).ID
                };
                let owner = if window.ParentWindowInBeginStack.is_null() {
                    0
                } else {
                    (*window.ParentWindowInBeginStack).ID
                };
                let mut identity = Identity {
                    popup: window.PopupId,
                    parent: owner,
                    ..Default::default()
                };
                let mut open_frame = -1;
                for popup_index in 0..context.OpenPopupStack.Size {
                    let popup = &*context.OpenPopupStack.Data.add(popup_index as usize);
                    if std::ptr::eq(popup.Window, window) {
                        identity.popup = popup.PopupId;
                        identity.parent = popup.OpenParentId;
                        identity.anchor = [
                            popup.OpenPopupPos.x.to_bits(),
                            popup.OpenPopupPos.y.to_bits(),
                        ];
                        open_frame = popup.OpenFrameCount;
                        break;
                    }
                }
                if window.Flags & sys::ImGuiWindowFlags_Popup != 0 && open_frame < 0 {
                    // CloseCurrentPopup removes the stack entry immediately,
                    // but the already submitted window still renders this frame.
                    // Preserve its identity so selecting an item cannot flash it.
                    if let Some(previous) = self.animations.get(&window.ID) {
                        identity = previous.identity;
                        open_frame = previous.open_frame;
                    }
                }
                if window.Flags & sys::ImGuiWindowFlags_Tooltip != 0 {
                    // Widget tooltips reuse one native window. Custom editor
                    // hover targets already hide/rearm when their subject moves.
                    identity.source = context.HoveredId;
                    if identity.source == 0 {
                        identity.source = self
                            .animations
                            .get(&window.ID)
                            .filter(|animation| animation.identity.parent == owner)
                            .map_or(0, |animation| animation.identity.source);
                    }
                }
                windows.push(Window {
                    id: window.ID,
                    parent,
                    flags: window.Flags,
                    draw_list: window.DrawList,
                    pos: [window.Pos.x, window.Pos.y],
                    size: [window.Size.x, window.Size.y],
                    identity,
                    open_frame,
                });
            }
        }
        let mut effects = HashMap::new();
        for window in &windows {
            if self.excluded.contains(&window.id)
                || window.flags & sys::ImGuiWindowFlags_DockNodeHost != 0
            {
                continue;
            }
            let kind = if self.panels.contains_key(&window.id) {
                Kind::Panel
            } else if window.flags & sys::ImGuiWindowFlags_Modal != 0 {
                Kind::Dialog
            } else if window.flags & (sys::ImGuiWindowFlags_Popup | sys::ImGuiWindowFlags_Tooltip)
                != 0
                || window.flags & sys::ImGuiWindowFlags_ChildWindow == 0
            {
                Kind::Popover
            } else {
                continue;
            };
            effects.insert(window.id, self.effect(context, window, kind));
        }
        let parents: HashMap<_, _> = windows
            .iter()
            .map(|window| (window.id, window.parent))
            .collect();
        let mut transformed = HashSet::new();
        for window in &windows {
            let mut root = window.id;
            // Each transient has its own effect; child windows inherit their
            // closest animated ancestor without double fading or scaling.
            for _ in 0..windows.len() {
                if effects.contains_key(&root) || self.excluded.contains(&root) {
                    break;
                }
                let Some(&parent) = parents.get(&root) else {
                    break;
                };
                if parent == 0 || parent == root {
                    break;
                }
                root = parent;
            }
            let Some(&effect) = effects.get(&root) else {
                continue;
            };
            if window.draw_list.is_null() || !transformed.insert(window.draw_list as usize) {
                continue;
            }
            let first_vertex = if root == window.id {
                self.panels.get(&root).copied().unwrap_or(0)
            } else {
                0
            };
            unsafe {
                transform(window.draw_list, first_vertex, effect);
            }
        }
        self.animations
            .retain(|_, animation| animation.last_visible >= frame - 2);
    }

    fn effect(&mut self, context: &sys::ImGuiContext, window: &Window, kind: Kind) -> Effect {
        let frame = context.FrameCount;
        let now = context.Time;
        let animation = self.animations.entry(window.id).or_insert(Animation {
            identity: window.identity,
            kind,
            began: now,
            last_visible: frame - 2,
            open_frame: window.open_frame,
            grow_settled: false,
        });
        // Consecutive OpenPopup calls can refresh OpenFrameCount each frame.
        let reopened = window.open_frame >= 0
            && animation.open_frame >= 0
            && window.open_frame != animation.open_frame
            && window.open_frame != animation.open_frame + 1;
        let appearing = animation.last_visible < frame - 1
            || animation.identity != window.identity
            || animation.kind != kind
            || reopened;
        if appearing {
            animation.began = now - f64::from(context.IO.DeltaTime.min(1.0 / 60.0));
            animation.grow_settled = false;
        } else if kind == Kind::Popover && interacted(context, window, self.text_input) {
            animation.grow_settled = true;
        }
        animation.identity = window.identity;
        animation.kind = kind;
        animation.last_visible = frame;
        animation.open_frame = window.open_frame;
        if !self.enabled {
            animation.began = now - kind.duration();
            animation.grow_settled = true;
        }
        let progress = ((now - animation.began) / kind.duration()).clamp(0.0, 1.0) as f32;
        let alpha = 1.0 - (1.0 - progress).powi(3);
        let scale = if kind == Kind::Popover && !animation.grow_settled {
            INITIAL_SCALE + (1.0 - INITIAL_SCALE) * alpha
        } else {
            1.0
        };
        let pivot = if window.flags & sys::ImGuiWindowFlags_Popup != 0 {
            let anchor = window.identity.anchor.map(f32::from_bits);
            [
                anchor[0].clamp(window.pos[0], window.pos[0] + window.size[0]),
                anchor[1].clamp(window.pos[1], window.pos[1] + window.size[1]),
            ]
        } else {
            [window.pos[0] + window.size[0] * 0.5, window.pos[1]]
        };
        Effect {
            alpha,
            scale,
            pivot,
        }
    }
}

fn interacted(context: &sys::ImGuiContext, window: &Window, text_input: bool) -> bool {
    let io = &context.IO;
    let mouse_inside = io.MousePos.x >= window.pos[0]
        && io.MousePos.x < window.pos[0] + window.size[0]
        && io.MousePos.y >= window.pos[1]
        && io.MousePos.y < window.pos[1] + window.size[1];
    (mouse_inside
        && (io.MouseDelta.x != 0.0
            || io.MouseDelta.y != 0.0
            || io.MouseClicked.iter().any(|&clicked| clicked)
            || io.MouseReleased.iter().any(|&released| released)))
        || text_input
        || io
            .KeysData
            .iter()
            .any(|key| key.Down && key.DownDuration == 0.0)
}

unsafe fn transform(list: *mut sys::ImDrawList, first_vertex: usize, effect: Effect) {
    if effect.alpha == 1.0 && effect.scale == 1.0 {
        return;
    }
    // SAFETY: the live RenderPre hook owns access to each deduplicated list.
    unsafe {
        sys::ImDrawList_ChannelsMerge(list);
        let list = &mut *list;
        for index in first_vertex..list.VtxBuffer.Size as usize {
            let vertex = &mut *list.VtxBuffer.Data.add(index);
            let alpha = ((vertex.col >> 24) as f32 * effect.alpha).round() as u32;
            vertex.col = (vertex.col & 0x00ff_ffff) | (alpha << 24);
            if effect.scale != 1.0 {
                vertex.pos.x = effect.pivot[0] + (vertex.pos.x - effect.pivot[0]) * effect.scale;
                vertex.pos.y = effect.pivot[1] + (vertex.pos.y - effect.pivot[1]) * effect.scale;
            }
        }
        if effect.scale != 1.0 {
            for index in 0..list.CmdBuffer.Size as usize {
                let clip = &mut (*list.CmdBuffer.Data.add(index)).ClipRect;
                clip.x = effect.pivot[0] + (clip.x - effect.pivot[0]) * effect.scale;
                clip.y = effect.pivot[1] + (clip.y - effect.pivot[1]) * effect.scale;
                clip.z = effect.pivot[0] + (clip.z - effect.pivot[0]) * effect.scale;
                clip.w = effect.pivot[1] + (clip.w - effect.pivot[1]) * effect.scale;
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/workbench/presentation/ui_animation_popup_tests.rs"]
mod tests;
