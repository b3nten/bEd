//! One prepared presentation snapshot per ImGui context and frame.
use crate::{animation::UiAnimations, appearance::UiTheme, font::UiFonts};
use dear_imgui_rs::{Context, ContextBinding, FontId, Ui};
use std::{cell::RefCell, io, sync::Arc};

/// Cheap immutable snapshot. Clone it before borrowing an ECS world mutably.
#[derive(Clone)]
pub struct UiFrameContext {
    binding: ContextBinding,
    theme: Arc<UiTheme>,
    pub main_font: Option<FontId>,
    pub font_generation: u64,
}

thread_local! {
    // Only drawing scopes live here; no context or mutable runtime is retained.
    static DRAWING: RefCell<Vec<UiFrameContext>> = const { RefCell::new(Vec::new()) };
}
impl UiFrameContext {
    pub fn theme(&self) -> &UiTheme {
        &self.theme
    }
    pub fn assert_matches(&self, ui: &Ui) {
        assert_eq!(
            self.binding.id(),
            ui.context_id(),
            "UI presentation belongs to another ImGui context"
        );
    }
    /// Resolve the current draw's prepared snapshot. Direct ImGui users without
    /// a runtime inherit native colors and the common default control metrics.
    pub fn for_ui(ui: &Ui) -> Self {
        if let Some(context) = DRAWING.with(|drawing| {
            drawing
                .borrow()
                .iter()
                .rev()
                .find(|context| context.binding.id() == ui.context_id())
                .cloned()
        }) {
            return context;
        }
        let mut theme = UiTheme::default();
        theme.fonts.size = ui.current_font_size();
        use dear_imgui_rs::StyleColor;
        theme.palette.background = ui.style_color(StyleColor::WindowBg);
        theme.palette.window_background = theme.palette.background;
        theme.palette.tab_bar = ui.style_color(StyleColor::MenuBarBg);
        theme.palette.surface = ui.style_color(StyleColor::PopupBg);
        theme.palette.text = ui.style_color(StyleColor::Text);
        theme.palette.accent = ui.style_color(StyleColor::CheckMark);
        theme.palette.selection = ui.style_color(StyleColor::TextSelectedBg);
        Self {
            binding: ui.binding(),
            theme: Arc::new(theme),
            main_font: None,
            font_generation: 0,
        }
    }
    /// Scope a complete frontend draw, including footer and independent popups.
    pub fn scope<R>(&self, draw: impl FnOnce() -> R) -> R {
        DRAWING.with(|drawing| drawing.borrow_mut().push(self.clone()));
        struct Scope;
        impl Drop for Scope {
            fn drop(&mut self) {
                DRAWING.with(|drawing| {
                    drawing.borrow_mut().pop().expect("UI drawing scope");
                });
            }
        }
        let _scope = Scope;
        draw()
    }
}

/// Main-thread context lifecycle. The desktop owns one; components only read its
/// prepared snapshots and never rebuild fonts or mutate another component.
pub struct UiRuntime {
    binding: ContextBinding,
    prepared: UiFrameContext,
    animations: UiAnimations,
}
impl UiRuntime {
    pub fn new(context: &Context) -> Self {
        let binding = context.binding();
        Self {
            prepared: UiFrameContext {
                binding: binding.clone(),
                theme: Arc::new(UiTheme::default()),
                main_font: None,
                font_generation: 0,
            },
            animations: UiAnimations::new(binding.clone()),
            binding,
        }
    }
    pub fn prepare(
        &mut self,
        context: &mut Context,
        requested: &UiTheme,
        fonts: &mut UiFonts,
    ) -> io::Result<()> {
        assert_eq!(
            context.binding().id(),
            self.binding.id(),
            "UI runtime context identity"
        );
        validate(requested)?;
        fonts.apply(context, &requested.fonts, false)?;
        crate::appearance::apply_style(context, requested, false);
        if &*self.prepared.theme != requested {
            self.prepared.theme = Arc::new(requested.clone());
        }
        self.prepared.main_font = fonts.main;
        self.prepared.font_generation = fonts.generation;
        Ok(())
    }
    pub fn frame_context(&self) -> UiFrameContext {
        self.prepared.clone()
    }
    pub fn begin(&mut self, ui: &Ui) {
        self.animations.begin(ui, &self.prepared.theme.motion);
    }
    pub fn end(&mut self, ui: &Ui) {
        self.animations.end(ui, self.prepared.theme.motion.enabled);
    }
    pub fn register_panel(&mut self, ui: &Ui) {
        self.animations.register_panel(ui);
    }
    pub fn exclude_current(&mut self, ui: &Ui) {
        self.animations.exclude_current(ui);
    }
    pub fn matches_context(&self, ui: &Ui) -> bool {
        self.binding.id() == ui.context_id()
    }
}

fn validate(theme: &UiTheme) -> io::Result<()> {
    let metrics = &theme.metrics;
    let dimensions = metrics
        .control_padding
        .into_iter()
        .chain(metrics.control_spacing)
        .chain(metrics.compact_padding)
        .chain(metrics.compact_spacing)
        .chain(metrics.tooltip_padding)
        .chain(metrics.menu_padding)
        .chain(metrics.menu_spacing)
        .chain(metrics.dialog_padding)
        .chain([
            metrics.control_rounding,
            metrics.border,
            metrics.popup_rounding,
            metrics.dialog_rounding,
            metrics.grab_rounding,
            metrics.grab_min_size,
            metrics.window_rounding,
            metrics.scrollbar_rounding,
            metrics.scrollbar_size,
        ]);
    let colors_valid = [
        theme.palette.background,
        theme.palette.window_background,
        theme.palette.tab_bar,
        theme.palette.surface,
        theme.palette.text,
        theme.palette.accent,
        theme.palette.selection,
    ]
    .into_iter()
    .flatten()
    .all(|value| value.is_finite() && (0.0..=1.0).contains(&value));
    let motion = &theme.motion;
    let valid = colors_valid
        && dimensions
            .into_iter()
            .all(|value| value.is_finite() && value >= 0.0)
        && theme.opacity.is_finite()
        && (0.0..=1.0).contains(&theme.opacity)
        && metrics.disabled_alpha.is_finite()
        && (0.0..=1.0).contains(&metrics.disabled_alpha)
        && metrics.tooltip_wrap_em.is_finite()
        && metrics.tooltip_wrap_em > 0.0
        && [
            motion.panel_seconds,
            motion.popup_seconds,
            motion.transition_seconds,
            motion.tree_seconds,
        ]
        .into_iter()
        .all(|value| value.is_finite() && value > 0.0)
        && motion.tree_stagger_seconds.is_finite()
        && motion.tree_stagger_seconds >= 0.0
        && motion.popup_initial_scale.is_finite()
        && (0.0..=1.0).contains(&motion.popup_initial_scale);
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "UI theme colors, dimensions and motion must be finite and within their supported ranges",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Condition, FramePrepareOptions, StyleVar, sys};

    #[test]
    fn prepared_components_preserve_context_theme_inside_square_panels() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        let mut runtime = UiRuntime::new(&context);
        let mut fonts = UiFonts::default();
        let mut theme = UiTheme::default();
        theme.metrics.popup_rounding = 13.0;
        theme.metrics.tooltip_padding = [18.0, 15.0];
        theme.metrics.control_padding = [0.8, 0.5];
        theme.motion.enabled = false;
        runtime.prepare(&mut context, &theme, &mut fonts).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        context.prepare_frame(FramePrepareOptions::new([600.0, 400.0], 1.0 / 60.0));
        let prepared = runtime.frame_context();
        let ui = context.frame();
        runtime.begin(ui);
        prepared.scope(|| {
            let _square = ui.push_style_var(StyleVar::WindowRounding(0.0));
            let _padding = ui.push_style_var(StyleVar::FramePadding([0.0; 2]));
            ui.window("square panel")
                .position([0.0; 2], Condition::Always)
                .size([300.0, 200.0], Condition::Always)
                .build(|| {
                    let active = UiFrameContext::for_ui(ui);
                    assert!(Arc::ptr_eq(&active.theme, &prepared.theme));
                    crate::components::button(ui, &active, "Themed button");
                    assert!(ui.item_rect_size()[1] >= ui.current_font_size() * 2.0);
                    crate::components::tooltip_text(ui, &active, "Themed tooltip");
                });
        });
        runtime.end(ui);
        ui.with_bound_context(|| unsafe {
            let native = &*sys::igGetCurrentContext();
            let tooltip = (0..native.Windows.Size)
                .find_map(|index| {
                    let window = &**native.Windows.Data.add(index as usize);
                    (window.Active && window.Flags & sys::ImGuiWindowFlags_Tooltip != 0)
                        .then_some(window)
                })
                .unwrap();
            assert_eq!(tooltip.WindowRounding, 13.0);
            assert_eq!(
                [tooltip.WindowPadding.x, tooltip.WindowPadding.y],
                [18.0, 15.0]
            );
        });
        drop(context.render_legacy());
    }

    #[test]
    fn rejected_theme_preserves_prepared_palette_metrics_and_font_generation() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        let mut runtime = UiRuntime::new(&context);
        let mut fonts = UiFonts::default();
        let theme = UiTheme::default();
        runtime.prepare(&mut context, &theme, &mut fonts).unwrap();
        let before = runtime.frame_context();
        let mut invalid = theme.clone();
        invalid.fonts.size = 44.0;
        invalid.metrics.tooltip_wrap_em = f32::NAN;
        assert!(runtime.prepare(&mut context, &invalid, &mut fonts).is_err());
        let after = runtime.frame_context();
        assert!(Arc::ptr_eq(&before.theme, &after.theme));
        assert_eq!(before.font_generation, after.font_generation);
        assert_eq!(
            context.style().frame_padding(),
            theme.metrics.control_padding.map(|v| v * theme.fonts.size)
        );
    }

    #[test]
    fn separate_contexts_keep_independent_theme_snapshots() {
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut first = Context::create();
        let mut first_runtime = UiRuntime::new(&first);
        let mut first_fonts = UiFonts::default();
        let first_theme = UiTheme::default();
        first_runtime
            .prepare(&mut first, &first_theme, &mut first_fonts)
            .unwrap();
        let first_snapshot = first_runtime.frame_context();
        // Creating another live context requires releasing native activation.
        // Every subsequent safe operation binds its own ContextBinding.
        unsafe { sys::igSetCurrentContext(std::ptr::null_mut()) };
        let mut second = Context::create();
        let mut second_runtime = UiRuntime::new(&second);
        let mut second_fonts = UiFonts::default();
        let mut second_theme = UiTheme::default();
        second_theme.palette.text = [0.2, 0.3, 0.4, 1.0];
        second_theme.metrics.popup_rounding = 3.0;
        second_runtime
            .prepare(&mut second, &second_theme, &mut second_fonts)
            .unwrap();
        assert_eq!(first_snapshot.theme(), &first_theme);
        assert_eq!(second_runtime.frame_context().theme(), &second_theme);
        assert_ne!(
            first_snapshot.binding.id(),
            second_runtime.frame_context().binding.id()
        );
    }
}
