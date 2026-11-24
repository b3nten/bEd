use crate::tokens::Space;

/// Things side by side, a `gap` apart.
///
/// ```ignore
/// Row::new().gap(Space::Sm).show(ui, |ui| {
///     ui.add(Button::new("Cancel"));
///     ui.add(Button::new("Save").primary());
/// });
/// ```
#[must_use = "a row does nothing until it's shown"]
#[derive(Clone, Copy)]
pub struct Row(Stack);

/// Things one under another, a `gap` apart.
#[must_use = "a column does nothing until it's shown"]
#[derive(Clone, Copy)]
pub struct Column(Stack);

#[derive(Clone, Copy)]
struct Stack {
    gap: Space,
    padding: Space,
    align: egui::Align,
    wrap: bool,
}

impl Default for Stack {
    fn default() -> Self {
        Self {
            gap: Space::Sm,
            padding: Space::None,
            align: egui::Align::Min,
            wrap: false,
        }
    }
}

impl Row {
    /// Centred on each other, `Space::Sm` apart.
    pub fn new() -> Self {
        Self(Stack {
            align: egui::Align::Center,
            ..Stack::default()
        })
    }
    pub fn gap(mut self, gap: Space) -> Self {
        self.0.gap = gap;
        self
    }
    /// Space around the row.
    pub fn padding(mut self, padding: Space) -> Self {
        self.0.padding = padding;
        self
    }
    /// Where things shorter than the row sit: top, centre or bottom.
    pub fn align(mut self, align: egui::Align) -> Self {
        self.0.align = align;
        self
    }
    /// Go on to another line when the row is full.
    pub fn wrap(mut self) -> Self {
        self.0.wrap = true;
        self
    }

    pub fn show<R>(
        self,
        ui: &mut egui::Ui,
        content: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<R> {
        let stack = self.0;
        let gap = stack.gap.px(ui);
        stack.padded(ui, |ui| {
            let size = egui::vec2(
                ui.available_size_before_wrap().x,
                ui.spacing().interact_size.y,
            );
            let layout = egui::Layout::left_to_right(stack.align).with_main_wrap(stack.wrap);
            ui.allocate_ui_with_layout(size, layout, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::splat(gap);
                content(ui)
            })
            .inner
        })
    }
}

impl Default for Row {
    fn default() -> Self {
        Self::new()
    }
}

impl Column {
    /// Aligned left, `Space::Sm` apart.
    pub fn new() -> Self {
        Self(Stack::default())
    }
    pub fn gap(mut self, gap: Space) -> Self {
        self.0.gap = gap;
        self
    }
    /// Space around the column.
    pub fn padding(mut self, padding: Space) -> Self {
        self.0.padding = padding;
        self
    }
    /// Where things narrower than the column sit: left, centre or right.
    pub fn align(mut self, align: egui::Align) -> Self {
        self.0.align = align;
        self
    }

    pub fn show<R>(
        self,
        ui: &mut egui::Ui,
        content: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<R> {
        let stack = self.0;
        let gap = stack.gap.px(ui);
        stack.padded(ui, |ui| {
            let remembered = Remembered::of(ui);
            let size = egui::vec2(
                ui.available_size_before_wrap().x,
                remembered.height.unwrap_or(0.0),
            );
            let layout = egui::Layout::top_down(stack.align);
            let (inner, height) = ui
                .allocate_ui_with_layout(size, layout, |ui| {
                    ui.spacing_mut().item_spacing.y = gap;
                    let inner = content(ui);
                    (inner, ui.min_rect().height())
                })
                .inner;
            remembered.remember(ui, height);
            inner
        })
    }
}

impl Default for Column {
    fn default() -> Self {
        Self::new()
    }
}

impl Stack {
    fn padded<R>(
        self,
        ui: &mut egui::Ui,
        content: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::InnerResponse<R> {
        let padding = self.padding.px(ui);
        egui::Frame::NONE.inner_margin(padding).show(ui, content)
    }
}

/// How tall a column's content was last frame. egui places a child from the
/// size it asks for before it's drawn; a column doesn't know its own until
/// after, so it asks for last frame's, and a row centres or bottom-aligns it
/// properly. (A row asks for one control's height, as egui's own does.)
struct Remembered {
    id: egui::Id,
    height: Option<f32>,
}

impl Remembered {
    fn of(ui: &egui::Ui) -> Self {
        let id = ui.next_auto_id().with("bedtime-column");
        let height = ui.data(|data| data.get_temp(id));
        Self { id, height }
    }

    fn remember(self, ui: &egui::Ui, height: f32) {
        if self.height != Some(height) {
            ui.data_mut(|data| data.insert_temp(self.id, height));
            // Drawn from a guess this frame: draw again from what it was.
            ui.ctx().request_repaint();
        }
    }
}
