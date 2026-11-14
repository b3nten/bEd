//! `+widgets`: every widget bed can draw today, from egui and from
//! bedtime-ui, on one page. It shows what the component library already has,
//! and how one theme change reaches all of it.
//!
//! Each card is tagged with where the widget comes from and the call that
//! draws it, so the page doubles as an index into the APIs.

use bedtime_terminal::{ShellCommandsAppExt, ShellInvocation};
use bedtime_ui::{BuiltinIcon, CodeView, UiContext, egui, theme, tree};
use bedtime_workbench::{
    Notification, Panel, PanelsWorldExt, Placement, WorkbenchWorldExt, persistence::ReflectPersist,
};
use bevy::prelude::*;

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Persist)]
#[require(Showcase)]
pub struct WidgetsPanel;

pub struct WidgetsPlugin;

impl Plugin for WidgetsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<WidgetsPanel>()
            .add_shell_command(["+widgets"], open_from_shell)
            .expect("bed's shell commands are unique");
    }
}

/// A new widgets panel, somewhere other than the terminal it was asked in.
fn open_from_shell(world: &mut World, invocation: ShellInvocation) {
    let result = world
        .open_panel_at(WidgetsPanel, Placement::From(invocation.panel))
        .map(|_| ());
    invocation.reply.complete(result);
}

/// Where a widget comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Egui,
    Bedtime,
    Workbench,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Egui => "egui",
            Source::Bedtime => "bedtime-ui",
            Source::Workbench => "workbench",
        }
    }
}

/// The demo widgets' values. Not persisted: it's a playground.
#[derive(Component)]
struct Showcase {
    filter: String,
    only: Option<Source>,
    // Buttons
    clicks: u32,
    // Selection
    checked: bool,
    mixed: bool,
    radio: usize,
    segment: usize,
    pinned: bool,
    toggles: [bool; 3],
    choice: usize,
    // Numbers
    level: f32,
    volume: f32,
    stepped: i32,
    exposure: f64,
    count: i32,
    price: f64,
    angle: f32,
    // Text
    name: String,
    search: String,
    password: String,
    notes: String,
    code: String,
    // Data
    code_view: CodeView,
    data_tree: bedtime_ui::data::DataTree,
    hex: bedtime_ui::hex::HexViewer,
    markdown: bedtime_ui::markdown::MarkdownView,
    compact: bool,
    picture: std::sync::Arc<bedtime_ui::image::Decoded>,
    image_view: bedtime_ui::image::ImageView,
    // Feedback
    progress: f32,
    running: bool,
    // Color
    color: egui::Color32,
    hsva: egui::ecolor::Hsva,
    // Containers
    open: bool,
    selected_row: usize,
    scene: egui::Rect,
    // Overlays
    modal: bool,
    // Tree
    tree_open: [bool; 2],
    tree_selected: usize,
    // Icons
    icon_filter: String,
    icon_size: f32,
}

impl Default for Showcase {
    fn default() -> Self {
        Self {
            filter: String::new(),
            only: None,
            clicks: 0,
            checked: true,
            mixed: false,
            radio: 1,
            segment: 0,
            pinned: true,
            toggles: [true, false, false],
            choice: 0,
            level: 0.6,
            volume: 40.0,
            stepped: 3,
            exposure: 100.0,
            count: 12,
            price: 9.99,
            angle: 0.6,
            name: "bed".into(),
            search: String::new(),
            password: "hunter2".into(),
            notes: "A multi-line note.\nIt grows as you type.".into(),
            code: "fn main() {\n    println!(\"hello, bed\");\n}".into(),
            code_view: CodeView::new(SAMPLE_JSON).language("json"),
            data_tree: bedtime_ui::data::DataTree::new(std::sync::Arc::new(
                bedtime_ui::data::parse(bedtime_ui::data::Format::Json, SAMPLE_JSON)
                    .expect("the sample is JSON"),
            )),
            hex: Default::default(),
            markdown: bedtime_ui::markdown::MarkdownView::new(SAMPLE_MARKDOWN)
                .code_language("rust"),
            compact: false,
            picture: std::sync::Arc::new(gradient()),
            image_view: Default::default(),
            progress: 0.35,
            running: true,
            color: egui::Color32::from_rgb(0xa7, 0xc0, 0x80),
            hsva: egui::ecolor::Hsva::new(0.6, 0.6, 0.9, 1.0),
            open: true,
            selected_row: 2,
            scene: egui::Rect::ZERO,
            modal: false,
            tree_open: [true, false],
            tree_selected: 2,
            icon_filter: String::new(),
            icon_size: 20.0,
        }
    }
}

/// What the page asked the app to do, applied after drawing.
enum Request {
    Notify(Notification),
}

impl Panel for WidgetsPanel {
    const LABEL: &'static str = "Widgets";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(mut showcase) = world.get_mut::<Showcase>(panel) else {
            return;
        };
        let mut requests = Vec::new();
        let mut frame = context.frame(ui);
        frame.scroll("widgets", 0.0, |mut frame| {
            frame.padded([18.0, 16.0], |frame| {
                page(frame.ui, context, &mut showcase, &mut requests);
            })
        });
        for request in requests {
            match request {
                Request::Notify(notification) => world.workbench_mut().notify(notification),
            }
        }
    }
}

fn page(ui: &mut egui::Ui, cx: &UiContext<'_>, s: &mut Showcase, requests: &mut Vec<Request>) {
    let p = &cx.theme.palette;
    ui.set_max_width(ui.available_width().min(760.0));
    ui.label(
        egui::RichText::new("COMPONENTS")
            .small()
            .strong()
            .color(theme::color(p.muted)),
    );
    ui.heading("Widgets");
    ui.label(
        egui::RichText::new(
            "Everything bed can draw today. Each card names its source and the call that draws it.",
        )
        .color(theme::color(p.muted)),
    );
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut s.filter)
                .hint_text("Filter widgets")
                .desired_width(220.0),
        );
        ui.selectable_value(&mut s.only, None, "All");
        for source in [Source::Egui, Source::Bedtime, Source::Workbench] {
            ui.selectable_value(&mut s.only, Some(source), source.label());
        }
    });
    ui.add_space(12.0);

    let filter = Filter {
        text: s.filter.to_lowercase(),
        only: s.only,
        section: std::cell::Cell::new(None),
    };

    filter.section("Typography");
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Text styles",
        "RichText",
        |ui| {
            ui.heading("Heading");
            ui.horizontal_wrapped(|ui| {
                ui.label("Body");
                ui.label(egui::RichText::new("Strong").strong());
                ui.label(egui::RichText::new("Weak").weak());
                ui.label(egui::RichText::new("Small").small());
                ui.label(egui::RichText::new("Italic").italics());
                ui.label(egui::RichText::new("Underline").underline());
                ui.label(egui::RichText::new("Strike").strikethrough());
                ui.label(egui::RichText::new("Monospace").monospace());
                ui.code("inline code");
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Status text",
        "colored_label",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(theme::color(cx.theme.palette.success), "Success");
                ui.colored_label(theme::color(cx.theme.palette.warning), "Warning");
                ui.colored_label(theme::color(cx.theme.palette.error), "Error");
                ui.colored_label(theme::color(cx.theme.palette.accent), "Accent");
                ui.colored_label(theme::color(cx.theme.palette.muted), "Muted");
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Links",
        "hyperlink_to · link",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.hyperlink_to("egui docs", "https://docs.rs/egui");
                if ui.link("An in-app link").clicked() {
                    s.clicks += 1;
                }
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Wrapping and truncation",
        "Label::wrap · truncate",
        |ui| {
            let long = "A long line of text that has more to say than the space it was given, so it has to do something about it.";
            ui.add(egui::Label::new(long).wrap());
            ui.add(egui::Label::new(long).truncate());
        },
    );

    filter.section("Buttons");
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Button",
        "Button · small_button",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Default").clicked() {
                    s.clicks += 1;
                }
                if ui.small_button("Small").clicked() {
                    s.clicks += 1;
                }
                if ui
                    .add(egui::Button::new("Frameless").frame(false))
                    .clicked()
                {
                    s.clicks += 1;
                }
                if ui
                    .add(egui::Button::new("Wide").min_size(egui::vec2(140.0, 0.0)))
                    .clicked()
                {
                    s.clicks += 1;
                }
                ui.add(egui::Button::new("Selected").selected(true));
                ui.add_enabled(false, egui::Button::new("Disabled"));
            });
            ui.label(
                egui::RichText::new(format!("{} clicks", s.clicks))
                    .small()
                    .weak(),
            );
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Button with shortcut",
        "Button::shortcut_text",
        |ui| {
            ui.add(egui::Button::new("Save").shortcut_text("⌘S"));
            ui.add(
                egui::Button::new("Open…")
                    .shortcut_text("⌘O")
                    .right_text("→"),
            );
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Primary button",
        "UiFrame::primary_button",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if cx.frame(ui).primary_button("Make something").clicked() {
                    s.clicks += 1;
                }
                ui.add_enabled_ui(false, |ui| cx.frame(ui).primary_button("Disabled"));
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Icon buttons",
        "icon_button · icon_button_sized · icon_ink_button",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                let mut f = cx.frame(ui);
                for icon in [
                    BuiltinIcon::Plus,
                    BuiltinIcon::Search,
                    BuiltinIcon::Settings,
                ] {
                    let response = f.icon_button(icon);
                    f.tooltip(&response, icon.key());
                }
                f.ui.separator();
                f.icon_button_sized(BuiltinIcon::Play, 22.0);
                f.ui.separator();
                for icon in [
                    BuiltinIcon::SplitRight,
                    BuiltinIcon::SplitDown,
                    BuiltinIcon::Close,
                ] {
                    f.icon_ink_button(icon, 16.0);
                }
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Floating bar",
        "floating_button · floating_toggle · floating_label",
        |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut f = cx.frame(ui);
                    if f.floating_toggle(BuiltinIcon::Play, "Play", s.running)
                        .clicked()
                    {
                        s.running = !s.running;
                    }
                    f.floating_button(BuiltinIcon::Repeat, "Repeat");
                    f.floating_button(BuiltinIcon::Fit, "Fit");
                    f.floating_label(format!("{:.0}%", s.progress * 100.0));
                });
            });
        },
    );

    filter.section("Selection");
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Checkbox",
        "checkbox · Checkbox::indeterminate",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut s.checked, "Checked");
                ui.add(egui::Checkbox::new(&mut s.mixed, "Indeterminate").indeterminate(true));
                ui.add_enabled(false, egui::Checkbox::new(&mut s.checked, "Disabled"));
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Radio",
        "radio_value",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                for (index, label) in ["Spaces", "Tabs", "Detect"].iter().enumerate() {
                    ui.radio_value(&mut s.radio, index, *label);
                }
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Segmented / toggle",
        "selectable_value · toggle_value",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                for (index, label) in ["Day", "Week", "Month"].iter().enumerate() {
                    ui.selectable_value(&mut s.segment, index, *label);
                }
                ui.separator();
                ui.toggle_value(&mut s.pinned, "📌 Pinned");
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Switch",
        "UiFrame::toggle",
        |ui| {
            let mut f = cx.frame(ui);
            for (value, label) in
                s.toggles
                    .iter_mut()
                    .zip(["Format on save", "Word wrap", "Minimap"])
            {
                f.toggle(value, label);
            }
            f.ui.add_enabled_ui(false, |ui| cx.frame(ui).toggle(&mut true, "Disabled"));
        },
    );
    card(ui, cx, &filter, Source::Egui, "Select", "ComboBox", |ui| {
        let options = ["Comfortable", "Compact", "Spacious"];
        egui::ComboBox::from_label("Density")
            .selected_text(options[s.choice])
            .show_ui(ui, |ui| {
                for (index, label) in options.iter().enumerate() {
                    ui.selectable_value(&mut s.choice, index, *label);
                }
            });
    });

    filter.section("Numbers");
    card(ui, cx, &filter, Source::Egui, "Slider", "Slider", |ui| {
        ui.add(egui::Slider::new(&mut s.level, 0.0..=1.0).text("Level"));
        ui.add(
            egui::Slider::new(&mut s.volume, 0.0..=100.0)
                .suffix(" dB")
                .text("With suffix"),
        );
        ui.add(egui::Slider::new(&mut s.stepped, 0..=10).text("Integer"));
        ui.add(
            egui::Slider::new(&mut s.exposure, 1.0..=10_000.0)
                .logarithmic(true)
                .text("Logarithmic"),
        );
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Slider::new(&mut s.level, 0.0..=1.0).vertical());
            ui.add(
                egui::Slider::new(&mut s.volume, 0.0..=100.0)
                    .vertical()
                    .show_value(false),
            );
            ui.label(egui::RichText::new("Vertical").small().weak());
        });
    });
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Number field",
        "DragValue · drag_angle",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Count");
                ui.add(egui::DragValue::new(&mut s.count).range(0..=99));
                ui.label("Price");
                ui.add(
                    egui::DragValue::new(&mut s.price)
                        .speed(0.01)
                        .prefix("$")
                        .fixed_decimals(2),
                );
                ui.label("Angle");
                ui.drag_angle(&mut s.angle);
            });
        },
    );

    filter.section("Text input");
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Text field",
        "TextEdit::singleline",
        |ui| {
            egui::Grid::new("text-fields")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut s.name);
                    ui.end_row();
                    ui.label("Search");
                    ui.add(egui::TextEdit::singleline(&mut s.search).hint_text("Type to search…"));
                    ui.end_row();
                    ui.label("Password");
                    ui.add(egui::TextEdit::singleline(&mut s.password).password(true));
                    ui.end_row();
                    ui.label("Limited");
                    ui.add(egui::TextEdit::singleline(&mut s.name).char_limit(8));
                    ui.end_row();
                    ui.label("Disabled");
                    ui.add_enabled(false, egui::TextEdit::singleline(&mut s.name));
                    ui.end_row();
                });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Text area",
        "TextEdit::multiline",
        |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut s.notes)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Code field",
        "TextEdit::code_editor",
        |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut s.code)
                    .code_editor()
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
        },
    );

    filter.section("Feedback");
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Progress",
        "ProgressBar",
        |ui| {
            if s.running {
                s.progress = (s.progress + ui.input(|i| i.stable_dt) * 0.08) % 1.0;
                ui.ctx().request_repaint();
            }
            ui.add(egui::ProgressBar::new(s.progress).show_percentage());
            ui.add(
                egui::ProgressBar::new(s.progress)
                    .text("Indexing…")
                    .animate(s.running),
            );
            ui.add(egui::ProgressBar::new(s.progress).desired_height(4.0));
        },
    );
    card(ui, cx, &filter, Source::Egui, "Spinner", "Spinner", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spinner();
            ui.add(egui::Spinner::new().size(24.0));
            ui.add(egui::Spinner::new().color(theme::color(cx.theme.palette.accent)));
        });
    });
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Rainbow ink",
        "rainbow_ink",
        |ui| {
            let ink = bedtime_ui::rainbow_ink(ui, cx.motion);
            ui.label(
                egui::RichText::new("The selection and caret color")
                    .color(ink)
                    .strong(),
            );
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Workbench,
        "Notification",
        "Workbench::notify",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Info").clicked() {
                    requests.push(Request::Notify(Notification::info("Saved 3 files")));
                }
                if ui.button("Warning").clicked() {
                    requests.push(Request::Notify(Notification::warning(
                        "The file changed on disk",
                    )));
                }
                if ui.button("Error").clicked() {
                    requests.push(Request::Notify(Notification::error(
                        "Couldn't reach the server",
                    )));
                }
            });
            ui.label(
                egui::RichText::new("Shown in the status bar.")
                    .small()
                    .weak(),
            );
        },
    );

    filter.section("Color");
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Color picker",
        "color_edit_button_srgba · color_picker_hsva_2d",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.color_edit_button_srgba(&mut s.color);
                ui.label(format!(
                    "#{:02x}{:02x}{:02x}",
                    s.color.r(),
                    s.color.g(),
                    s.color.b()
                ));
            });
            egui::color_picker::color_picker_hsva_2d(
                ui,
                &mut s.hsva,
                egui::color_picker::Alpha::Opaque,
            );
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Theme palette",
        "Theme::palette",
        |ui| {
            let p = &cx.theme.palette;
            let swatches = [
                ("background", p.background),
                ("panel", p.panel_background),
                ("surface", p.surface),
                ("elevated", p.elevated),
                ("border", p.border),
                ("text", p.text),
                ("muted", p.muted),
                ("accent", p.accent),
                ("selection", p.selection),
                ("success", p.success),
                ("warning", p.warning),
                ("error", p.error),
            ];
            ui.horizontal_wrapped(|ui| {
                for (name, rgba) in swatches {
                    swatch(ui, cx, name, theme::color(rgba));
                }
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for (index, rgba) in p.terminal.iter().enumerate() {
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 3.0, theme::color(*rgba));
                    response.on_hover_text(format!("terminal {index}"));
                }
            });
        },
    );

    filter.section("Layout");
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Collapsing",
        "CollapsingHeader",
        |ui| {
            egui::CollapsingHeader::new("Advanced options")
                .default_open(true)
                .show(ui, |ui| {
                    ui.label("Disclosed content.");
                    egui::CollapsingHeader::new("Nested").show(ui, |ui| {
                        ui.label("Deeper still.");
                    });
                });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Disclosure",
        "paint_chevron · animate_expansion",
        |ui| {
            let response = ui.add(egui::Button::new("      Show details").frame(false));
            if response.clicked() {
                s.open = !s.open;
            }
            let openness = bedtime_ui::animate_expansion(ui.ctx(), response.id, s.open, cx.motion);
            bedtime_ui::paint_chevron(
                ui.painter(),
                egui::pos2(response.rect.left() + 10.0, response.rect.center().y),
                openness,
                ui.visuals().text_color(),
            );
            if openness > 0.0 {
                let height = 40.0 * openness;
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), height),
                    egui::Sense::hover(),
                );
                ui.painter().with_clip_rect(rect).text(
                    rect.left_top() + egui::vec2(16.0, 6.0),
                    egui::Align2::LEFT_TOP,
                    "Expanded with bed's 160 ms ease.",
                    egui::FontId::proportional(cx.metrics.font_size),
                    ui.visuals().text_color(),
                );
            }
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Group and frame",
        "Frame::group · Frame::canvas · indent",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.group(|ui| {
                    ui.label("Grouped");
                    ui.label("content");
                });
                egui::Frame::canvas(ui.style())
                    .inner_margin(8)
                    .show(ui, |ui| ui.label("A canvas frame"));
            });
            ui.indent("indent", |ui| ui.label("Indented under its parent."));
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Grid",
        "Grid::striped",
        |ui| {
            egui::Grid::new("grid")
                .striped(true)
                .num_columns(3)
                .spacing([24.0, 6.0])
                .show(ui, |ui| {
                    ui.strong("File");
                    ui.strong("Size");
                    ui.strong("Changed");
                    ui.end_row();
                    for (file, size, changed) in [
                        ("main.rs", "64 KB", "now"),
                        ("widgets.rs", "22 KB", "a minute ago"),
                        ("effects.rs", "8 KB", "yesterday"),
                    ] {
                        ui.label(file);
                        ui.label(size);
                        ui.label(changed);
                        ui.end_row();
                    }
                });
        },
    );
    card(ui, cx, &filter, Source::Egui, "Sides", "Sides", |ui| {
        egui::Sides::new().show(
            ui,
            |ui| {
                ui.label("Left-aligned");
            },
            |ui| {
                let _ = ui.button("Right-aligned");
            },
        );
    });
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Separator",
        "Separator",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("One");
                ui.separator();
                ui.label("Two");
                ui.separator();
                ui.label("Three");
            });
            ui.separator();
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Virtual list",
        "ScrollArea::show_rows",
        |ui| {
            egui::ScrollArea::vertical()
                .id_salt("rows")
                .max_height(140.0)
                .auto_shrink([false, true])
                .show_rows(ui, 22.0, 10_000, |ui, range| {
                    for index in range {
                        if ui
                            .selectable_label(
                                s.selected_row == index,
                                format!("Row {:05}", index + 1),
                            )
                            .clicked()
                        {
                            s.selected_row = index;
                        }
                    }
                });
        },
    );
    card(ui, cx, &filter, Source::Egui, "Resizable", "Resize", |ui| {
        egui::Resize::default()
            .id_salt("resize")
            .default_size(egui::vec2(260.0, 70.0))
            .max_width(ui.available_width())
            .show(ui, |ui| {
                ui.label("Drag the corner.");
            });
    });
    card(ui, cx, &filter, Source::Egui, "Canvas", "Scene", |ui| {
        egui::Frame::canvas(ui.style()).show(ui, |ui| {
            ui.set_height(160.0);
            egui::Scene::new()
                .zoom_range(0.25..=4.0)
                .show(ui, &mut s.scene, |ui| {
                    ui.label("Pan and zoom me.");
                    let _ = ui.button("A button in a scene");
                    ui.painter().circle_stroke(
                        egui::pos2(160.0, 40.0),
                        24.0,
                        egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                    );
                });
        });
    });

    filter.section("Overlays");
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Tooltip",
        "on_hover_text",
        |ui| {
            ui.button("Hover me").on_hover_text("egui's plain tooltip");
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Animated tooltip",
        "UiFrame::tooltip · tooltip_ui",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                let f = cx.frame(ui);
                let plain = f.ui.button("Hover me");
                f.tooltip(&plain, "Expands from its source, with bed's motion policy.");
                let rich = f.ui.button("Rich tooltip");
                f.tooltip_ui(&rich, |mut f| {
                    f.ui.strong("Rich content");
                    f.icon(
                        BuiltinIcon::Info,
                        16.0,
                        theme::color(cx.theme.palette.accent),
                    );
                    f.ui.label("Any widgets can go here.");
                });
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Egui,
        "Menus",
        "menu_button · context_menu · Popup::menu",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.menu_button("Menu", |ui| {
                    let _ = ui.button("Open");
                    let _ = ui.add(egui::Button::new("Save").shortcut_text("⌘S"));
                    ui.menu_button("Recent", |ui| {
                        let _ = ui.button("main.rs");
                        let _ = ui.button("widgets.rs");
                    });
                    ui.separator();
                    ui.add_enabled(false, egui::Button::new("Unavailable"));
                });
                ui.button("Right-click me").context_menu(|ui| {
                    if ui.button("Copy").clicked() {
                        ui.close();
                    }
                    if ui.button("Paste").clicked() {
                        ui.close();
                    }
                });
                let response = ui.button("Popup ▾");
                egui::Popup::menu(&response).show(|ui| {
                    ui.label("Arbitrary popup content.");
                    ui.checkbox(&mut s.checked, "Even controls");
                });
            });
        },
    );
    card(ui, cx, &filter, Source::Egui, "Menu bar", "MenuBar", |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            for title in ["File", "Edit", "View"] {
                ui.menu_button(title, |ui| {
                    let _ = ui.button(format!("{title} something"));
                });
            }
        });
    });
    card(ui, cx, &filter, Source::Egui, "Dialog", "Modal", |ui| {
        if ui.button("Open dialog").clicked() {
            s.modal = true;
        }
    });
    if s.modal {
        let ctx = ui.ctx().clone();
        let modal = egui::Modal::new(egui::Id::new((cx.view, "widgets-modal"))).show(&ctx, |ui| {
            ui.set_width(320.0);
            ui.heading("Discard changes?");
            ui.label("This can't be undone.");
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if cx.frame(ui).primary_button("Discard").clicked() {
                    s.modal = false;
                }
                if ui.button("Cancel").clicked() {
                    s.modal = false;
                }
            });
        });
        if modal.should_close() {
            s.modal = false;
        }
    }

    filter.section("Data");
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Data tree",
        "data::DataTree::new(value).show(ui, cx)",
        |ui| {
            ui.weak("Click a container to open it; right-click to copy a value or path.");
            ui.allocate_ui(egui::vec2(ui.available_width(), 170.0), |ui| {
                ui.set_height(170.0);
                s.data_tree.show(ui, cx);
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Hex view",
        "hex::HexViewer::show(ui, cx, bytes, keys)",
        |ui| {
            ui.weak("Click and drag to select; right-click to copy, find or go to an offset.");
            ui.allocate_ui(egui::vec2(ui.available_width(), 150.0), |ui| {
                ui.set_height(150.0);
                s.hex.show(ui, cx, SAMPLE_JSON.as_bytes(), false);
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Markdown",
        "markdown::MarkdownView::new(text).code_language(\"rust\").show",
        |ui| {
            ui.checkbox(&mut s.compact, "Compact (tooltips)");
            s.markdown.style = if s.compact {
                bedtime_ui::markdown::MarkdownStyle::Compact
            } else {
                bedtime_ui::markdown::MarkdownStyle::Document
            };
            ui.allocate_ui(egui::vec2(ui.available_width(), 260.0), |ui| {
                ui.set_height(260.0);
                // The document style scrolls itself; compact is the caller's to.
                let clicked = if s.compact {
                    egui::ScrollArea::vertical()
                        .id_salt("markdown-card")
                        .auto_shrink(false)
                        .show(ui, |ui| s.markdown.show(ui, cx))
                        .inner
                } else {
                    s.markdown.show(ui, cx)
                };
                if let Some(link) = clicked {
                    requests.push(Request::Notify(Notification::info(format!(
                        "Clicked {link}"
                    ))));
                }
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Image view",
        "image::ImageView::show(ui, cx, &picture, keys)",
        |ui| {
            ui.weak("Scroll or pinch to zoom, drag to pan, double-click for actual size; right-click for backdrops.");
            ui.allocate_ui(egui::vec2(ui.available_width(), 180.0), |ui| {
                ui.set_height(180.0);
                s.image_view.show(ui, cx, &s.picture, false);
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Code view",
        "CodeView::new(text).language(\"json\").show",
        |ui| {
            ui.checkbox(&mut s.code_view.wrap, "Wrap");
            ui.weak("Select and copy; Cmd/Ctrl+F finds while the pointer is over it.");
            ui.allocate_ui(egui::vec2(ui.available_width(), 150.0), |ui| {
                ui.set_height(150.0);
                s.code_view.show(ui, cx);
            });
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Tree rows",
        "tree::paint_row",
        |ui| {
            let rows = [
                (0, "src", Some(BuiltinIcon::Folder), Some(0)),
                (1, "main.rs", Some(BuiltinIcon::Rust), None),
                (1, "widgets.rs", Some(BuiltinIcon::Rust), None),
                (0, "assets", Some(BuiltinIcon::Folder), Some(1)),
                (1, "theme.ron", Some(BuiltinIcon::Settings), None),
                (0, "Cargo.toml", Some(BuiltinIcon::Settings), None),
            ];
            let height = tree::row_height(cx);
            let mut hidden_below: Option<usize> = None;
            for (index, (depth, label, icon, folder)) in rows.into_iter().enumerate() {
                if hidden_below.is_some_and(|d| depth > d) {
                    continue;
                }
                hidden_below = None;
                let expanded = folder.map(|f| s.tree_open[f]);
                if expanded == Some(false) {
                    hidden_below = Some(depth);
                }
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), height),
                    egui::Sense::click(),
                );
                let row = tree::Row {
                    label: Some(label),
                    depth,
                    icon,
                    expanded,
                    selected: s.tree_selected == index,
                    ink: ui.visuals().text_color(),
                    icon_ink: cx.theme.palette.muted,
                };
                tree::paint_highlight(ui, ui.painter(), cx, rect, &response, row.selected);
                tree::paint_row(ui, cx, row, rect, Some(&response), 1.0, tree::INDENT);
                if response.clicked() {
                    s.tree_selected = index;
                    if let Some(f) = folder {
                        s.tree_open[f] = !s.tree_open[f];
                    }
                }
            }
        },
    );
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Drop target",
        "paint_drop_overlay",
        |ui| {
            let (rect, response) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 56.0), egui::Sense::hover());
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Hover to preview the drop treatment",
                egui::FontId::proportional(cx.metrics.font_size),
                theme::color(cx.theme.palette.muted),
            );
            if response.hovered() {
                bedtime_ui::paint_drop_overlay(
                    ui.painter(),
                    &cx.theme.palette,
                    rect,
                    cx.metrics.rounding,
                );
            }
        },
    );

    filter.section("Icons");
    card(
        ui,
        cx,
        &filter,
        Source::Bedtime,
        "Built-in icons",
        "UiFrame::icon · BuiltinIcon",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut s.icon_filter)
                        .hint_text("Filter icons")
                        .desired_width(160.0),
                );
                ui.add(egui::Slider::new(&mut s.icon_size, 12.0..=48.0).text("Size"));
                ui.label(
                    egui::RichText::new(format!("{} icons", BuiltinIcon::ALL.len()))
                        .small()
                        .weak(),
                );
            });
            ui.add_space(4.0);
            let tint = ui.visuals().text_color();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                for icon in BuiltinIcon::ALL
                    .iter()
                    .filter(|icon| icon.key().contains(&s.icon_filter))
                {
                    let response = cx.frame(ui).icon(*icon, s.icon_size, tint);
                    response.on_hover_text(icon.key());
                }
            });
        },
    );

    ui.add_space(24.0);
}

/// What the filter row lets through.
struct Filter {
    text: String,
    only: Option<Source>,
    /// The section heading to draw before the next card shown, so sections
    /// with nothing left after filtering disappear.
    section: std::cell::Cell<Option<&'static str>>,
}

impl Filter {
    fn section(&self, title: &'static str) {
        self.section.set(Some(title));
    }

    fn shows(&self, source: Source, title: &str, api: &str) -> bool {
        self.only.is_none_or(|only| only == source)
            && (self.text.is_empty()
                || title.to_lowercase().contains(&self.text)
                || api.to_lowercase().contains(&self.text)
                || source.label().contains(&self.text))
    }
}

fn heading(ui: &mut egui::Ui, cx: &UiContext<'_>, title: &str) {
    ui.add_space(10.0);
    ui.label(
        egui::RichText::new(title.to_uppercase())
            .small()
            .strong()
            .color(theme::color(cx.theme.palette.muted)),
    );
    ui.add_space(2.0);
}

/// One widget's card: title, source tag and API on top, the demo below.
fn card(
    ui: &mut egui::Ui,
    cx: &UiContext<'_>,
    filter: &Filter,
    source: Source,
    title: &str,
    api: &str,
    body: impl FnOnce(&mut egui::Ui),
) {
    if !filter.shows(source, title, api) {
        return;
    }
    if let Some(section) = filter.section.take() {
        heading(ui, cx, section);
    }
    let p = &cx.theme.palette;
    egui::Frame::NONE
        .fill(theme::color(p.surface))
        .stroke(egui::Stroke::new(1.0, theme::color(p.border)))
        .corner_radius(cx.metrics.rounding.max(6.0))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.push_id(title, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(title).strong());
                    tag(ui, cx, source);
                });
                ui.add(egui::Label::new(
                    egui::RichText::new(api)
                        .monospace()
                        .small()
                        .color(theme::color(p.muted)),
                ));
                ui.add_space(8.0);
                body(ui);
            });
        });
    ui.add_space(8.0);
}

/// A small rounded label saying where a widget comes from.
fn tag(ui: &mut egui::Ui, cx: &UiContext<'_>, source: Source) {
    let p = &cx.theme.palette;
    let hue = match source {
        Source::Egui => p.muted,
        Source::Bedtime => p.accent,
        Source::Workbench => p.warning,
    };
    let font = egui::FontId::proportional(cx.metrics.font_size * 0.78);
    let galley = ui
        .painter()
        .layout_no_wrap(source.label().into(), font, theme::color(hue));
    let size = galley.size() + egui::vec2(12.0, 4.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let mut fill = hue;
    fill[3] *= 0.16;
    ui.painter()
        .rect_filled(rect, size.y / 2.0, theme::color(fill));
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        theme::color(hue),
    );
}

/// A named color chip.
fn swatch(ui: &mut egui::Ui, cx: &UiContext<'_>, name: &str, color: egui::Color32) {
    ui.vertical(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(64.0, 28.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 4.0, color);
        ui.painter().rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0, theme::color(cx.theme.palette.border)),
            egui::StrokeKind::Inside,
        );
        ui.label(egui::RichText::new(name).small().weak());
    });
}

/// The code view card's text.
const SAMPLE_JSON: &str = r#"{
  "name": "bed",
  "version": 1,
  "widgets": ["code view", "tree rows", "icons"],
  "colours": { "from_theme": true, "roles": 14 },
  "note": "Long lines wrap unless Wrap is off, and then the view scrolls sideways to show the rest of them."
}"#;

/// The image view card's picture: a 96 × 64 gradient, partly transparent so
/// the backdrop shows.
fn gradient() -> bedtime_ui::image::Decoded {
    let (width, height) = (96u32, 64u32);
    let rgba: Vec<u8> = (0..height)
        .flat_map(|y| {
            (0..width).flat_map(move |x| {
                let r = (x * 255 / (width - 1)) as u8;
                let b = (y * 255 / (height - 1)) as u8;
                let a = if (x / 16 + y / 16) % 2 == 0 { 255 } else { 140 };
                [r, 160, b, a]
            })
        })
        .collect();
    bedtime_ui::image::Decoded {
        size: [width, height],
        format: "RGBA",
        frames: vec![bedtime_ui::image::Frame {
            size: [width, height],
            rgba: rgba.into(),
            delay: std::time::Duration::ZERO,
        }],
    }
}

/// The Markdown card's text.
const SAMPLE_MARKDOWN: &str = "## Markdown view\n\nParagraphs with *emphasis*, **strong**, `code` and \
[links](https://example.org).\n\n- [x] Task lists\n- Nested\n  1. numbered\n\n\
> [!TIP]\n> Callouts, as on GitHub.\n\n```\nfn main() {\n    println!(\"coloured as rust\");\n}\n```\n\n\
| Style | For |\n|---|---|\n| Document | previews |\n| Compact | tooltips |\n";
