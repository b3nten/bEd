//! What a body is and how to read it: text (in which language, from which
//! charset), an image, or other bytes. Decided by `Content-Type`, then by the
//! body itself when that's missing or says nothing (`application/octet-stream`).
use encoding_rs::{Encoding, UTF_8};

/// A body, made ready to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    /// The media type, lowercase, without parameters (`text/html`).
    pub media_type: Option<String>,
    pub kind: Kind,
    /// Text: the body decoded.
    pub text: Option<String>,
    /// The charset the text was decoded from, if it wasn't UTF-8.
    pub charset: Option<&'static str>,
    /// Some bytes weren't valid in the charset and were replaced (�).
    pub replaced: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Text, in a language its code can be coloured as (`json`, `html`).
    Text {
        language: Option<&'static str>,
    },
    Image,
    Binary,
}

/// How much of a body is looked at to guess what it is, or find a charset
/// declared inside it.
const SNIFF: usize = 8 * 1024;

/// Read `body`, served as `content_type`.
pub fn decode(content_type: Option<&str>, body: &[u8]) -> Content {
    let (media_type, charset) = content_type.map(parse).unwrap_or_default();
    let media_type = media_type.filter(|m| !m.is_empty());
    let kind = match media_type.as_deref() {
        // Before text: SVG is XML, but shown as a picture.
        Some(media) if media.starts_with("image/") => Kind::Image,
        Some(media) if textual(media) => Kind::Text {
            language: language(media),
        },
        Some(media) if !unknown(media) => Kind::Binary,
        _ => sniff(body),
    };
    let mut content = Content {
        media_type,
        kind,
        text: None,
        charset: None,
        replaced: false,
    };
    if let Kind::Text { language } = kind {
        let encoding = Encoding::for_bom(body)
            .map(|(encoding, _)| encoding)
            .or_else(|| charset.as_deref().and_then(label))
            .or_else(|| declared(language, body))
            .unwrap_or(UTF_8);
        // Strips a BOM, and a UTF-16 one decodes as UTF-16 whatever was said.
        let (text, encoding, replaced) = encoding.decode(body);
        content.text = Some(text.into_owned());
        content.charset = (encoding != UTF_8).then(|| encoding.name());
        content.replaced = replaced;
    }
    content
}

/// `text/html; charset=ISO-8859-1` as `("text/html", Some("iso-8859-1"))`.
fn parse(content_type: &str) -> (Option<String>, Option<String>) {
    let mut parts = content_type.split(';');
    let media = parts.next().map(|m| m.trim().to_ascii_lowercase());
    let charset = parts.find_map(|part| {
        let (name, value) = part.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
    });
    (media, charset)
}

fn label(charset: &str) -> Option<&'static Encoding> {
    Encoding::for_label(charset.as_bytes())
}

/// A media type that says nothing about the body.
fn unknown(media: &str) -> bool {
    matches!(
        media,
        "application/octet-stream" | "binary/octet-stream" | "application/unknown"
    )
}

fn textual(media: &str) -> bool {
    let (kind, subtype) = media.split_once('/').unwrap_or((media, ""));
    kind == "text"
        || ["+json", "+xml", "+yaml"]
            .iter()
            .any(|suffix| subtype.ends_with(suffix))
        || matches!(
            subtype,
            "json"
                | "xml"
                | "javascript"
                | "ecmascript"
                | "x-javascript"
                | "x-www-form-urlencoded"
                | "graphql"
                | "sql"
                | "toml"
                | "yaml"
                | "x-yaml"
                | "x-sh"
                | "x-ndjson"
                | "jsonl"
                | "csv"
        ) && kind == "application"
}

/// The language to colour a textual media type as, by bedtime-syntax's names.
fn language(media: &str) -> Option<&'static str> {
    let subtype = media.split_once('/').map_or(media, |(_, s)| s);
    let subtype = subtype.rsplit('+').next().unwrap_or(subtype);
    Some(match subtype {
        "json" => "json",
        "html" | "xhtml" => "html",
        "xml" => "xml",
        "javascript" | "ecmascript" | "x-javascript" => "javascript",
        "css" => "css",
        "markdown" | "x-markdown" => "markdown",
        "yaml" | "x-yaml" => "yaml",
        "toml" => "toml",
        "sql" => "sql",
        "x-sh" | "x-shellscript" => "bash",
        _ => return None,
    })
}

/// A body without a useful type: text if it has a BOM, or is UTF-8 without
/// NULs (in its first `SNIFF` bytes); an image if it starts like one.
fn sniff(body: &[u8]) -> Kind {
    let head = &body[..body.len().min(SNIFF)];
    const IMAGES: &[&[u8]] = &[b"\x89PNG\r\n\x1a\n", b"\xff\xd8\xff", b"GIF87a", b"GIF89a"];
    let webp = head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP");
    if webp || IMAGES.iter().any(|magic| head.starts_with(magic)) {
        return Kind::Image;
    }
    if Encoding::for_bom(head).is_some() {
        return Kind::Text { language: None };
    }
    if head.contains(&0) {
        return Kind::Binary;
    }
    // The head may end mid-character.
    let valid = match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && body.len() > head.len(),
    };
    if !valid {
        return Kind::Binary;
    }
    let text = String::from_utf8_lossy(head);
    let start = text.trim_start();
    let language = if start.starts_with('{') || start.starts_with('[') {
        Some("json")
    } else if start.len() >= 9 && start[..9].eq_ignore_ascii_case("<!doctype")
        || start.starts_with("<html")
    {
        Some("html")
    } else if start.starts_with("<?xml") {
        Some("xml")
    } else {
        None
    };
    Kind::Text { language }
}

/// The charset an HTML or XML body declares near its start
/// (`<meta charset="…">`, `<?xml … encoding="…"?>`).
fn declared(language: Option<&str>, body: &[u8]) -> Option<&'static Encoding> {
    let head = &body[..body.len().min(SNIFF)];
    let head = String::from_utf8_lossy(head).to_ascii_lowercase();
    let after = match language? {
        "html" => head.find("charset=").map(|at| at + "charset=".len()),
        "xml" => head
            .strip_prefix("<?xml")
            .and_then(|rest| rest.split_once("?>"))
            .and_then(|(declaration, _)| declaration.find("encoding="))
            .map(|at| "<?xml".len() + at + "encoding=".len()),
        _ => None,
    }?;
    let value = head[after..].trim_start_matches(['"', '\'']);
    let end = value
        .find(|c: char| !(c.is_ascii_alphanumeric() || "-_:.".contains(c)))
        .unwrap_or(value.len());
    label(&value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(content: &Content) -> &str {
        content.text.as_deref().unwrap()
    }

    #[test]
    fn latin_1_html_is_text_decoded_from_its_charset() {
        // "Français" in ISO-8859-1: not UTF-8.
        let body = b"<html>Fran\xe7ais</html>";
        let content = decode(Some("text/html; charset=ISO-8859-1"), body);
        assert_eq!(
            content.kind,
            Kind::Text {
                language: Some("html")
            }
        );
        assert_eq!(text(&content), "<html>Français</html>");
        // Browsers read ISO-8859-1 as windows-1252, and so does encoding_rs.
        assert_eq!(content.charset, Some("windows-1252"));
        assert!(!content.replaced);
    }

    #[test]
    fn html_without_a_header_charset_uses_its_meta() {
        let body = b"<html><head><meta charset=\"windows-1252\"></head>\x93hi\x94</html>";
        let content = decode(Some("text/html"), body);
        assert!(text(&content).contains("\u{201c}hi\u{201d}"));
        let body = b"<meta http-equiv=Content-Type content='text/html; charset=iso-8859-1'>\xe9";
        assert!(text(&decode(Some("text/html"), body)).ends_with('é'));
    }

    #[test]
    fn xml_uses_its_declaration() {
        let body = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>\xe9</a>";
        let content = decode(Some("application/xml"), body);
        assert_eq!(
            text(&content),
            "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>é</a>"
        );
    }

    #[test]
    fn invalid_utf_8_in_text_is_replaced_not_binary() {
        let content = decode(Some("text/plain"), b"ok \xff ok");
        assert_eq!(content.kind, Kind::Text { language: None });
        assert_eq!(text(&content), "ok \u{fffd} ok");
        assert!(content.replaced);
        assert_eq!(content.charset, None);
    }

    #[test]
    fn a_bom_wins_and_is_dropped() {
        let content = decode(
            Some("text/plain; charset=iso-8859-1"),
            b"\xef\xbb\xbfh\xc3\xa9",
        );
        assert_eq!(text(&content), "hé");
        let content = decode(None, b"\xff\xfeh\0i\0");
        assert_eq!(text(&content), "hi");
        assert_eq!(content.charset, Some("UTF-16LE"));
    }

    #[test]
    fn media_types_choose_the_language() {
        let kind = |t| decode(Some(t), b"").kind;
        assert_eq!(
            kind("application/json"),
            Kind::Text {
                language: Some("json")
            }
        );
        assert_eq!(
            kind("application/problem+json; charset=utf-8"),
            Kind::Text {
                language: Some("json")
            }
        );
        assert_eq!(
            kind("application/atom+xml"),
            Kind::Text {
                language: Some("xml")
            }
        );
        assert_eq!(
            kind("text/javascript"),
            Kind::Text {
                language: Some("javascript")
            }
        );
        assert_eq!(kind("text/csv"), Kind::Text { language: None });
        assert_eq!(kind("image/png"), Kind::Image);
        assert_eq!(kind("image/svg+xml"), Kind::Image);
        assert_eq!(kind("application/pdf"), Kind::Binary);
        assert_eq!(kind("font/woff2"), Kind::Binary);
    }

    #[test]
    fn untyped_bodies_are_sniffed() {
        let kind = |body: &[u8]| decode(None, body).kind;
        assert_eq!(
            kind(b" {\"a\":1}"),
            Kind::Text {
                language: Some("json")
            }
        );
        assert_eq!(
            kind(b"<!DOCTYPE html><p>"),
            Kind::Text {
                language: Some("html")
            }
        );
        assert_eq!(kind(b"plain words"), Kind::Text { language: None });
        assert_eq!(kind(b"\x89PNG\r\n\x1a\n...."), Kind::Image);
        assert_eq!(kind(b"a\0b"), Kind::Binary);
        assert_eq!(kind(b"\xc3\x28 not utf-8"), Kind::Binary);
        assert_eq!(kind(b"BMW, not a bitmap"), Kind::Text { language: None });
        assert_eq!(
            decode(Some("application/octet-stream"), b"hello").kind,
            Kind::Text { language: None }
        );
    }
}
