//! A database panel in the status bar (`add_panel_status::<SqlitePanel>`):
//! the selected table or view and which of its rows show, or what Query's
//! last run came to; and whether the database is read-only.
use crate::{Kind, Mode, Outcome, Page, SqlitePanel, db::grouped};
use bedtime_ui::{UiContext, egui, theme};
use bedtime_workbench::status::StatusPieces;
use bevy::prelude::*;
use std::time::Duration;

/// The status bar item for a database panel.
pub fn item(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>, panel: Entity) {
    let Some(sqlite) = world.get::<SqlitePanel>(panel) else {
        return;
    };
    let palette = &context.theme.palette;
    let mut row = StatusPieces::new(ui, context);
    // Right to left: read-only, then what shows (its last piece first).
    if sqlite.read_only() {
        row.colored(theme::color(palette.warning), "read-only");
    }
    let shown = match sqlite.mode {
        Mode::Query if sqlite.running() => {
            row.ui()
                .ctx()
                .request_repaint_after(Duration::from_millis(250));
            vec!["running…".to_owned()]
        }
        Mode::Query => match sqlite.outcome() {
            Some(Ok(outcome)) => outcome_pieces(outcome),
            Some(Err(_)) => {
                row.colored(theme::color(palette.error), "query failed");
                Vec::new()
            }
            None => Vec::new(),
        },
        Mode::Data | Mode::Structure => {
            let selected = sqlite.selected.as_deref();
            let object = selected
                .and_then(|name| sqlite.schema()?.object(name))
                .map(|object| (object.name.as_str(), object.kind));
            // The rows showing, if they're the selected object's.
            let rows = (sqlite.mode == Mode::Data)
                .then(|| sqlite.page().zip(sqlite.page_for()))
                .flatten()
                .filter(|(_, browse)| Some(browse.table.as_str()) == selected)
                .map(|(page, browse)| (page, !browse.filter.trim().is_empty()));
            object
                .map(|object| browse_pieces(object, rows))
                .unwrap_or_default()
        }
    };
    for piece in shown.into_iter().rev() {
        row.weak(piece);
    }
}

/// "users (table)", "rows 1–1,000 of 48,210", with "filtered" when a filter
/// applies.
fn browse_pieces((name, kind): (&str, Kind), rows: Option<(&Page, bool)>) -> Vec<String> {
    let kind = match kind {
        Kind::Table => "table",
        Kind::View => "view",
    };
    let mut pieces = vec![format!("{name} ({kind})")];
    if let Some((page, filtered)) = rows {
        let shown = page.rows.len() as u64;
        pieces.push(if shown == 0 {
            if filtered { "no rows match" } else { "no rows" }.to_owned()
        } else {
            let first = page.first_row() + 1;
            let mut rows = format!(
                "rows {}–{} of {}",
                grouped(first),
                grouped(first + shown - 1),
                grouped(page.total)
            );
            if filtered {
                rows.push_str(" (filtered)");
            }
            rows
        });
    }
    pieces
}

/// "312 rows", "4 ms"; "2 statements", "3 rows changed", "1.2 s".
fn outcome_pieces(outcome: &Outcome) -> Vec<String> {
    // The summary joins its parts with " · ": they're pieces here.
    let mut pieces: Vec<String> = outcome.summary().split(" · ").map(str::to_owned).collect();
    if outcome.statements > 0 {
        pieces.push(duration(outcome.seconds));
    }
    pieces
}

fn duration(seconds: f64) -> String {
    if seconds < 1.0 {
        format!("{} ms", (seconds * 1000.0).round() as u64)
    } else {
        format!("{seconds:.1} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PAGE_ROWS, Value};

    fn page(rows: usize, total: u64, at: u64) -> Page {
        Page {
            rows: vec![vec![Value::Null]; rows],
            total,
            page: at,
            ..Default::default()
        }
    }

    #[test]
    fn browsing_says_which_rows_show() {
        let users = ("users", Kind::Table);
        assert_eq!(browse_pieces(users, None), ["users (table)"]);
        assert_eq!(
            browse_pieces(users, Some((&page(1000, 48_210, 0), false))),
            ["users (table)", "rows 1–1,000 of 48,210"]
        );
        assert_eq!(
            browse_pieces(users, Some((&page(210, 48_210, 48), false)))[1],
            format!("rows {}–48,210 of 48,210", grouped(48 * PAGE_ROWS + 1))
        );
        assert_eq!(
            browse_pieces(("recent", Kind::View), Some((&page(3, 3, 0), true))),
            ["recent (view)", "rows 1–3 of 3 (filtered)"]
        );
        assert_eq!(
            browse_pieces(users, Some((&page(0, 0, 0), false))),
            ["users (table)", "no rows"]
        );
        assert_eq!(
            browse_pieces(users, Some((&page(0, 0, 0), true))),
            ["users (table)", "no rows match"]
        );
    }

    #[test]
    fn a_query_says_what_it_came_to_and_how_long_it_took() {
        let read = Outcome {
            columns: vec!["id".into()],
            rows: vec![vec![Value::Integer(1)]; 312],
            statements: 1,
            seconds: 0.0041,
            ..Default::default()
        };
        assert_eq!(outcome_pieces(&read), ["312 rows", "4 ms"]);
        let wrote = Outcome {
            statements: 2,
            changed: 3,
            wrote: true,
            seconds: 1.25,
            ..Default::default()
        };
        assert_eq!(
            outcome_pieces(&wrote),
            ["2 statements", "3 rows changed", "1.2 s"]
        );
        assert_eq!(outcome_pieces(&Outcome::default()), ["Nothing to run"]);
    }
}
