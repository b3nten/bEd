//! A font viewer for workbench apps: TrueType and OpenType files (and the
//! first face of a collection). Preview shows the font at sizes, with text
//! you can type; Glyphs shows every character it has, and clicking one
//! inspects it: filled, with its outline, points and metrics. The floating
//! bar switches between them; the right-click menu copies.
use bedtime_ui::{BuiltinIcon, Floating, UiContext, UiFrame, egui, theme};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{Panel, PanelPresentation, ViewersAppExt, viewer_document};
use bevy::prelude::*;
use std::sync::Arc;

mod face;
pub use face::{Command, Face, FontFile, Glyph, Outline, SUPPORTED_EXTENSIONS};

pub struct FontPlugin;

/// Font files: the extensions in `SUPPORTED_EXTENSIONS`.
pub const FONT: bedtime_workbench::FormatId = bedtime_workbench::FormatId("font");

impl Plugin for FontPlugin {
    fn build(&self, app: &mut App) {
        use bedtime_workbench::{Format, FormatsAppExt};
        app.register_type::<FontPanel>()
            .register_format(Format::new(FONT).extensions(SUPPORTED_EXTENSIONS.iter().copied()))
            .register_viewer::<FontFile, FontPanel>("Font")
            .formats([FONT]);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum FontMode {
    #[default]
    Preview,
    Glyphs,
}

const SAMPLE: &str = "The quick brown fox jumps over the lazy dog";
const SIZES: &[f32] = &[12.0, 16.0, 20.0, 28.0, 36.0, 48.0, 64.0, 96.0];

#[bedtime_ui::panel]
#[derive(Reflect)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
pub struct FontPanel {
    pub mode: FontMode,
    pub sample: String,
    #[reflect(ignore)]
    pub selected: Option<char>,
    #[reflect(ignore)]
    inspected: Option<(char, egui::TextureHandle, Arc<Glyph>)>,
}

impl Default for FontPanel {
    fn default() -> Self {
        Self {
            mode: FontMode::Preview,
            sample: SAMPLE.into(),
            selected: None,
            inspected: None,
        }
    }
}

impl Panel for FontPanel {
    const LABEL: &'static str = "Font";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = viewer_document::<FontFile>(ui, world, panel) else {
            return;
        };
        let Some(face) = document.model.0 else {
            return;
        };
        let focused = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|presentation| presentation.focused);
        let Some(family) = family(ui.ctx(), &face) else {
            // Registered with egui; usable next frame.
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        };
        if let Some(mut font) = world.get_mut::<FontPanel>(panel) {
            draw(ui, context, panel, &mut font, &face, family, focused);
        }
    }
}

/// The face's egui font family, registering it the first time (it can be
/// used from the next frame). Fonts set later (a theme's) drop it, so it's
/// checked each frame.
fn family(ctx: &egui::Context, face: &Arc<Face>) -> Option<egui::FontFamily> {
    let name = format!("bedtime-font:{:p}", Arc::as_ptr(&face.bytes));
    let family = egui::FontFamily::Name(name.as_str().into());
    let present = ctx.fonts(|fonts| fonts.definitions().families.contains_key(&family));
    if present {
        return Some(family);
    }
    ctx.add_font(egui::epaint::text::FontInsert::new(
        &name,
        egui::FontData::from_owned(face.bytes.to_vec()),
        vec![egui::epaint::text::InsertFontFamily {
            family,
            priority: egui::epaint::text::FontPriority::Highest,
        }],
    ));
    ctx.request_repaint();
    None
}

fn draw(
    ui: &mut egui::Ui,
    context: &UiContext<'_>,
    panel: Entity,
    font: &mut FontPanel,
    face: &Arc<Face>,
    family: egui::FontFamily,
    focused: bool,
) {
    let id = egui::Id::new(("bedtime.font", panel));
    let rect = ui.available_rect_before_wrap();
    let mut frame = context.frame(ui);
    let p = context.theme.palette.clone();
    let muted = theme::color(p.muted);
    // The page, scrolling under the floating bar.
    frame.scroll(id.with("scroll"), 0.0, |f| {
        f.ui.set_min_width(rect.width());
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(28, 22))
            .show(f.ui, |ui| {
                header(ui, face, &family, muted);
                ui.add_space(18.0);
                match font.mode {
                    FontMode::Preview => preview(ui, font, &family, muted, &p),
                    FontMode::Glyphs => glyphs(ui, font, face, &family, muted, &p, focused),
                }
            });
    });
    Floating::new(id.with("modes"), rect, egui::Align2::RIGHT_TOP).show(&mut frame, |f| {
        if f.floating_toggle(BuiltinIcon::Type, "Preview", font.mode == FontMode::Preview)
            .clicked()
        {
            font.mode = FontMode::Preview;
        }
        if f.floating_toggle(BuiltinIcon::Grid, "Glyphs", font.mode == FontMode::Glyphs)
            .clicked()
        {
            font.mode = FontMode::Glyphs;
        }
    });
    if font.mode == FontMode::Glyphs
        && let Some(c) = font.selected
    {
        inspector(&mut frame, id, rect, font, face, c);
    }
}

fn header(ui: &mut egui::Ui, face: &Face, family: &egui::FontFamily, muted: egui::Color32) {
    ui.label(
        egui::RichText::new(&face.family)
            .font(egui::FontId::new(40.0, family.clone()))
            .strong(),
    );
    let mut facts = vec![face.style.clone(), format!("{} glyphs", face.glyphs)];
    if face.monospace {
        facts.push("Monospaced".into());
    }
    if !face.axes.is_empty() {
        let tags: Vec<&str> = face.axes.iter().map(|(tag, ..)| tag.as_str()).collect();
        facts.push(format!("Variable ({})", tags.join(", ")));
    }
    if face.faces > 1 {
        facts.push(format!("1 of {} faces", face.faces));
    }
    ui.label(egui::RichText::new(facts.join(" · ")).color(muted));
    let mut credits = Vec::new();
    if let Some(version) = &face.version {
        credits.push(version.clone());
    }
    if let Some(designer) = &face.designer {
        credits.push(designer.clone());
    }
    if !credits.is_empty() {
        ui.label(
            egui::RichText::new(credits.join(" · "))
                .color(muted)
                .small(),
        );
    }
}

fn preview(
    ui: &mut egui::Ui,
    font: &mut FontPanel,
    family: &egui::FontFamily,
    muted: egui::Color32,
    p: &theme::Palette,
) {
    // The sample: type to change it.
    let edit = egui::TextEdit::multiline(&mut font.sample)
        .font(egui::FontId::new(48.0, family.clone()))
        .frame(egui::Frame::NONE)
        .desired_width(f32::INFINITY)
        .desired_rows(1)
        .hint_text(SAMPLE);
    let response = ui.add(edit);
    response.context_menu(|ui| {
        if ui.button("Reset Sample Text").clicked() {
            font.sample = SAMPLE.into();
        }
    });
    let sample = if font.sample.trim().is_empty() {
        SAMPLE.to_owned()
    } else {
        font.sample.replace('\n', " ")
    };
    ui.add_space(14.0);
    rule(ui, p);
    // The waterfall.
    for &size in SIZES {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_sized(
                egui::vec2(34.0, size),
                egui::Label::new(
                    egui::RichText::new(format!("{size:.0}"))
                        .monospace()
                        .small()
                        .color(muted),
                ),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&sample).font(egui::FontId::new(size, family.clone())),
                )
                .truncate(),
            );
        });
    }
    ui.add_space(12.0);
    rule(ui, p);
    ui.add_space(10.0);
    for line in [
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "abcdefghijklmnopqrstuvwxyz",
        "0123456789 .,:;!?&@#$%*(){}[]",
    ] {
        ui.label(egui::RichText::new(line).font(egui::FontId::new(30.0, family.clone())));
    }
}

fn rule(ui: &mut egui::Ui, p: &theme::Palette) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(
        rect,
        0.0,
        theme::color(theme::mix(p.border, p.panel_background, 0.3)),
    );
}

const CELL: egui::Vec2 = egui::vec2(72.0, 84.0);

#[allow(clippy::too_many_arguments)]
fn glyphs(
    ui: &mut egui::Ui,
    font: &mut FontPanel,
    face: &Face,
    family: &egui::FontFamily,
    muted: egui::Color32,
    p: &theme::Palette,
    focused: bool,
) {
    let characters = &face.characters;
    let columns = ((ui.available_width() / CELL.x).floor() as usize).max(1);
    // Arrow keys move the selection (and scroll to it).
    let mut keyed = false;
    if focused
        && let Some(selected) = font.selected
        && !ui.ctx().egui_wants_keyboard_input()
        && let Ok(index) = characters.binary_search(&selected)
    {
        let moved = ui.input_mut(|i| {
            let mut index = index as isize;
            for (key, step) in [
                (egui::Key::ArrowLeft, -1),
                (egui::Key::ArrowRight, 1),
                (egui::Key::ArrowUp, -(columns as isize)),
                (egui::Key::ArrowDown, columns as isize),
            ] {
                if i.consume_key(egui::Modifiers::NONE, key) {
                    index += step;
                }
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                return None;
            }
            Some(index.clamp(0, characters.len() as isize - 1) as usize)
        });
        let moved = moved.map(|index| characters[index]);
        keyed = moved != font.selected;
        font.selected = moved;
    }
    let selection = theme::color(p.selection);
    let hover = theme::color(theme::mix(p.panel_background, p.text, 0.06));
    let ink = theme::color(p.text);
    // Only the rows on screen are laid out; the rest is empty space.
    let rows = characters.len().div_ceil(columns);
    let top = ui.cursor().top();
    let clip = ui.clip_rect();
    let mut first = (((clip.top() - top) / CELL.y).floor().max(0.0) as usize).min(rows);
    let mut last = (((clip.bottom() - top) / CELL.y).ceil().max(0.0) as usize).min(rows);
    if keyed
        && let Some(index) = font
            .selected
            .and_then(|c| characters.binary_search(&c).ok())
    {
        let row = index / columns;
        let rect = egui::Rect::from_min_size(
            egui::pos2(ui.cursor().left(), top + row as f32 * CELL.y),
            egui::vec2(CELL.x, CELL.y),
        );
        ui.scroll_to_rect(rect, None);
        first = first.min(row);
        last = last.max(row + 1);
    }
    let width = columns as f32 * CELL.x;
    ui.allocate_space(egui::vec2(width, first as f32 * CELL.y));
    ui.spacing_mut().item_spacing.y = 0.0;
    for row in first..last {
        let chunk = &characters[row * columns..((row + 1) * columns).min(characters.len())];
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for &c in chunk {
                let (rect, response) = ui.allocate_exact_size(CELL, egui::Sense::click());
                let selected = font.selected == Some(c);
                // Named by its code point, for accessibility (and scripts).
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Button,
                        true,
                        selected,
                        format!("{:04X}", c as u32),
                    )
                });
                if selected {
                    ui.painter().rect_filled(rect.shrink(2.0), 6.0, selection);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect.shrink(2.0), 6.0, hover);
                }
                ui.painter().text(
                    rect.center() - egui::vec2(0.0, 8.0),
                    egui::Align2::CENTER_CENTER,
                    c,
                    egui::FontId::new(30.0, family.clone()),
                    ink,
                );
                ui.painter().text(
                    rect.center_bottom() - egui::vec2(0.0, 8.0),
                    egui::Align2::CENTER_BOTTOM,
                    format!("{:04X}", c as u32),
                    egui::FontId::monospace(10.0),
                    muted,
                );
                if response.clicked() {
                    font.selected = (!selected).then_some(c);
                    response.request_focus();
                }
                response.context_menu(|ui| {
                    if ui.button("Copy Character").clicked() {
                        ui.ctx().copy_text(c.to_string());
                    }
                    if ui.button("Copy Code Point").clicked() {
                        ui.ctx().copy_text(format!("U+{:04X}", c as u32));
                    }
                });
            }
        });
    }
    ui.allocate_space(egui::vec2(width, (rows - last) as f32 * CELL.y));
}

/// The selected glyph, large: filled, with its outline, points and the
/// font's lines (ascender, cap height, x-height, baseline, descender).
fn inspector(
    frame: &mut UiFrame<'_>,
    id: egui::Id,
    rect: egui::Rect,
    font: &mut FontPanel,
    face: &Face,
    c: char,
) {
    const SIDE: f32 = 240.0;
    if font.inspected.as_ref().is_none_or(|(at, ..)| *at != c) {
        font.inspected = face.glyph(c).and_then(|glyph| {
            let pixels_per_point = frame.ui.ctx().pixels_per_point();
            let size = (SIDE * pixels_per_point).round() as u32;
            let (scale, origin) = layout(face, SIDE * pixels_per_point);
            let pixels = glyph.outline.rasterize(size, |[x, y]| {
                [origin[0] + x * scale, origin[1] - y * scale]
            })?;
            let image =
                egui::ColorImage::from_rgba_unmultiplied([size as usize, size as usize], &pixels);
            let texture = frame.ui.ctx().load_texture(
                format!("{id:?}-glyph"),
                image,
                egui::TextureOptions::LINEAR,
            );
            Some((c, texture, Arc::new(glyph)))
        });
    }
    let Some((_, texture, glyph)) = font.inspected.clone() else {
        return;
    };
    let p = frame.theme.palette.clone();
    let mut close = false;
    Floating::new(id.with("inspector"), rect, egui::Align2::RIGHT_BOTTOM)
        .shown(true)
        .show(frame, |f| {
            f.scope(
                egui::UiBuilder::new().layout(egui::Layout::top_down(egui::Align::Min)),
                |mut card| {
                    let ui = &mut *card.ui;
                    let (canvas, _) =
                        ui.allocate_exact_size(egui::Vec2::splat(SIDE), egui::Sense::hover());
                    let painter = ui.painter().with_clip_rect(canvas);
                    let (scale, origin) = layout(face, SIDE);
                    let to = |[x, y]: [f32; 2]| {
                        canvas.min + egui::vec2(origin[0] + x * scale, origin[1] - y * scale)
                    };
                    // The font's lines.
                    let line_ink = theme::color(theme::mix(p.border, p.panel_background, 0.2));
                    let label_ink = theme::color(p.muted);
                    let mut lines = vec![
                        (face.ascent, "ascender"),
                        (0.0, "baseline"),
                        (face.descent, "descender"),
                    ];
                    if let Some(cap) = face.cap_height {
                        lines.push((cap, "cap height"));
                    }
                    if let Some(x) = face.x_height {
                        lines.push((x, "x-height"));
                    }
                    for (y, name) in lines {
                        let at = to([0.0, y]).y;
                        painter.hline(canvas.x_range(), at, egui::Stroke::new(1.0, line_ink));
                        painter.text(
                            egui::pos2(canvas.right() - 4.0, at - 2.0),
                            egui::Align2::RIGHT_BOTTOM,
                            name,
                            egui::FontId::proportional(9.0),
                            label_ink,
                        );
                    }
                    // Its advance.
                    for x in [0.0, glyph.advance] {
                        let at = to([x, 0.0]).x;
                        painter.vline(at, canvas.y_range(), egui::Stroke::new(1.0, line_ink));
                    }
                    // Filled, then its outline and points.
                    painter.image(
                        texture.id(),
                        canvas,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        theme::color(theme::mix(p.text, p.panel_background, 0.55)),
                    );
                    let accent = theme::color(p.accent);
                    for line in glyph.outline.polylines(|p| to(p).into()) {
                        let points: Vec<egui::Pos2> =
                            line.into_iter().map(egui::Pos2::from).collect();
                        painter.add(egui::Shape::line(points, egui::Stroke::new(1.2, accent)));
                    }
                    let (on, off) = glyph.outline.points();
                    for point in on {
                        painter.circle_filled(to(point), 2.2, accent);
                    }
                    for point in off {
                        painter.circle_stroke(to(point), 2.0, egui::Stroke::new(1.0, label_ink));
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "Glyph {} · advance {:.0} of {} units",
                            glyph.id, glyph.advance, face.units_per_em
                        ))
                        .small()
                        .color(label_ink),
                    );
                    let name = glyph
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("glyph {}", glyph.id));
                    card.scope(
                        egui::UiBuilder::new()
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        |mut row| {
                            row.ui.set_width(SIDE);
                            row.ui.label(
                                egui::RichText::new(format!("U+{:04X}  {name}", c as u32))
                                    .monospace()
                                    .small(),
                            );
                            let room = row.ui.available_width() - row.metrics.font_size - 12.0;
                            row.ui.add_space(room.max(0.0));
                            if row
                                .floating_button(BuiltinIcon::Close, "Close (Escape)")
                                .clicked()
                            {
                                close = true;
                            }
                        },
                    );
                },
            );
        });
    if close {
        font.selected = None;
    }
}

/// Font units to a `side` square: the em from descender to ascender fits,
/// with a margin, and glyphs start a little in.
fn layout(face: &Face, side: f32) -> (f32, [f32; 2]) {
    let height = (face.ascent - face.descent).max(face.units_per_em as f32);
    let scale = side * 0.78 / height;
    let top = (side - height * scale) / 2.0;
    (scale, [side * 0.18, top + face.ascent * scale])
}
