//! The HTTP client in the status bar (`add_panel_status::<HttpClientPanel>`):
//! the selected request's method and host, and how its last exchange went.
use crate::{
    Exchange, Exchanges, HttpClientPanel, HttpRequests, Method, Received,
    panel::{duration, size},
};
use bedtime_ui::{UiContext, egui, theme};
use bedtime_workbench::status::StatusPieces;
use bevy::prelude::*;
use std::time::Duration;

/// Errors longer than this (in characters) are cut, with the whole on hover.
const ERROR_SHOWN: usize = 60;

/// How a piece of the item is tinted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tint {
    Weak,
    Normal,
    Warning,
    Error,
}

/// The last exchange, as shown.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Sending(String),
    /// The status ("200 OK") with its tint, then time and size.
    Response(String, Tint, Vec<String>),
    /// The error as shown, and in full when it was cut.
    Failed(String, Option<String>),
}

/// The status bar item for an HTTP client panel.
pub fn item(ui: &mut egui::Ui, world: &World, context: &UiContext<'_>, panel: Entity) {
    let Some(id) = world.get::<HttpClientPanel>(panel).and_then(|p| p.selected) else {
        return;
    };
    let Some(request) = world
        .get_resource::<HttpRequests>()
        .and_then(|requests| requests.0.get(id))
    else {
        return;
    };
    let palette = &context.theme.palette;
    let mut row = StatusPieces::new(ui, context);
    // Right to left: the exchange (its last piece first), then the request.
    let exchange = world.get_resource::<Exchanges>().and_then(|e| e.get(id));
    match exchange.map(outcome) {
        Some(Outcome::Sending(text)) => {
            row.weak(text);
            row.ui()
                .ctx()
                .request_repaint_after(Duration::from_millis(500));
        }
        Some(Outcome::Response(status, tint, details)) => {
            for detail in details.into_iter().rev() {
                row.weak(detail);
            }
            match tint {
                Tint::Weak => row.weak(status),
                Tint::Normal => row.label(status),
                Tint::Warning => row.colored(theme::color(palette.warning), status),
                Tint::Error => row.colored(theme::color(palette.error), status),
            };
        }
        Some(Outcome::Failed(shown, full)) => {
            let label = row.colored(theme::color(palette.error), shown);
            if let Some(full) = full {
                label.on_hover_text(full);
            }
        }
        None => {}
    }
    let target = target(request.method, &request.url);
    if !target.is_empty() {
        row.weak(target);
    }
}

fn outcome(exchange: &Exchange) -> Outcome {
    match exchange {
        Exchange::Sending { started, .. } => sending(started.elapsed()),
        Exchange::Done(Ok(received)) => response(received),
        Exchange::Done(Err(error)) => failed(&error.to_string()),
    }
}

fn sending(elapsed: Duration) -> Outcome {
    match elapsed.as_secs() {
        0 => Outcome::Sending("sending…".into()),
        secs => Outcome::Sending(format!("sending… {secs}s")),
    }
}

fn response(received: &Received) -> Outcome {
    let response = &received.response;
    let status = match response.reason {
        Some(reason) => format!("{} {reason}", response.status),
        None => response.status.to_string(),
    };
    let tint = match response.status {
        200..300 => Tint::Normal,
        400..500 => Tint::Warning,
        500..600 => Tint::Error,
        _ => Tint::Weak,
    };
    let mut size = size(response.body.len());
    if response.truncated {
        size.push_str(" (truncated)");
    }
    let details = vec![duration(response.elapsed), size];
    Outcome::Response(status, tint, details)
}

fn failed(error: &str) -> Outcome {
    let error = error.trim();
    let line = error.lines().next().unwrap_or_default();
    if line.chars().count() <= ERROR_SHOWN && line.len() == error.len() {
        return Outcome::Failed(error.to_owned(), None);
    }
    let cut: String = line.chars().take(ERROR_SHOWN).collect();
    Outcome::Failed(format!("{}…", cut.trim_end()), Some(error.to_owned()))
}

/// "GET api.example.com": the method and the URL's host, or the URL as
/// written when there's no host to find (a `{{base}}` variable, say).
fn target(method: Method, url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = if host.is_empty() { url } else { host };
    format!("{} {host}", method.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Response, content};

    fn received(
        status: u16,
        reason: Option<&'static str>,
        body: usize,
        truncated: bool,
    ) -> Received {
        Received {
            response: Response {
                status,
                reason,
                url: String::new(),
                headers: Vec::new(),
                body: vec![0; body],
                truncated,
                elapsed: Duration::from_millis(182),
            },
            content: content::decode(None, &[]),
            text: None,
            pretty: None,
            tree: None,
            image: None,
            serial: 0,
        }
    }

    #[test]
    fn the_target_is_the_method_and_host() {
        assert_eq!(
            target(Method::Get, "https://api.example.com/users?page=2"),
            "GET api.example.com"
        );
        assert_eq!(
            target(Method::Post, "http://user:pw@localhost:8080"),
            "POST localhost:8080"
        );
        assert_eq!(target(Method::Get, "{{base}}/users"), "GET {{base}}");
        assert_eq!(target(Method::Get, "  "), "");
    }

    #[test]
    fn a_response_shows_its_status_time_and_size() {
        assert_eq!(
            response(&received(200, Some("OK"), 4300, false)),
            Outcome::Response(
                "200 OK".into(),
                Tint::Normal,
                vec!["182 ms".into(), "4.2 KB".into()]
            )
        );
        assert_eq!(
            response(&received(404, Some("Not Found"), 0, true)),
            Outcome::Response(
                "404 Not Found".into(),
                Tint::Warning,
                vec!["182 ms".into(), "0 B (truncated)".into()]
            )
        );
        let Outcome::Response(status, tint, _) = response(&received(599, None, 0, false)) else {
            panic!("a response");
        };
        assert_eq!((status.as_str(), tint), ("599", Tint::Error));
        let Outcome::Response(_, tint, _) = response(&received(301, None, 0, false)) else {
            panic!("a response");
        };
        assert_eq!(tint, Tint::Weak);
    }

    #[test]
    fn sending_counts_seconds() {
        assert_eq!(
            sending(Duration::from_millis(300)),
            Outcome::Sending("sending…".into())
        );
        assert_eq!(
            sending(Duration::from_millis(2400)),
            Outcome::Sending("sending… 2s".into())
        );
    }

    #[test]
    fn long_errors_are_cut_with_the_whole_on_hover() {
        assert_eq!(
            failed("Timed out"),
            Outcome::Failed("Timed out".into(), None)
        );
        let long = "x".repeat(100);
        let Outcome::Failed(shown, Some(full)) = failed(&long) else {
            panic!("cut");
        };
        assert_eq!(shown.chars().count(), ERROR_SHOWN + 1);
        assert_eq!(full, long);
        let Outcome::Failed(shown, Some(_)) = failed("refused\nmore detail") else {
            panic!("cut at the line");
        };
        assert_eq!(shown, "refused…");
    }
}
