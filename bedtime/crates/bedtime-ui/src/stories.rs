//! Stories: each component shown on its own, with the knobs that change it.
//! A playground (`src/bedui`) lists them; a story is a function, so it's
//! also a usage example that has to compile.
use crate::{BuiltinIcon, Theme, prelude::*, theme};
use std::collections::HashMap;

/// A component, shown: `draw` adds it to the `Ui`, asking `Knobs` for what
/// can be changed.
#[derive(Clone, Copy)]
pub struct Story {
    pub group: &'static str,
    pub name: &'static str,
    pub draw: fn(&mut egui::Ui, &mut Knobs),
}

/// bedtime-ui's own stories.
pub fn builtin() -> Vec<Story> {
    vec![
        Story {
            group: "Foundations",
            name: "Tokens",
            draw: tokens,
        },
        Story {
            group: "Components",
            name: "Button",
            draw: button,
        },
        Story {
            group: "Components",
            name: "Text",
            draw: text,
        },
        Story {
            group: "Components",
            name: "Checkbox and Toggle",
            draw: checks,
        },
        Story {
            group: "Components",
            name: "Text Field",
            draw: text_field,
        },
        Story {
            group: "Components",
            name: "Select",
            draw: select,
        },
        Story {
            group: "Components",
            name: "Segmented",
            draw: segmented,
        },
        Story {
            group: "Components",
            name: "Slider",
            draw: slider,
        },
        Story {
            group: "Components",
            name: "Color Field",
            draw: color_field,
        },
        Story {
            group: "Components",
            name: "Text Area",
            draw: text_area,
        },
        Story {
            group: "Components",
            name: "List Row and Menu",
            draw: lists,
        },
        Story {
            group: "Components",
            name: "Breadcrumbs",
            draw: breadcrumbs,
        },
        Story {
            group: "Components",
            name: "Table",
            draw: table,
        },
        Story {
            group: "Components",
            name: "Spinner",
            draw: spinner,
        },
        Story {
            group: "Layout",
            name: "Section and Separator",
            draw: sections,
        },
        Story {
            group: "Layout",
            name: "Row and Column",
            draw: stacks,
        },
    ]
}

/// A story's adjustable values. A story asks for each as it draws
/// (`knobs.toggle("Disabled", false)`); the values are kept between frames
/// and `controls` draws their editors.
#[derive(Clone, Default)]
pub struct Knobs {
    values: HashMap<&'static str, Value>,
    declared: Vec<(&'static str, Vec<String>)>,
}

#[derive(Clone)]
enum Value {
    Toggle(bool),
    Choice(usize),
    Text(String),
}

impl Knobs {
    /// Before a story draws: forget which knobs the last frame asked for.
    pub fn begin(&mut self) {
        self.declared.clear();
    }

    fn declare(&mut self, name: &'static str, options: Vec<String>, initial: Value) -> &Value {
        if !self.declared.iter().any(|(declared, _)| *declared == name) {
            self.declared.push((name, options));
        }
        self.values.entry(name).or_insert(initial)
    }

    pub fn toggle(&mut self, name: &'static str, initial: bool) -> bool {
        match self.declare(name, Vec::new(), Value::Toggle(initial)) {
            Value::Toggle(value) => *value,
            _ => initial,
        }
    }

    /// One of `options` (the first, to start with), named by its `Debug`.
    pub fn choice<T: Copy + std::fmt::Debug>(&mut self, name: &'static str, options: &[T]) -> T {
        let names = options.iter().map(|option| format!("{option:?}")).collect();
        match self.declare(name, names, Value::Choice(0)) {
            Value::Choice(index) => options[(*index).min(options.len() - 1)],
            _ => options[0],
        }
    }

    pub fn text(&mut self, name: &'static str, initial: &str) -> String {
        match self.declare(name, Vec::new(), Value::Text(initial.into())) {
            Value::Text(value) => value.clone(),
            _ => initial.into(),
        }
    }

    /// Editors for the knobs the story asked for, in the order it did.
    pub fn controls(&mut self, ui: &mut egui::Ui) {
        if self.declared.is_empty() {
            ui.add(Text::new("This story has no controls").muted());
            return;
        }
        Column::new().gap(Space::Md).show(ui, |ui| {
            for (name, options) in &self.declared {
                let Some(value) = self.values.get_mut(name) else {
                    continue;
                };
                Column::new().gap(Space::Xs).show(ui, |ui| match value {
                    Value::Toggle(on) => {
                        ui.add(Checkbox::new(on, *name));
                    }
                    Value::Text(text) => {
                        ui.add(Text::caption(*name).muted());
                        ui.add(TextField::new(text).label(*name).fill_width());
                    }
                    Value::Choice(chosen) => {
                        ui.add(Text::caption(*name).muted());
                        Row::new().gap(Space::Xs).wrap().show(ui, |ui| {
                            for (index, option) in options.iter().enumerate() {
                                let button = Button::new(option)
                                    .small()
                                    .ghost()
                                    .selected(*chosen == index);
                                if ui.add(button).clicked() {
                                    *chosen = index;
                                }
                            }
                        });
                    }
                });
            }
        });
    }
}

fn section(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    Column::new().gap(Space::Sm).show(ui, |ui| {
        ui.add(Text::caption(title).muted());
        content(ui);
    });
}

/// A story's own state (what its controls edit), kept by egui between frames.
fn stateful<T: Clone + Send + Sync + 'static>(
    ui: &mut egui::Ui,
    name: &'static str,
    initial: impl FnOnce() -> T,
    content: impl FnOnce(&mut egui::Ui, &mut T),
) {
    let id = egui::Id::new(("bedtime-story", name));
    let mut state = ui
        .data(|data| data.get_temp::<T>(id))
        .unwrap_or_else(initial);
    content(ui, &mut state);
    ui.data_mut(|data| data.insert_temp(id, state));
}

fn checks(ui: &mut egui::Ui, knobs: &mut Knobs) {
    let label = knobs.text("Label", "Wrap long lines");
    let size = knobs.choice("Size", &[Size::Md, Size::Sm, Size::Lg]);
    let disabled = knobs.toggle("Disabled", false);

    stateful(
        ui,
        "checks",
        || [true, false, true, false],
        |ui, values| {
            Column::new().gap(Space::Xl).show(ui, |ui| {
                section(ui, "Checkbox", |ui| {
                    ui.add_enabled(!disabled, Checkbox::new(&mut values[0], &label).size(size));
                    ui.add_enabled(
                        !disabled,
                        Checkbox::new(&mut values[1], "Show hidden files").size(size),
                    );
                    let (all, any) = (values[0] && values[1], values[0] || values[1]);
                    let mut both = all;
                    let some = Checkbox::new(&mut both, "Both of the above")
                        .size(size)
                        .indeterminate(any && !all);
                    if ui.add_enabled(!disabled, some).changed() {
                        (values[0], values[1]) = (both, both);
                    }
                });
                section(ui, "Toggle", |ui| {
                    ui.add_enabled(
                        !disabled,
                        Toggle::new(&mut values[2], "Autosave").size(size),
                    );
                    ui.add_enabled(
                        !disabled,
                        Toggle::new(&mut values[3], "Reduce motion").size(size),
                    );
                });
                section(ui, "Every size", |ui| {
                    Row::new().gap(Space::Lg).show(ui, |ui| {
                        for size in Size::ALL {
                            ui.add(Checkbox::new(&mut values[0], format!("{size:?}")).size(size));
                            ui.add(Toggle::new(&mut values[2], format!("{size:?}")).size(size));
                        }
                    });
                });
            });
        },
    );
}

fn text_field(ui: &mut egui::Ui, knobs: &mut Knobs) {
    let hint = knobs.text("Hint", "Search files");
    let size = knobs.choice("Size", &[Size::Md, Size::Sm, Size::Lg]);
    let icon = knobs.toggle("Icon", true);
    let clearable = knobs.toggle("Clearable", true);
    let fill = knobs.toggle("Fill width", false);
    let invalid = knobs.toggle("Invalid", false);
    let disabled = knobs.toggle("Disabled", false);

    let initial = || {
        [
            String::new(),
            "https://example.com".to_owned(),
            "hunter2".to_owned(),
            "Read from the panel".to_owned(),
        ]
    };
    stateful(ui, "text-field", initial, |ui, values| {
        Column::new().gap(Space::Xl).show(ui, |ui| {
            section(ui, "With the controls", |ui| {
                let mut field = TextField::new(&mut values[0])
                    .hint(&hint)
                    .size(size)
                    .invalid(invalid);
                if icon {
                    field = field.icon(BuiltinIcon::Search);
                }
                if clearable {
                    field = field.clearable();
                }
                if fill {
                    field = field.fill_width();
                }
                ui.add_enabled(!disabled, field);
            });
            section(ui, "Every size", |ui| {
                Row::new().gap(Space::Sm).show(ui, |ui| {
                    for size in Size::ALL {
                        ui.add(
                            TextField::new(&mut values[1])
                                .label("Address")
                                .size(size)
                                .width(180.0),
                        );
                    }
                });
            });
            section(ui, "Beside a button", |ui| {
                Row::new().gap(Space::Sm).show(ui, |ui| {
                    ui.add(TextField::new(&mut values[1]).label("Address").width(260.0));
                    ui.add(Button::new("Send").primary());
                });
            });
            section(
                ui,
                "Plain (a title edited in place) and muted (not in use)",
                |ui| {
                    ui.add(
                        TextField::new(&mut values[1])
                            .label("Title")
                            .size(Size::Lg)
                            .plain()
                            .width(320.0),
                    );
                    ui.add(TextField::new(&mut values[1]).label("Unused").muted(true));
                },
            );
            section(ui, "Password", |ui| {
                ui.add(TextField::new(&mut values[2]).label("Password").password());
            });
            section(ui, "Editing what's only read (ui.edit)", |ui| {
                // As a panel does: draw from a value it can't change, and
                // act on the new one.
                let read_only: &String = &values[3].clone();
                if let Some(edited) = ui.edit(read_only, |ui, text| {
                    ui.add(TextField::new(text).label("Read-only state"))
                }) {
                    values[3] = edited;
                }
                ui.add(Text::caption(format!("The state is \"{}\"", values[3])).muted());
            });
        });
    });
}

const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];
const LANGUAGES: [&str; 24] = [
    "Bash",
    "C",
    "C++",
    "CSS",
    "Dart",
    "Elixir",
    "Go",
    "Haskell",
    "HTML",
    "Java",
    "JavaScript",
    "JSON",
    "Kotlin",
    "Lua",
    "Markdown",
    "OCaml",
    "Python",
    "Ruby",
    "Rust",
    "Swift",
    "TOML",
    "TypeScript",
    "YAML",
    "Zig",
];

fn select(ui: &mut egui::Ui, knobs: &mut Knobs) {
    let size = knobs.choice("Size", &[Size::Md, Size::Sm, Size::Lg]);
    let fill = knobs.toggle("Fill width", false);
    let disabled = knobs.toggle("Disabled", false);

    stateful(
        ui,
        "select",
        || ("GET", "Rust", "Nothing"),
        |ui, values| {
            Column::new().gap(Space::Xl).show(ui, |ui| {
                section(ui, "With the controls", |ui| {
                    let mut select = Select::new(&mut values.0, &METHODS).size(size);
                    if fill {
                        select = select.fill_width();
                    }
                    ui.add_enabled(!disabled, select);
                    ui.add(Text::caption(format!("Chosen: {}", values.0)).muted());
                });
                section(
                    ui,
                    "Searchable (type to narrow, arrows and Enter to choose)",
                    |ui| {
                        ui.add_enabled(
                            !disabled,
                            Select::new(&mut values.1, &LANGUAGES)
                                .size(size)
                                .searchable(),
                        );
                        ui.add(Text::caption(format!("Chosen: {}", values.1)).muted());
                    },
                );
                section(ui, "Nothing chosen yet", |ui| {
                    ui.add(Select::new(&mut values.2, &METHODS).placeholder("Choose a method…"));
                });
                section(ui, "In a row", |ui| {
                    Row::new().gap(Space::Sm).show(ui, |ui| {
                        ui.add(Select::new(&mut values.0, &METHODS));
                        let mut address = "https://example.com".to_owned();
                        ui.add(TextField::new(&mut address).label("Address").width(240.0));
                        ui.add(Button::new("Send").primary());
                    });
                });
            });
        },
    );
}

const VIEWS: [&str; 6] = ["Graph", "Lanes", "Arcs", "Timeline", "Flame", "List"];

fn segmented(ui: &mut egui::Ui, knobs: &mut Knobs) {
    let size = knobs.choice("Size", &[Size::Md, Size::Sm, Size::Lg]);
    let disabled = knobs.toggle("Disabled", false);

    stateful(
        ui,
        "segmented",
        || ("Graph", "Week", false),
        |ui, values| {
            Column::new().gap(Space::Xl).show(ui, |ui| {
                section(ui, "With the controls", |ui| {
                    ui.add_enabled(!disabled, Segmented::new(&mut values.0, &VIEWS).size(size));
                    ui.add(Text::caption(format!("Chosen: {}", values.0)).muted());
                });
                section(ui, "Every size", |ui| {
                    for size in Size::ALL {
                        ui.add(Segmented::new(&mut values.1, &["Day", "Week", "Month"]).size(size));
                    }
                });
                section(ui, "Two options, named by a function", |ui| {
                    ui.add(Segmented::with(&mut values.2, &[false, true], |slowest| {
                        if *slowest { "slowest" } else { "latest" }.into()
                    }));
                });
                section(ui, "Too narrow for a row: the segments wrap", |ui| {
                    ui.scope(|ui| {
                        ui.set_max_width(220.0);
                        ui.add(Segmented::new(&mut values.0, &VIEWS));
                    });
                });
            });
        },
    );
}

fn sections(ui: &mut egui::Ui, _: &mut Knobs) {
    stateful(
        ui,
        "sections",
        || [true, false, true, true],
        |ui, values| {
            Column::new().gap(Space::Xl).show(ui, |ui| {
                section(ui, "Section", |ui| {
                    ui.scope(|ui| {
                        ui.set_max_width(320.0);
                        Section::new("Show").open().show(ui, |ui| {
                            ui.add(Checkbox::new(&mut values[0], "Internal systems"));
                            ui.add(Checkbox::new(&mut values[1], "Sync points"));
                        });
                        Section::new("Edges").show(ui, |ui| {
                            ui.add(Checkbox::new(&mut values[2], "Ordering"));
                            ui.add(Checkbox::new(&mut values[3], "Data flow"));
                        });
                        Section::new("One inside another").show(ui, |ui| {
                            ui.add(Text::new("Sections nest."));
                            Section::new("Inner").open().show(ui, |ui| {
                                ui.add(Text::new("As deep as it's useful.").muted());
                            });
                        });
                    });
                });
                section(ui, "Separator, in a column", |ui| {
                    ui.scope(|ui| {
                        ui.set_max_width(320.0);
                        ui.add(Text::new("71 systems"));
                        ui.add(Separator::new());
                        ui.add(Text::new("6 sets"));
                        ui.add(Separator::new().space(Space::Xl));
                        ui.add(Text::new("26 ordering edges").muted());
                    });
                });
                section(ui, "Separator, in a row", |ui| {
                    Row::new().show(ui, |ui| {
                        ui.add(Button::new("Cut").ghost());
                        ui.add(Button::new("Copy").ghost());
                        ui.add(Separator::new());
                        ui.add(Button::new("Paste").ghost());
                    });
                });
            });
        },
    );
}

fn slider(ui: &mut egui::Ui, knobs: &mut Knobs) {
    let size = knobs.choice("Size", &[Size::Md, Size::Sm, Size::Lg]);
    let fill = knobs.toggle("Fill width", false);
    let disabled = knobs.toggle("Disabled", false);

    stateful(
        ui,
        "slider",
        || (0.65_f32, 4_i32, 1.0_f32),
        |ui, values| {
            Column::new().gap(Space::Xl).show(ui, |ui| {
                section(ui, "With the controls", |ui| {
                    let mut slider = Slider::new(&mut values.0, 0.0..=1.0)
                        .label("Opacity")
                        .size(size);
                    if fill {
                        slider = slider.fill_width();
                    }
                    ui.add_enabled(!disabled, slider);
                    ui.add(Text::caption(format!("Opacity: {:.2}", values.0)).muted());
                });
                section(ui, "Whole numbers", |ui| {
                    Row::new().show(ui, |ui| {
                        ui.add(Slider::new(&mut values.1, 1..=8).label("Tab width"));
                        ui.add(Text::new(format!("{} spaces", values.1)));
                    });
                });
                section(ui, "Beside a NumberField: drag it or type it", |ui| {
                    Row::new().show(ui, |ui| {
                        ui.add(Slider::new(&mut values.0, 0.0..=1.0).label("Opacity"));
                        ui.add(
                            NumberField::new(&mut values.0)
                                .range(0.0..=1.0)
                                .label("Opacity"),
                        );
                    });
                    Row::new().show(ui, |ui| {
                        ui.add(Slider::new(&mut values.1, 1..=8).label("Tab width"));
                        ui.add(
                            NumberField::new(&mut values.1)
                                .range(1..=8)
                                .suffix(" spaces")
                                .width(110.0),
                        );
                    });
                });
                section(ui, "Logarithmic (0.25 to 64)", |ui| {
                    Row::new().show(ui, |ui| {
                        ui.add(
                            Slider::new(&mut values.2, 0.25..=64.0)
                                .logarithmic()
                                .label("Zoom"),
                        );
                        ui.add(Text::new(format!("{:.2}×", values.2)));
                        if ui.add(Button::new("1×").small()).clicked() {
                            values.2 = 1.0;
                        }
                    });
                });
            });
        },
    );
}

fn color_field(ui: &mut egui::Ui, knobs: &mut Knobs) {
    let size = knobs.choice("Size", &[Size::Md, Size::Sm, Size::Lg]);
    let initial = || {
        let palette = &Theme::dark().palette;
        [palette.accent, palette.error, palette.selection]
    };
    stateful(ui, "color-field", initial, |ui, colors| {
        Column::new().gap(Space::Xl).show(ui, |ui| {
            section(ui, "Click the swatch to pick, or type the hex", |ui| {
                Table::new("colors").columns(2).show(ui, |table| {
                    for (name, color) in ["Accent", "Error", "Selection"]
                        .iter()
                        .zip(colors.iter_mut())
                    {
                        table.row(|ui| {
                            ui.add(Text::new(*name));
                            ui.add(ColorField::new(color).size(size));
                        });
                    }
                });
            });
            section(ui, "What they are", |ui| {
                Row::new().show(ui, |ui| {
                    for color in colors.iter() {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(64.0, 32.0), egui::Sense::hover());
                        ui.painter()
                            .rect_filled(rect, Radius::Md.px(ui), theme::color(*color));
                    }
                });
            });
        });
    });
}

fn text_area(ui: &mut egui::Ui, knobs: &mut Knobs) {
    let hint = knobs.text("Hint", "Request body");
    let fill = knobs.toggle("Fill width", false);
    let invalid = knobs.toggle("Invalid", false);
    let disabled = knobs.toggle("Disabled", false);

    let initial = || {
        (
            String::new(),
            "{\n  \"name\": \"bed\",\n  \"ok\": true\n}".to_owned(),
        )
    };
    stateful(ui, "text-area", initial, |ui, values| {
        Column::new().gap(Space::Xl).show(ui, |ui| {
            section(ui, "With the controls", |ui| {
                let mut area = TextArea::new(&mut values.0)
                    .hint(&hint)
                    .label("Notes")
                    .invalid(invalid);
                if fill {
                    area = area.fill_width();
                }
                ui.add_enabled(!disabled, area);
            });
            section(ui, "Taller, with text", |ui| {
                ui.add(
                    TextArea::new(&mut values.1)
                        .label("Body")
                        .rows(6)
                        .width(360.0),
                );
            });
        });
    });
}

fn lists(ui: &mut egui::Ui, _: &mut Knobs) {
    let theme = Theme::of(ui);
    let methods = [
        ("GET", Tone::Success, "List users"),
        (
            "POST",
            Tone::Warning,
            "Create a user with a rather long name that's cut short",
        ),
        ("DELETE", Tone::Error, "Remove a user"),
    ];
    stateful(
        ui,
        "lists",
        || (0_usize, String::new()),
        |ui, values| {
            Column::new().gap(Space::Xl).show(ui, |ui| {
                section(
                    ui,
                    "Rows: a tag, text cut to fit, a spinner, and a menu on right-click",
                    |ui| {
                        ui.scope(|ui| {
                            ui.set_max_width(320.0);
                            ui.spacing_mut().item_spacing.y = 1.0;
                            for (index, (method, tone, name)) in methods.iter().enumerate() {
                                let color = theme::color(tone.rgba(&theme.palette));
                                let row = ListRow::new(*name)
                                    .tag(*method, color, "DELETE")
                                    .selected(values.0 == index)
                                    .busy(index == 1);
                                let response = ui.add(row);
                                if response.clicked() {
                                    values.0 = index;
                                }
                                response.context_menu(|ui| {
                                    if ui.add(MenuItem::new("Duplicate").shortcut("⌘D")).clicked()
                                    {
                                        values.1 = format!("Duplicated {name}");
                                    }
                                    if ui.add(MenuItem::new("Delete…").danger()).clicked() {
                                        values.1 = format!("Deleted {name}");
                                    }
                                });
                            }
                        });
                        if !values.1.is_empty() {
                            ui.add(Text::caption(&values.1).muted());
                        }
                    },
                );
                section(
                    ui,
                    "A button that opens a menu (MenuButton), and a row with a detail",
                    |ui| {
                        Row::new().show(ui, |ui| {
                            MenuButton::new(Button::new("main").dropdown()).show(ui, |ui| {
                                let _ = ui.add(MenuItem::new("develop"));
                                let _ = ui.add(MenuItem::new("main").checked(true));
                            });
                            MenuButton::new(
                                Button::icon_only(BuiltinIcon::Settings, "Options").ghost(),
                            )
                            .keep_open()
                            .show(ui, |ui| {
                                ui.add(Text::caption("View as").muted());
                                let _ = ui.add(MenuItem::new("List").checked(false).keep_open());
                                let _ = ui.add(MenuItem::new("Tree").checked(true).keep_open());
                            });
                            ui.add(Button::new("A long name, cut to fit").width(140.0));
                        });
                        ui.scope(|ui| {
                            ui.set_max_width(320.0);
                            ui.add(ListRow::new("deep.rs").detail("src/workbench/panel"));
                        });
                    },
                );
                section(ui, "Rows: an icon, a count, struck through, italic", |ui| {
                    ui.scope(|ui| {
                        ui.set_max_width(320.0);
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.add(ListRow::new("src").icon(BuiltinIcon::Folder).trailing("12"));
                        ui.add(
                            ListRow::new("Cargo.toml")
                                .icon(BuiltinIcon::Config)
                                .selected(true),
                        );
                        ui.add(ListRow::new("A hidden set").strikethrough(true));
                        ui.add(ListRow::new("A resource").italic(true).trailing("3w 5r"));
                    });
                });
                section(ui, "Menu items, as a menu shows them", |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_width(220.0);
                        ui.spacing_mut().item_spacing.y = 1.0;
                        let _ = ui.add(
                            MenuItem::new("New File")
                                .icon(BuiltinIcon::NewFile)
                                .shortcut("⌘N"),
                        );
                        let _ =
                            ui.add(MenuItem::new("Save").icon(BuiltinIcon::Save).shortcut("⌘S"));
                        ui.add(Separator::new().space(Space::Xs));
                        let _ = ui.add(MenuItem::new("Delete…").icon(BuiltinIcon::Trash).danger());
                    });
                });
                section(ui, "Checked items (a tick when on)", |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_width(220.0);
                        ui.spacing_mut().item_spacing.y = 1.0;
                        let _ = ui.add(MenuItem::new("Show Hidden Files").checked(false));
                        let _ = ui.add(MenuItem::new("Show Ignored Files").checked(true));
                    });
                });
            });
        },
    );
}

fn breadcrumbs(ui: &mut egui::Ui, knobs: &mut Knobs) {
    let size = knobs.choice("Size", &[Size::Sm, Size::Md, Size::Lg]);
    let path = ["bed", "bedtime", "crates", "bedtime-ui", "src", "widgets"];
    stateful(
        ui,
        "breadcrumbs",
        || path.len() - 1,
        |ui, at| {
            Column::new().gap(Space::Xl).show(ui, |ui| {
                section(ui, "With the controls (click one to go there)", |ui| {
                    if let Some(index) = Breadcrumbs::new(path[..=*at].iter().copied())
                        .size(size)
                        .show(ui)
                    {
                        *at = index;
                    }
                    if *at + 1 < path.len() && ui.add(Button::new("Back down").small()).clicked() {
                        *at = path.len() - 1;
                    }
                });
                section(ui, "Too narrow: the first ones go", |ui| {
                    ui.scope(|ui| {
                        ui.set_max_width(220.0);
                        outlined(ui, |ui| {
                            ui.set_width(220.0);
                            let _ = Breadcrumbs::new(path).size(size).show(ui);
                        });
                    });
                });
            });
        },
    );
}

fn table(ui: &mut egui::Ui, knobs: &mut Knobs) {
    let striped = knobs.toggle("Striped", false);
    let gap = knobs.choice("Gap", &[Space::Lg, Space::Sm, Space::Md, Space::Xl]);
    let columns = [
        ("id", "INTEGER", "primary key"),
        ("title", "VARCHAR(80)", "not null, default 'Untitled'"),
        ("artist_id", "INTEGER", "references artist(id)"),
        ("year", "INTEGER", ""),
    ];
    Column::new().gap(Space::Xl).show(ui, |ui| {
        section(ui, "With the controls", |ui| {
            let mut table = Table::new("columns")
                .headers(["Column", "Type", "Constraints"])
                .gap(gap);
            if striped {
                table = table.striped();
            }
            table.show(ui, |table| {
                for (name, declared, constraints) in columns {
                    table.row(|ui| {
                        ui.add(Text::new(name));
                        ui.add(Text::new(declared));
                        ui.add(Text::new(constraints));
                    });
                }
            });
        });
        section(
            ui,
            "Without headings, striped, with other things in its cells",
            |ui| {
                Table::new("headers").striped().show(ui, |table| {
                    for (name, value, tone) in [
                        ("content-type", "application/json", Tone::Neutral),
                        ("x-status", "ok", Tone::Success),
                        ("x-warning", "deprecated", Tone::Warning),
                    ] {
                        table.row(|ui| {
                            ui.add(Text::new(name).strong());
                            ui.add(Text::new(value).tone(tone));
                            ui.add(Button::new("Copy").small().ghost());
                        });
                    }
                });
            },
        );
    });
}

fn spinner(ui: &mut egui::Ui, _: &mut Knobs) {
    Column::new().gap(Space::Xl).show(ui, |ui| {
        section(ui, "Every size", |ui| {
            Row::new().gap(Space::Lg).show(ui, |ui| {
                for size in Size::ALL {
                    ui.add(Spinner::new().size(size));
                }
            });
        });
        section(ui, "Beside text", |ui| {
            Row::new().show(ui, |ui| {
                ui.add(Spinner::new());
                ui.add(Text::new("Sending… 1.2 s"));
            });
        });
    });
}

fn button(ui: &mut egui::Ui, knobs: &mut Knobs) {
    let label = knobs.text("Label", "Save changes");
    let variant = knobs.choice("Variant", &Variant::ALL);
    let size = knobs.choice("Size", &Size::ALL);
    let icon = knobs.toggle("Icon", false);
    let fill = knobs.toggle("Fill width", false);
    let disabled = knobs.toggle("Disabled", false);

    Column::new().gap(Space::Xl).show(ui, |ui| {
        section(ui, "With the controls", |ui| {
            let mut button = Button::new(label).variant(variant).size(size);
            if icon {
                button = button.icon(BuiltinIcon::Plus);
            }
            if fill {
                button = button.fill_width();
            }
            ui.add_enabled(!disabled, button);
        });
        section(ui, "Every variant and size", |ui| {
            for size in Size::ALL {
                Row::new().gap(Space::Sm).show(ui, |ui| {
                    for variant in Variant::ALL {
                        let button = Button::new(format!("{variant:?}"))
                            .variant(variant)
                            .size(size);
                        ui.add_enabled(!disabled, button);
                    }
                });
            }
        });
        section(ui, "With icons", |ui| {
            Row::new().gap(Space::Sm).show(ui, |ui| {
                ui.add(Button::new("New file").icon(BuiltinIcon::Plus).primary());
                ui.add(Button::new("Open folder").icon(BuiltinIcon::FolderOpen));
                ui.add(Button::new("Refresh").icon(BuiltinIcon::RefreshCw).ghost());
                ui.add(Button::new("Delete").icon(BuiltinIcon::Trash).danger());
            });
            Row::new().gap(Space::Xs).show(ui, |ui| {
                for size in Size::ALL {
                    ui.add(Button::icon_only(BuiltinIcon::Settings, "Settings").size(size));
                    ui.add(
                        Button::icon_only(BuiltinIcon::Close, "Close")
                            .size(size)
                            .ghost(),
                    );
                }
            });
        });
    });
}

fn text(ui: &mut egui::Ui, knobs: &mut Knobs) {
    let sample = knobs.text("Text", "The quick brown fox");
    let role = knobs.choice("Role", &TextRole::ALL);
    let tone = knobs.choice("Tone", &Tone::ALL);
    let muted = knobs.toggle("Muted", false);

    Column::new().gap(Space::Xl).show(ui, |ui| {
        section(ui, "With the controls", |ui| {
            let mut text = Text::new(&sample).role(role).tone(tone);
            if muted {
                text = text.muted();
            }
            ui.add(text);
        });
        section(ui, "Roles", |ui| {
            for role in TextRole::ALL {
                ui.add(Text::new(format!("{role:?}: {sample}")).role(role));
            }
        });
        section(ui, "Tones", |ui| {
            for tone in Tone::ALL {
                ui.add(Text::new(format!("{tone:?}: {sample}")).tone(tone));
            }
            ui.add(Text::new(format!("Muted: {sample}")).muted());
        });
    });
}

/// A filled box, to show where a layout puts things.
fn block(ui: &mut egui::Ui, width: f32, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let palette = &Theme::of(ui).palette;
    let fill = theme::mix(palette.panel_background, palette.accent, 0.45);
    ui.painter()
        .rect_filled(rect, Radius::Sm.px(ui), theme::color(fill));
}

/// An outline around what `content` draws, to show a layout's padding.
fn outlined(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    let border = theme::color(Theme::of(ui).palette.border);
    egui::Frame::NONE
        .stroke(egui::Stroke::new(1.0, border))
        .corner_radius(Radius::Md.px(ui))
        .show(ui, content);
}

fn stacks(ui: &mut egui::Ui, knobs: &mut Knobs) {
    let gap = knobs.choice(
        "Gap",
        &[
            Space::Sm,
            Space::None,
            Space::Xs,
            Space::Md,
            Space::Lg,
            Space::Xl,
        ],
    );
    let padding = knobs.choice("Padding", &Space::ALL);
    let align = knobs.choice(
        "Align",
        &[egui::Align::Min, egui::Align::Center, egui::Align::Max],
    );

    Column::new().gap(Space::Xl).show(ui, |ui| {
        section(ui, "Row", |ui| {
            outlined(ui, |ui| {
                Row::new()
                    .gap(gap)
                    .padding(padding)
                    .align(align)
                    .show(ui, |ui| {
                        block(ui, 64.0, 24.0);
                        block(ui, 40.0, 48.0);
                        block(ui, 88.0, 32.0);
                    });
            });
        });
        section(ui, "Column", |ui| {
            outlined(ui, |ui| {
                Column::new()
                    .gap(gap)
                    .padding(padding)
                    .align(align)
                    .show(ui, |ui| {
                        block(ui, 160.0, 20.0);
                        block(ui, 96.0, 20.0);
                        block(ui, 128.0, 20.0);
                    });
            });
        });
        section(ui, "A row that wraps", |ui| {
            outlined(ui, |ui| {
                Row::new().gap(gap).padding(padding).wrap().show(ui, |ui| {
                    for index in 0..14 {
                        block(ui, 48.0 + (index % 4) as f32 * 16.0, 24.0);
                    }
                });
            });
        });
        section(ui, "Together", |ui| {
            Row::new().gap(Space::Sm).show(ui, |ui| {
                ui.add(Button::new("Cancel"));
                ui.add(Button::new("Save").primary());
                ui.add(Text::new("Saved a minute ago").muted());
            });
        });
    });
}

fn tokens(ui: &mut egui::Ui, _: &mut Knobs) {
    let theme = Theme::of(ui);
    Column::new().gap(Space::Xl).show(ui, |ui| {
        section(ui, "Space", |ui| {
            for space in Space::ALL {
                Row::new().gap(Space::Md).show(ui, |ui| {
                    let points = space.px(ui);
                    ui.add(Text::new(format!("{space:?}")));
                    block(ui, points.max(1.0), 16.0);
                    ui.add(Text::caption(format!("{points}")).muted());
                });
            }
        });
        section(ui, "Size (a control's height)", |ui| {
            Row::new().gap(Space::Md).show(ui, |ui| {
                for size in Size::ALL {
                    ui.add(Button::new(format!("{size:?} {}", size.height(ui))).size(size));
                }
            });
        });
        section(ui, "Radius", |ui| {
            Row::new().gap(Space::Md).show(ui, |ui| {
                for radius in Radius::ALL {
                    Column::new().gap(Space::Xs).show(ui, |ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::Vec2::splat(40.0), egui::Sense::hover());
                        let fill =
                            theme::mix(theme.palette.panel_background, theme.palette.accent, 0.45);
                        ui.painter()
                            .rect_filled(rect, radius.px(ui).min(20.0), theme::color(fill));
                        ui.add(Text::caption(format!("{radius:?}")).muted());
                    });
                }
            });
        });
        section(ui, "Tone", |ui| {
            Row::new().gap(Space::Md).show(ui, |ui| {
                for tone in Tone::ALL {
                    Column::new().gap(Space::Xs).show(ui, |ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(64.0, 32.0), egui::Sense::hover());
                        ui.painter().rect_filled(
                            rect,
                            Radius::Md.px(ui),
                            theme::color(tone.rgba(&theme.palette)),
                        );
                        ui.add(Text::caption(format!("{tone:?}")).muted());
                    });
                }
            });
        });
    });
}
