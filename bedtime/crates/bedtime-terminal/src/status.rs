//! A terminal in the status bar (`add_panel_status::<TerminalPanel>`): what's
//! running, where, the size, and how far back it's scrolled; or how the
//! shell ended.
use crate::{TerminalPanel, TerminalStatus};
use bedtime_ui::{UiContext, egui};
use bedtime_workbench::status::StatusPieces;
use bevy::prelude::*;
use std::path::Path;

/// How a piece of the item is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tint {
    Weak,
    Error,
}

/// The status bar item for a terminal panel.
pub fn item(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>, panel: Entity) {
    let Some(terminal) = world.get::<TerminalPanel>(panel) else {
        return;
    };
    let snapshot = terminal.snapshot();
    let name = terminal
        .foreground_process_name()
        .or_else(|| terminal.shell_name().map(str::to_owned));
    let home = std::env::var_os("HOME");
    let pieces = pieces(
        terminal.status(),
        name.as_deref(),
        snapshot.current_directory.as_deref(),
        home.as_deref().map(Path::new),
        [snapshot.cols, snapshot.rows],
        snapshot.display_offset,
    );
    let palette = &context.theme.palette;
    // Right to left: the last piece first.
    let mut row = StatusPieces::new(ui, context);
    for (text, tint) in pieces.into_iter().rev() {
        match tint {
            Tint::Weak => row.weak(text),
            Tint::Error => row.colored(bedtime_ui::theme::color(palette.error), text),
        };
    }
}

/// The item's pieces, left to right; empty ones left out.
fn pieces(
    status: &TerminalStatus,
    name: Option<&str>,
    directory: Option<&Path>,
    home: Option<&Path>,
    [cols, rows]: [usize; 2],
    scrolled: usize,
) -> Vec<(String, Tint)> {
    let mut pieces = Vec::new();
    match status {
        TerminalStatus::Exited(exit) => match exit.and_then(|exit| exit.code()) {
            Some(0) => pieces.push(("exited (0)".to_owned(), Tint::Weak)),
            Some(code) => pieces.push((format!("exited ({code})"), Tint::Error)),
            None => pieces.push(("exited".to_owned(), Tint::Error)),
        },
        TerminalStatus::Failed(_) => pieces.push(("failed".to_owned(), Tint::Error)),
        _ => {
            if let Some(name) = name.filter(|name| !name.is_empty()) {
                pieces.push((name.to_owned(), Tint::Weak));
            }
        }
    }
    if let Some(directory) = directory {
        pieces.push((abbreviated(directory, home), Tint::Weak));
    }
    if cols > 0 && rows > 0 {
        pieces.push((format!("{cols}×{rows}"), Tint::Weak));
    }
    if scrolled > 0 {
        let lines = if scrolled == 1 { "line" } else { "lines" };
        pieces.push((format!("↑ {scrolled} {lines}"), Tint::Weak));
    }
    pieces
}

/// `directory` with the home folder as `~`.
fn abbreviated(directory: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| directory.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => directory.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(pieces: Vec<(String, Tint)>) -> Vec<String> {
        pieces.into_iter().map(|(text, _)| text).collect()
    }

    #[test]
    fn a_running_shell_shows_its_name_folder_and_size() {
        let home = Path::new("/Users/me");
        let pieces = pieces(
            &TerminalStatus::Running,
            Some("zsh"),
            Some(Path::new("/Users/me/Dev/bed")),
            Some(home),
            [120, 40],
            0,
        );
        assert_eq!(texts(pieces), ["zsh", "~/Dev/bed", "120×40"]);
    }

    #[test]
    fn empty_pieces_are_left_out_and_scrolling_is_shown() {
        let shown = pieces(&TerminalStatus::Starting, None, None, None, [0, 0], 1);
        assert_eq!(texts(shown), ["↑ 1 line"]);
        let shown = pieces(
            &TerminalStatus::Running,
            Some(""),
            None,
            None,
            [80, 24],
            120,
        );
        assert_eq!(texts(shown), ["80×24", "↑ 120 lines"]);
    }

    #[test]
    fn folders_outside_home_stay_whole() {
        let home = Some(Path::new("/Users/me"));
        assert_eq!(abbreviated(Path::new("/Users/me"), home), "~");
        assert_eq!(abbreviated(Path::new("/Users/meet"), home), "/Users/meet");
        assert_eq!(abbreviated(Path::new("/tmp"), None), "/tmp");
    }

    #[cfg(unix)]
    #[test]
    fn an_exit_replaces_the_name_tinted_by_its_code() {
        use std::os::unix::process::ExitStatusExt;
        let exited = |raw| TerminalStatus::Exited(Some(std::process::ExitStatus::from_raw(raw)));
        let ok = pieces(&exited(0), Some("zsh"), None, None, [80, 24], 0);
        assert_eq!(ok[0], ("exited (0)".to_owned(), Tint::Weak));
        assert_eq!(ok.len(), 2, "no name, still the size");
        // A raw wait status: exit code 1 is 1 << 8.
        let failed = pieces(&exited(1 << 8), Some("zsh"), None, None, [80, 24], 0);
        assert_eq!(failed[0], ("exited (1)".to_owned(), Tint::Error));
        let unknown = pieces(&TerminalStatus::Exited(None), None, None, None, [0, 0], 0);
        assert_eq!(unknown, [("exited".to_owned(), Tint::Error)]);
    }
}
