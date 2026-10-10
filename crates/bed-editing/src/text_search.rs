//! Pure, byte-oriented project matching and project-relative path filters.
use glob::{MatchOptions, Pattern};
use regex::bytes::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_words: bool,
    pub regex: bool,
    pub include_ignored: bool,
    pub include: String,
    pub exclude: String,
}

pub struct CompiledSearch {
    matcher: Regex,
    whole_words: bool,
    regex: bool,
    include: Vec<Pattern>,
    exclude: Vec<Pattern>,
}
impl CompiledSearch {
    pub fn new(query: &[u8], options: &SearchOptions) -> Result<Self, String> {
        let pattern = if options.regex {
            std::str::from_utf8(query)
                .map_err(|error| error.to_string())?
                .to_owned()
        } else {
            query.iter().map(|byte| format!("\\x{byte:02x}")).collect()
        };
        let matcher = RegexBuilder::new(&pattern)
            .unicode(false)
            .case_insensitive(!options.case_sensitive)
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            matcher,
            whole_words: options.whole_words,
            regex: options.regex,
            include: patterns(&options.include)?,
            exclude: patterns(&options.exclude)?,
        })
    }
    pub fn includes_path(&self, relative: &str) -> bool {
        let matches = |pattern: &Pattern| {
            let target = if pattern.as_str().contains('/') {
                relative
            } else {
                relative.rsplit('/').next().unwrap_or(relative)
            };
            pattern.matches_with(
                target,
                MatchOptions {
                    case_sensitive: true,
                    require_literal_separator: true,
                    require_literal_leading_dot: false,
                },
            )
        };
        (self.include.is_empty() || self.include.iter().any(matches))
            && !self.exclude.iter().any(matches)
    }
    pub fn matches<'a>(&'a self, line: &'a [u8]) -> impl Iterator<Item = Range<usize>> + 'a {
        self.matcher
            .find_iter(line)
            .filter(move |found| {
                !self.whole_words
                    || (!found
                        .start()
                        .checked_sub(1)
                        .and_then(|i| line.get(i))
                        .is_some_and(word_byte)
                        && !line.get(found.end()).is_some_and(word_byte))
            })
            .map(|found| found.range())
    }
    /// Expand against the original whole line, retaining anchor/capture context.
    pub fn replacement(
        &self,
        line: &[u8],
        range: Range<usize>,
        replacement: &str,
    ) -> Result<Vec<u8>, String> {
        let captures = self
            .matcher
            .captures_at(line, range.start)
            .filter(|captures| captures.get(0).is_some_and(|found| found.range() == range))
            .ok_or_else(|| "Match changed; refresh before replacing".to_owned())?;
        if !self.regex {
            return Ok(replacement.as_bytes().to_vec());
        }
        let mut bytes = Vec::new();
        captures.expand(replacement.as_bytes(), &mut bytes);
        Ok(bytes)
    }
}
fn word_byte(byte: &u8) -> bool {
    byte.is_ascii_alphanumeric() || *byte == b'_'
}
fn patterns(input: &str) -> Result<Vec<Pattern>, String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let pattern = part.strip_prefix("./").unwrap_or(part);
            let pattern = if pattern.ends_with('/') {
                format!("{pattern}**")
            } else {
                pattern.to_owned()
            };
            Pattern::new(&pattern).map_err(|error| format!("Invalid path filter {part:?}: {error}"))
        })
        .collect()
}

/// Shared permissive text probe used by project search and explicit text reads.
pub fn binary_text_probe(bytes: &[u8]) -> bool {
    let prefix = &bytes[..bytes.len().min(1024)];
    let junk = prefix
        .iter()
        .filter(|&&c| c == 0 || (c < 32 && c != b'\n' && c != b'\r' && c != b'\t'))
        .count();
    !prefix.is_empty() && junk > prefix.len() / 10
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literals_case_words_and_nonoverlapping_ranges() {
        let options = SearchOptions::default();
        let search = CompiledSearch::new(b"aba", &options).unwrap();
        assert_eq!(
            search.matches(b"ABABA aba").collect::<Vec<_>>(),
            vec![0..3, 6..9]
        );
        let search = CompiledSearch::new(
            b"cat",
            &SearchOptions {
                whole_words: true,
                ..options.clone()
            },
        )
        .unwrap();
        assert_eq!(
            search
                .matches(b"cat scatter cat_ CAT cat.")
                .collect::<Vec<_>>(),
            vec![0..3, 17..20, 21..24]
        );
        let search = CompiledSearch::new(b".*[", &options).unwrap();
        assert_eq!(search.matches(b"x.*[").collect::<Vec<_>>(), vec![1..4]);
        assert_eq!(search.replacement(b"x.*[", 1..4, "$1").unwrap(), b"$1");
        let search = CompiledSearch::new(
            b"cat",
            &SearchOptions {
                case_sensitive: true,
                ..options
            },
        )
        .unwrap();
        assert_eq!(search.matches(b"CAT cat").collect::<Vec<_>>(), vec![4..7]);
    }
    #[test]
    fn captures_empty_matches_and_invalid_patterns() {
        let options = SearchOptions {
            regex: true,
            ..Default::default()
        };
        let search = CompiledSearch::new(b"(?P<name>[a-z]+)=(\\d+)", &options).unwrap();
        assert_eq!(
            search
                .replacement(b"abc=42", 0..6, "${name}:$2:$$")
                .unwrap(),
            b"abc:42:$"
        );
        assert!(search.replacement(b"abc=43x", 0..7, "").is_err());
        let search = CompiledSearch::new(b"^|$", &options).unwrap();
        assert_eq!(search.matches(b"abc").collect::<Vec<_>>(), vec![0..0, 3..3]);
        assert!(CompiledSearch::new(b"[", &options).is_err());
        assert!(
            CompiledSearch::new(
                b"x",
                &SearchOptions {
                    include: "[".into(),
                    ..options
                }
            )
            .is_err()
        );
    }
    #[test]
    fn includes_are_alternatives_and_exclusions_win() {
        let search = CompiledSearch::new(
            b"x",
            &SearchOptions {
                include: "src/**/*.rs, *.toml".into(),
                exclude: "src/generated/, ignored.rs".into(),
                ..Default::default()
            },
        )
        .unwrap();
        for path in [
            "src/main.rs",
            "src/deep/a.rs",
            "Cargo.toml",
            "nested/Cargo.toml",
        ] {
            assert!(search.includes_path(path), "{path}");
        }
        for path in [
            "other/a.rs",
            "src/generated/a.rs",
            "src/ignored.rs",
            "src/a.txt",
        ] {
            assert!(!search.includes_path(path), "{path}");
        }
    }
}
