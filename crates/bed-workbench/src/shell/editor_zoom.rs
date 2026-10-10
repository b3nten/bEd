//! Text-view zoom commands and native gesture routing. Other panels do not opt in.
use super::*;
use crate::commands::EditorZoomCommand;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorPinchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

pub(super) struct EditorPinchEvent {
    viewport: u32,
    position: [f32; 2],
    delta: f64,
    phase: EditorPinchPhase,
}

impl Workbench {
    /// Queue native input for resolution inside the next UI frame. Positions
    /// use the same logical desktop coordinates as ImGui's mouse input.
    pub fn queue_editor_pinch(
        &mut self,
        viewport: u32,
        position: [f32; 2],
        delta: f64,
        phase: EditorPinchPhase,
    ) {
        self.pending_editor_pinches.push(EditorPinchEvent {
            viewport,
            position,
            delta,
            phase,
        });
    }

    pub(super) fn dispatch_editor_zoom(&mut self, command: EditorZoomCommand) -> bool {
        let Some(view) = self
            .tabs
            .iter_mut()
            .find(|tab| Some(tab.id) == self.focused)
            .and_then(|tab| tab.panel.editor_mut())
        else {
            return false;
        };
        match command {
            EditorZoomCommand::In => view.zoom_in(),
            EditorZoomCommand::Out => view.zoom_out(),
            EditorZoomCommand::Reset => view.reset_zoom(),
        }
        true
    }

    pub(super) fn editor_zoom_input(&mut self, ui: &Ui) {
        let blocked = self.active_overlay() != Overlay::None
            || self.file_dialog.is_some()
            || self.reload_confirmation.is_some()
            || self.file_operations.modal_visible()
            || self.modules.explorer.finder_visible()
            || ui.is_popup_open_with_flags(
                "",
                dear_imgui_rs::PopupQueryFlags::ANY_POPUP_ID
                    | dear_imgui_rs::PopupQueryFlags::ANY_POPUP_LEVEL,
            );
        if blocked {
            self.editor_pinch_target = None;
        }
        for event in std::mem::take(&mut self.pending_editor_pinches) {
            if event.phase == EditorPinchPhase::Started {
                self.editor_pinch_target =
                    if blocked || !event.position.iter().all(|value| value.is_finite()) {
                        None
                    } else {
                        // Resolve the gesture's starting position even if later
                        // pointer events were queued before this frame.
                        let mut hovered = std::ptr::null_mut();
                        unsafe {
                            sys::igFindHoveredWindowEx(
                                event.position.into(),
                                true,
                                &mut hovered,
                                std::ptr::null_mut(),
                            );
                        }
                        self.tabs.iter().find_map(|tab| {
                            let view = tab.panel.editor()?;
                            let layout = view.presentation().layout;
                            if tab.viewport != event.viewport
                                || event.position[0] < layout.pane_pos[0]
                                || event.position[0] >= layout.pane_pos[0] + layout.pane_size[0]
                                || event.position[1] < layout.pane_pos[1]
                                || event.position[1] >= layout.pane_pos[1] + layout.pane_size[1]
                            {
                                return None;
                            }
                            let name = CString::new(self.title(tab)).unwrap();
                            let owns_hover = unsafe {
                                let window = sys::igFindWindowByName(name.as_ptr());
                                !hovered.is_null()
                                    && !window.is_null()
                                    && (*hovered).RootWindow == window
                            };
                            owns_hover.then_some((event.viewport, view.id()))
                        })
                    };
            }
            let factor = (1.0 + event.delta) as f32;
            if !blocked
                && event.phase != EditorPinchPhase::Cancelled
                && factor.is_finite()
                && factor > 0.0
                && let Some((viewport, view_id)) = self.editor_pinch_target
                && viewport == event.viewport
                && let Some(view) = self
                    .tabs
                    .iter_mut()
                    .find_map(|tab| tab.panel.editor_mut().filter(|view| view.id() == view_id))
            {
                view.zoom_by(factor);
            }
            if matches!(
                event.phase,
                EditorPinchPhase::Ended | EditorPinchPhase::Cancelled
            ) && self
                .editor_pinch_target
                .is_some_and(|(viewport, _)| viewport == event.viewport)
            {
                self.editor_pinch_target = None;
            }
        }
        let primary = if cfg!(target_os = "macos") && !ui.io().config_macosx_behaviors() {
            ui.io().key_super()
        } else {
            ui.io().key_ctrl()
        };
        if blocked
            || ui.io().want_text_input()
            || !primary
            || ui.io().key_alt()
            || !self
                .tabs
                .iter()
                .any(|tab| Some(tab.id) == self.focused && tab.panel.editor().is_some())
        {
            return;
        }
        let zoom_in_held = ui.is_key_down(Key::Equal) || ui.is_key_down(Key::KeypadAdd);
        let zoom_out_held = !ui.io().key_shift()
            && (ui.is_key_down(Key::Minus) || ui.is_key_down(Key::KeypadSubtract));
        if zoom_in_held || zoom_out_held {
            // Native key repeats can queue text between ImGui's zoom repeats.
            // Consume the chord's characters while held, including at zoom limits.
            ui.with_bound_context(|| unsafe {
                // SAFETY: the active UI owns this Unicode queue. Compact it in
                // place without growing its allocation or retaining references.
                let queue = &mut (*sys::igGetIO_Nil()).InputQueueCharacters;
                if queue.Size > 0 {
                    let characters =
                        std::slice::from_raw_parts_mut(queue.Data, queue.Size as usize);
                    let mut kept = 0;
                    for index in 0..characters.len() {
                        let codepoint = characters[index];
                        if (zoom_in_held && (codepoint == '=' as u32 || codepoint == '+' as u32))
                            || (zoom_out_held && codepoint == '-' as u32)
                        {
                            continue;
                        }
                        characters[kept] = codepoint;
                        kept += 1;
                    }
                    queue.Size = kept as i32;
                }
            });
        }
        let command = if ui.is_key_pressed(Key::Equal) || ui.is_key_pressed(Key::KeypadAdd) {
            Some(EditorZoomCommand::In)
        } else if !ui.io().key_shift()
            && (ui.is_key_pressed(Key::Minus) || ui.is_key_pressed(Key::KeypadSubtract))
        {
            Some(EditorZoomCommand::Out)
        } else {
            None
        };
        if let Some(command) = command {
            self.dispatch_editor_zoom(command);
        }
    }
}
