//! Original event scripts driven through native ImGui input and a real bash PTY.
//! Assertions read the unchanged upstream DrawOp baselines as cell-text oracles.
#[cfg(unix)]
mod unix {
    use bed_terminal::{
        terminal::{Terminal, TerminalEvent},
        terminal_font::TerminalFonts,
        terminal_pty::{PtyEvent, PtyOptions, TerminalPty, TerminalShell, WindowSize},
        terminal_view::{TerminalIo, TerminalView},
    };
    use dear_imgui_rs::{
        ClipboardBackend, Condition, Context, FontSource, FramePrepareOptions, Key,
    };
    use serde_json::Value;
    use std::{
        cell::RefCell,
        collections::HashMap,
        fs, io,
        path::{Path, PathBuf},
        rc::Rc,
        time::{Duration, Instant},
    };
    struct Pipe(TerminalPty);
    impl TerminalIo for Pipe {
        fn pump(&mut self, term: &mut Terminal) -> io::Result<bool> {
            let mut changed = false;
            for event in self.0.poll() {
                match event {
                    PtyEvent::Output(bytes) => {
                        changed = true;
                        for reply in term.feed(&bytes) {
                            if let TerminalEvent::Write(bytes) = reply {
                                self.0.write(&bytes)?;
                            }
                        }
                    }
                    PtyEvent::Exited(_) => {
                        return Err(io::Error::other("test shell exited unexpectedly"));
                    }
                    PtyEvent::Error(error) => return Err(io::Error::other(error)),
                }
            }
            Ok(changed)
        }
        fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.0.write(bytes)
        }
        fn resize(&mut self, cols: usize, rows: usize, cw: f32, ch: f32) -> io::Result<()> {
            self.0.resize(WindowSize {
                num_cols: cols as u16,
                num_lines: rows as u16,
                cell_width: cw as u16,
                cell_height: ch as u16,
            })
        }
    }
    struct Clipboard(Rc<RefCell<String>>);
    impl ClipboardBackend for Clipboard {
        fn get(&mut self) -> Option<String> {
            Some(self.0.borrow().clone())
        }
        fn set(&mut self, s: &str) {
            *self.0.borrow_mut() = s.into();
        }
    }
    #[derive(Clone)]
    enum Event {
        Character(char),
        Key(Key, bool),
        Clipboard(String),
    }
    fn modifier(name: &str) -> (Key, Key) {
        match name {
            "Ctrl" => {
                if cfg!(target_os = "macos") {
                    (Key::LeftSuper, Key::ModSuper)
                } else {
                    (Key::LeftCtrl, Key::ModCtrl)
                }
            }
            "Shift" => (Key::LeftShift, Key::ModShift),
            _ => panic!("untranslated modifier {name}"),
        }
    }
    fn key(name: &str) -> Key {
        match name {
            "Left" => Key::LeftArrow,
            "Backspace" => Key::Backspace,
            "Enter" => Key::Enter,
            "V" => Key::V,
            "C" => Key::C,
            _ => panic!("untranslated fixture key {name}"),
        }
    }
    fn schedule(text: &str) -> Vec<Vec<Event>> {
        let mut frames: Vec<Vec<Event>> = Vec::new();
        let mut at = 0;
        let mut put = |at: usize, event: Event| {
            if frames.len() <= at {
                frames.resize_with(at + 1, Vec::new);
            }
            frames[at].push(event);
        };
        for line in text.lines() {
            let line = line.trim_start();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (command, arg) = line.split_once(' ').unwrap();
            match command {
                "wait" => at += arg.parse::<usize>().unwrap(),
                "type" => {
                    for character in arg.chars() {
                        put(at, Event::Character(character));
                        at += 1;
                    }
                }
                "key" => {
                    let key = key(arg);
                    put(at, Event::Key(key, true));
                    put(at + 1, Event::Key(key, false));
                    at += 2;
                }
                "clipboard" => {
                    put(at, Event::Clipboard(arg.into()));
                    at += 1;
                }
                "chord" => {
                    let (mods, arg) = arg.split_once(' ').unwrap();
                    for name in mods.split('+') {
                        let (physical, flag) = modifier(name);
                        for key in [physical, flag] {
                            put(at, Event::Key(key, true));
                            put(at + 1, Event::Key(key, false));
                        }
                    }
                    let key = key(arg);
                    put(at, Event::Key(key, true));
                    put(at + 1, Event::Key(key, false));
                    at += 2;
                }
                _ => panic!("untranslated event {command}"),
            }
        }
        if frames.len() < at + 30 {
            frames.resize_with(at + 30, Vec::new);
        }
        frames
    }
    fn rows(term: &Terminal) -> Vec<String> {
        (0..term.rows())
            .map(|row| {
                let mut text = String::new();
                for col in 0..term.cols() {
                    let cell = term.cell(row, col);
                    if cell.mode & bed_terminal::terminal::ATTR_WDUMMY == 0 {
                        text.push(cell.character);
                    }
                }
                text.trim_end().into()
            })
            .collect()
    }
    fn frame(
        context: &mut Context,
        term: &mut Terminal,
        view: &mut TerminalView,
        pipe: &mut Pipe,
        fonts: &TerminalFonts,
    ) {
        context.prepare_frame(FramePrepareOptions::new([800.0, 600.0], 1.0 / 60.0));
        let ui = context.frame();
        ui.window("Terminal")
            .size([800.0, 500.0], Condition::Always)
            .position([0.0, 0.0], Condition::Always)
            .build(|| {
                ui.set_keyboard_focus_here();
                view.draw(ui, term, fonts, pipe).unwrap();
            });
        drop(context.render_legacy());
    }
    pub fn run() {
        let base = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .join("tests/fixtures/terminal_input");
        let bash = ["/opt/homebrew/bin/bash", "/usr/local/bin/bash", "/bin/bash"]
            .into_iter()
            .find(|path| Path::new(path).is_file())
            .expect("bash is required by the original fixtures");
        for name in [
            "type_hello",
            "backspace_edit",
            "arrow_line_edit",
            "utf8_typing",
            "clipboard_paste",
            "ctrl_c_interrupt",
        ] {
            let folder = base.join(name);
            let expected: Value =
                serde_json::from_slice(&fs::read(folder.join("expected.json")).unwrap()).unwrap();
            let home = std::env::temp_dir()
                .join(format!("bed-terminal-input-{}-{name}", std::process::id()));
            fs::create_dir_all(&home).unwrap();
            let mut env = HashMap::new();
            for (k, v) in [
                ("PATH", "/usr/bin:/bin"),
                ("PS1", "$ "),
                ("PS2", "> "),
                ("TERM", "xterm-256color"),
                ("LANG", "C.UTF-8"),
                ("LC_ALL", "C.UTF-8"),
                ("TZ", "UTC"),
                ("INPUTRC", "/dev/null"),
                ("HISTFILE", "/dev/null"),
                ("BASH_SILENCE_DEPRECATION_WARNING", "1"),
                ("PROMPT_COMMAND", ""),
                ("BASH_ENV", ""),
                ("ENV", ""),
            ] {
                env.insert(k.into(), v.into());
            }
            env.insert("HOME".into(), home.to_string_lossy().into_owned());
            let options = PtyOptions {
                working_directory: Some(home.clone()),
                shell: Some(TerminalShell::new(
                    bash,
                    vec!["--noprofile".into(), "--norc".into(), "-i".into()],
                )),
                env,
            };
            let mut pipe = Pipe(
                TerminalPty::spawn(
                    &options,
                    WindowSize {
                        num_cols: 80,
                        num_lines: 24,
                        cell_width: 8,
                        cell_height: 16,
                    },
                )
                .unwrap(),
            );
            let mut context = Context::create();
            context.set_ini_filename(None::<PathBuf>).unwrap();
            let clipboard = Rc::new(RefCell::new(String::new()));
            context.set_clipboard_backend(Clipboard(clipboard.clone()));
            let font = context
                .font_atlas()
                .add_font(&[FontSource::default_font_with_size(16.0)]);
            context
                .font_atlas()
                .try_claim_legacy_renderer()
                .unwrap()
                .build();
            let fonts = TerminalFonts {
                regular: Some(font),
                bold: Some(font),
                italic: Some(font),
                bold_italic: Some(font),
                size: 16.0,
                ..TerminalFonts::default()
            };
            let mut term = Terminal::new(80, 24);
            let mut view = TerminalView::default();
            // Startup synchronization avoids relying on the host's CPU speed for a prompt.
            let deadline = Instant::now() + Duration::from_secs(5);
            while !rows(&term).iter().any(|row| row.starts_with("$")) {
                assert!(
                    Instant::now() < deadline,
                    "{name} startup timed out {:?}",
                    rows(&term)
                );
                frame(&mut context, &mut term, &mut view, &mut pipe, &fonts);
                std::thread::sleep(Duration::from_millis(2));
            }
            for events in schedule(&fs::read_to_string(folder.join("events.txt")).unwrap()) {
                for event in events {
                    match event {
                        Event::Character(c) => {
                            context.io_mut().add_input_characters_utf8(c.to_string())
                        }
                        Event::Key(k, down) => context.io_mut().add_key_event(k, down),
                        Event::Clipboard(s) => *clipboard.borrow_mut() = s,
                    }
                }
                frame(&mut context, &mut term, &mut view, &mut pipe, &fonts);
                std::thread::sleep(Duration::from_millis(2));
            }
            let expected_rows: Vec<String> = expected["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| {
                    row["ops"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|op| op["text"].as_str())
                        .collect::<String>()
                        .trim_end()
                        .into()
                })
                .take_while(|s: &String| !s.is_empty())
                .collect();
            let deadline = Instant::now() + Duration::from_secs(2);
            while rows(&term)[..expected_rows.len()] != expected_rows {
                assert!(
                    Instant::now() < deadline,
                    "{name} expected {:?}, actual {:?}",
                    expected_rows,
                    rows(&term)
                );
                frame(&mut context, &mut term, &mut view, &mut pipe, &fonts);
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(view.focused, "{name} canvas focus");
            pipe.0.shutdown();
            drop(context);
            fs::remove_dir_all(home).unwrap();
            println!("PASS original input fixture {name}");
        }
    }
}
fn main() {
    #[cfg(unix)]
    unix::run();
}
