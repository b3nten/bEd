//! Translated from ned lsp/lsp_uri_options.{h,cpp}; see LICENSE and NOTICE.
//! Selection returns an action; the host opens a document after releasing the
//! client borrow and then converts its UTF-16 position to a byte column.
use crate::{presentation::LspPresentationOptions, views::view_layout::ViewLayout};
use bed_lsp::lsp_locations::LspLocation;
use dear_imgui_rs::{
    Condition, FocusedFlags, Key, MouseButton, StyleColor, StyleVar, Ui, WindowFlags,
    WindowHoveredFlags,
};

#[derive(Default)]
pub struct LspUriOptions {
    selected_index: usize,
}

fn geometry(
    fs: f32,
    item_height: f32,
    text_height: f32,
    count: usize,
    display: [f32; 2],
    embedded_pane: Option<([f32; 2], [f32; 2])>,
) -> ([f32; 2], [f32; 2], f32, f32, f32) {
    let padding = fs * 0.8;
    let title_height = item_height + text_height * 0.4 + fs * 0.2;
    let footer_height = item_height + padding;
    let total_height =
        title_height + item_height * count.max(1) as f32 + footer_height + padding * 2.0;
    let size = [
        (fs * 30.0).min(display[0] * 0.9),
        total_height.min(display[1] * 0.5) + if count <= 1 { fs * 0.5 } else { fs * 1.25 },
    ];
    let position = if let Some((pane_pos, pane_size)) = embedded_pane {
        let mut position = [
            pane_pos[0] + pane_size[0] * 0.5 - size[0] * 0.5,
            pane_pos[1] + pane_size[1] * 0.35 - size[1] * 0.5,
        ];
        for axis in 0..2 {
            if position[axis] < pane_pos[axis] {
                position[axis] = pane_pos[axis];
            }
            if position[axis] + size[axis] > pane_pos[axis] + pane_size[axis] {
                position[axis] = pane_pos[axis] + pane_size[axis] - size[axis];
            }
        }
        position
    } else {
        [
            display[0] * 0.5 - size[0] * 0.5,
            display[1] * 0.35 - size[1] * 0.5,
        ]
    };
    (position, size, padding, title_height, footer_height)
}

impl LspUriOptions {
    /// Draw result content in an existing dock window. Keyboard navigation
    /// applies only while this result window or one of its children has focus.
    pub fn render_body(
        &mut self,
        ui: &Ui,
        title: &str,
        options: &[LspLocation],
        pending: bool,
    ) -> Option<LspLocation> {
        if self.selected_index >= options.len() {
            self.selected_index = 0;
        }
        let keyboard = ui.is_window_focused_with_flags(FocusedFlags::CHILD_WINDOWS);
        ui.text(format!("{title} ({})", options.len()));
        ui.separator();
        let mut selected = None;
        ui.child_window("##ContentScroll")
            .size([0.0, -ui.frame_height_with_spacing()])
            .flags(WindowFlags::HORIZONTAL_SCROLLBAR | WindowFlags::ALWAYS_VERTICAL_SCROLLBAR)
            .build(ui, || {
                if pending {
                    ui.text("Loading...");
                } else if options.is_empty() {
                    ui.text("No results available");
                } else {
                    selected = self.render_rows(ui, options, keyboard);
                }
            });
        ui.separator();
        ui.text("Up/Down Enter");
        if keyboard
            && !pending
            && !options.is_empty()
            && (ui.is_key_pressed(Key::Enter) || ui.is_key_pressed(Key::KeypadEnter))
        {
            selected = Some(options[self.selected_index].clone());
        }
        selected
    }
    fn render_rows(
        &mut self,
        ui: &Ui,
        options: &[LspLocation],
        keyboard: bool,
    ) -> Option<LspLocation> {
        let mut selected = None;
        if keyboard && !ui.is_any_item_active() {
            if ui.is_key_pressed(Key::UpArrow) {
                self.selected_index = if self.selected_index > 0 {
                    self.selected_index - 1
                } else {
                    options.len() - 1
                };
                ui.set_scroll_here_y(0.0);
            }
            if ui.is_key_pressed(Key::DownArrow) {
                self.selected_index = (self.selected_index + 1) % options.len();
                ui.set_scroll_here_y(1.0);
            }
        }
        for (index, option) in options.iter().enumerate() {
            let is_selected = self.selected_index == index;
            let filename = option
                .file
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&option.file);
            let label = format!(
                "{filename}:{}:{}",
                option.line.wrapping_add(1),
                option.character.wrapping_add(1)
            );
            let _hover = ui.push_style_color(
                StyleColor::HeaderHovered,
                if is_selected {
                    ui.style_color(StyleColor::TextSelectedBg)
                } else {
                    [0.0; 4]
                },
            );
            if ui
                .selectable_config(&label)
                .selected(is_selected)
                .allow_double_click(true)
                .span_all_columns(true)
                .close_popups(false)
                .build()
            {
                self.selected_index = index;
                if ui.is_mouse_double_clicked(MouseButton::Left) {
                    selected = Some(options[self.selected_index].clone());
                }
            }
            if is_selected
                && ((keyboard && ui.is_key_pressed(Key::UpArrow))
                    || (keyboard && ui.is_key_pressed(Key::DownArrow))
                    || ui.is_window_appearing())
            {
                ui.set_scroll_here_y(0.5);
            }
        }
        selected
    }
    pub fn render(
        &mut self,
        ui: &Ui,
        title: &str,
        options: &[LspLocation],
        show: &mut bool,
        settings: &LspPresentationOptions,
        layout: &ViewLayout,
    ) -> Option<LspLocation> {
        if !*show {
            return None;
        }
        if self.selected_index >= options.len() {
            self.selected_index = 0;
        }
        let fs = ui.current_font_size();
        let embedded = settings
            .embedded
            .then_some((layout.pane_pos, layout.pane_size));
        let (position, size, padding, title_height, footer_height) = geometry(
            fs,
            ui.text_line_height_with_spacing(),
            ui.text_line_height(),
            options.len(),
            ui.io().display_size(),
            embedded,
        );
        let bg = settings.background_color();
        let mut bg = bed_core::util::color::blend(ui.style_color(StyleColor::Text), bg, 0.055);
        bg[3] = 1.0;
        let _padding = ui.push_style_var(StyleVar::WindowPadding([padding; 2]));
        let _round = ui.push_style_var(StyleVar::WindowRounding(fs * 0.5));
        let _border_size = ui.push_style_var(StyleVar::WindowBorderSize(1.0));
        let _spacing = ui.push_style_var(StyleVar::ItemSpacing([fs * 0.4; 2]));
        let _window_bg = ui.push_style_color(StyleColor::WindowBg, bg);
        let _child_bg = ui.push_style_color(StyleColor::ChildBg, bg);
        let _border = ui.push_style_color(StyleColor::Border, ui.style_color(StyleColor::Border));
        let _frame_bg = ui.push_style_color(StyleColor::FrameBg, bg);
        let _header = ui.push_style_color(
            StyleColor::Header,
            ui.style_color(StyleColor::TextSelectedBg),
        );
        let _hover = ui.push_style_color(StyleColor::HeaderHovered, [0.0; 4]);
        let _active = ui.push_style_color(
            StyleColor::HeaderActive,
            ui.style_color(StyleColor::HeaderActive),
        );
        let mut selected = None;
        let rendered = ui
            .window("##LSPUriOptions")
            .position(position, Condition::Always)
            .size(size, Condition::Always)
            .flags(
                WindowFlags::NO_TITLE_BAR
                    | WindowFlags::NO_RESIZE
                    | WindowFlags::NO_MOVE
                    | WindowFlags::NO_SCROLLBAR,
            )
            .build(|| {
                if ui.is_mouse_clicked(MouseButton::Left) {
                    let mouse = ui.io().mouse_pos();
                    let pos = ui.window_pos();
                    let size = ui.window_size();
                    if !ui.is_window_hovered_with_flags(
                        WindowHoveredFlags::ALLOW_WHEN_BLOCKED_BY_POPUP,
                    ) && (mouse[0] < pos[0]
                        || mouse[0] > pos[0] + size[0]
                        || mouse[1] < pos[1]
                        || mouse[1] > pos[1] + size[1])
                    {
                        *show = false;
                    }
                }
                if ui.is_key_pressed(Key::Escape) {
                    *show = false;
                }
                ui.child_window("##Header")
                    .size([0.0, title_height])
                    .build(ui, || {
                        ui.text(format!("{title} ({})", options.len()));
                        ui.separator();
                    });
                if options.is_empty() {
                    // Pending requests have this same label, as in the original.
                    ui.text("No results available");
                } else {
                    let height = size[1] - title_height - footer_height - padding * 2.0;
                    ui.child_window("##ContentScroll")
                        .size([0.0, height])
                        .flags(
                            WindowFlags::HORIZONTAL_SCROLLBAR
                                | WindowFlags::ALWAYS_VERTICAL_SCROLLBAR,
                        )
                        .build(ui, || {
                            selected = self.render_rows(ui, options, true);
                            if selected.is_some() {
                                *show = false;
                            }
                        });
                }
                ui.child_window("##Footer")
                    .size([0.0, footer_height])
                    .build(ui, || {
                        ui.separator();
                        ui.text("Up/Down Enter");
                    });
                if (ui.is_key_pressed(Key::Enter) || ui.is_key_pressed(Key::KeypadEnter))
                    && !options.is_empty()
                {
                    selected = Some(options[self.selected_index].clone());
                    *show = false;
                }
            });
        if rendered.is_none() {
            *show = false;
        }
        if !*show {
            self.selected_index = 0;
        }
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use dear_imgui_rs::{Context, FramePrepareOptions};

    fn context() -> Context {
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context
    }
    fn settings(_directory: &TempDir) -> LspPresentationOptions {
        LspPresentationOptions {
            embedded: false,
            ..Default::default()
        }
    }
    fn render(
        context: &mut Context,
        picker: &mut LspUriOptions,
        options: &[LspLocation],
        show: &mut bool,
        settings: &LspPresentationOptions,
    ) -> Option<LspLocation> {
        context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
        let selected = picker.render(
            context.frame(),
            "Goto Definition",
            options,
            show,
            settings,
            &ViewLayout::default(),
        );
        drop(context.render_legacy());
        selected
    }
    fn options() -> Vec<LspLocation> {
        vec![
            LspLocation {
                file: "/tmp/a.rs".into(),
                line: 2,
                character: 4,
            },
            LspLocation {
                file: "C:\\path\\b.rs".into(),
                line: 7,
                character: 1,
            },
        ]
    }
    #[test]
    fn native_docked_body_keeps_host_window_and_requires_focus_for_keys() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = context();
        let mut picker = LspUriOptions::default();
        let options = options();
        let draw =
            |context: &mut Context, picker: &mut LspUriOptions, focused: bool, pending: bool| {
                context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
                let ui = context.frame();
                let mut selection = None;
                ui.window("Host results")
                    .position([30.0, 40.0], Condition::Always)
                    .size([420.0, 320.0], Condition::Always)
                    .focused(focused)
                    .build(|| {
                        let before = (
                            ui.window_pos(),
                            ui.window_size(),
                            ui.style_color(StyleColor::WindowBg),
                        );
                        selection = picker.render_body(ui, "References", &options, pending);
                        assert_eq!(
                            before,
                            (
                                ui.window_pos(),
                                ui.window_size(),
                                ui.style_color(StyleColor::WindowBg)
                            )
                        );
                    });
                if !focused {
                    ui.window("Other host pane")
                        .size([200.0, 100.0], Condition::Always)
                        .focused(true)
                        .build(|| ui.text("Host input"));
                }
                ui.with_bound_context(|| unsafe {
                    assert!(
                        dear_imgui_rs::sys::igFindWindowByName(c"##LSPUriOptions".as_ptr())
                            .is_null()
                    );
                });
                drop(context.render_legacy());
                selection
            };
        draw(&mut context, &mut picker, true, false);
        draw(&mut context, &mut picker, true, false);
        context.io_mut().add_key_event(Key::UpArrow, true);
        draw(&mut context, &mut picker, true, false);
        assert_eq!(picker.selected_index, 1);
        context.io_mut().add_key_event(Key::UpArrow, false);
        draw(&mut context, &mut picker, true, false);
        context.io_mut().add_key_event(Key::Enter, true);
        assert_eq!(
            draw(&mut context, &mut picker, true, false),
            Some(options[1].clone())
        );
        context.io_mut().clear_input_keys();
        draw(&mut context, &mut picker, false, false);
        context.io_mut().add_key_event(Key::DownArrow, true);
        assert!(draw(&mut context, &mut picker, false, false).is_none());
        assert_eq!(picker.selected_index, 1);
        context.io_mut().clear_input_keys();
        context.io_mut().add_key_event(Key::Enter, true);
        assert!(draw(&mut context, &mut picker, true, true).is_none());
    }
    #[test]
    fn native_picker_arrows_wrap_enter_selects_and_escape_dismisses() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let directory = TempDir::new();
        let settings = settings(&directory);
        let mut context = context();
        let mut picker = LspUriOptions::default();
        let options = options();
        let mut show = true;
        render(&mut context, &mut picker, &options, &mut show, &settings);
        render(&mut context, &mut picker, &options, &mut show, &settings);
        context.io_mut().add_key_event(Key::UpArrow, true);
        assert!(render(&mut context, &mut picker, &options, &mut show, &settings).is_none());
        assert_eq!(picker.selected_index, 1);
        context.io_mut().add_key_event(Key::UpArrow, false);
        render(&mut context, &mut picker, &options, &mut show, &settings);
        context.io_mut().add_key_event(Key::DownArrow, true);
        render(&mut context, &mut picker, &options, &mut show, &settings);
        assert_eq!(picker.selected_index, 0);
        context.io_mut().add_key_event(Key::DownArrow, false);
        render(&mut context, &mut picker, &options, &mut show, &settings);
        context.io_mut().add_key_event(Key::KeypadEnter, true);
        assert_eq!(
            render(&mut context, &mut picker, &options, &mut show, &settings),
            Some(options[0].clone())
        );
        assert!(!show);
        assert_eq!(picker.selected_index, 0);
        context.io_mut().clear_input_keys();
        show = true;
        render(&mut context, &mut picker, &options, &mut show, &settings);
        context.io_mut().add_key_event(Key::Escape, true);
        assert!(render(&mut context, &mut picker, &options, &mut show, &settings).is_none());
        assert!(!show);
    }
    #[test]
    fn native_empty_picker_stays_open_on_enter_and_outside_click_closes() {
        let _context_lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let directory = TempDir::new();
        let settings = settings(&directory);
        let mut context = context();
        let mut picker = LspUriOptions::default();
        let mut show = true;
        render(&mut context, &mut picker, &[], &mut show, &settings);
        context.io_mut().add_key_event(Key::Enter, true);
        assert!(render(&mut context, &mut picker, &[], &mut show, &settings).is_none());
        assert!(show);
        context.io_mut().clear_input_keys();
        context.io_mut().add_mouse_pos_event([0.0, 0.0]);
        context
            .io_mut()
            .add_mouse_button_event(MouseButton::Left, true);
        render(&mut context, &mut picker, &[], &mut show, &settings);
        assert!(!show);
    }
    #[test]
    fn pane_geometry_retains_sequential_clamps_and_display_based_limits() {
        let (pos, size, padding, title, footer) = geometry(
            20.0,
            24.0,
            20.0,
            30,
            [1000.0, 800.0],
            Some(([100.0, 50.0], [200.0, 150.0])),
        );
        assert_eq!(size, [600.0, 425.0]);
        assert_eq!(pos, [-300.0, -225.0]);
        assert_eq!((padding, title, footer), (16.0, 36.0, 40.0));
    }
}
