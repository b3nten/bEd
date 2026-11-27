//! The agent's own file reads and writes (ACP `fs/read_text_file` and
//! `fs/write_text_file`), through the documents that are open: it reads a
//! file as its editor shows it, unsaved changes and all, and what it writes
//! to an open file is an edit there, one that can be undone, saved at once.
//! A file that isn't open is read and written on disk.
use bedtime_workbench::{BedtimeWorldExt as _, BufferEdit, EditOptions, TextBuffer};
use bevy::prelude::*;
use std::{ops::Range, path::Path};

/// The text of the file at `path`: from `line` (counted from 1) if given,
/// `limit` lines of it if given.
pub(crate) fn read(
    world: &World,
    path: &Path,
    line: Option<u32>,
    limit: Option<u32>,
) -> Result<String, String> {
    let bedtime = world.bedtime();
    let open = bedtime
        .document_for_file::<TextBuffer>(path)
        .and_then(|document| bedtime.read_document(document).ok())
        .map(ToString::to_string);
    let text = match open {
        Some(text) => text,
        None => std::fs::read_to_string(path)
            .map_err(|error| format!("Couldn't read {}: {error}", path.display()))?,
    };
    Ok(lines(&text, line, limit))
}

/// Make the file at `path` hold `content`.
pub(crate) fn write(world: &mut World, path: &Path, content: &str) -> Result<(), String> {
    let Some(document) = world.bedtime().document_for_file::<TextBuffer>(path) else {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)
                .map_err(|error| format!("Couldn't make {}: {error}", folder.display()))?;
        }
        return std::fs::write(path, content)
            .map_err(|error| format!("Couldn't write {}: {error}", path.display()));
    };
    let failed = |error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
    let bedtime = world.bedtime();
    let old = bedtime
        .read_document(document)
        .map_err(|error| failed(&error))?
        .to_string();
    let revision = bedtime
        .inspect_document(document.id())
        .map_err(|error| failed(&error))?
        .revision;
    if let Some((range, with)) = changed(&old, content) {
        world
            .bedtime_mut()
            .edit_document_transaction(
                document,
                revision,
                [BufferEdit::new(range, with)],
                EditOptions::new("Agent Edit"),
            )
            .map_err(|error| failed(&error))?;
    }
    // On disk at once: the agent builds and tests what's there.
    let unsaved = world
        .bedtime()
        .inspect_document(document.id())
        .is_ok_and(|info| info.dirty && !info.saving);
    if unsaved {
        world
            .bedtime_mut()
            .save_document(document.id())
            .map_err(|error| failed(&error))?;
    }
    Ok(())
}

/// `text` from line `line` (from 1), at most `limit` lines.
fn lines(text: &str, line: Option<u32>, limit: Option<u32>) -> String {
    if line.is_none() && limit.is_none() {
        return text.to_owned();
    }
    text.split_inclusive('\n')
        .skip(line.unwrap_or(1).saturating_sub(1) as usize)
        .take(limit.map_or(usize::MAX, |limit| limit as usize))
        .collect()
}

/// The one replacement that makes `old` into `new`: what's between the
/// start and the end they share. `None` if they're the same.
fn changed(old: &str, new: &str) -> Option<(Range<usize>, String)> {
    if old == new {
        return None;
    }
    let mut start = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(start) {
        start -= 1;
    }
    let most = old.len().min(new.len()) - start;
    let mut shared = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(most)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - shared) {
        shared -= 1;
    }
    Some((
        start..old.len() - shared,
        new[start..new.len() - shared].to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(old: &str, new: &str) -> String {
        let mut text = old.to_owned();
        if let Some((range, with)) = changed(old, new) {
            text.replace_range(range, &with);
        }
        text
    }

    #[test]
    fn a_write_changes_only_what_differs() {
        assert_eq!(changed("same", "same"), None);
        assert_eq!(
            changed("one two three", "one 2 three"),
            Some((4..7, "2".to_owned()))
        );
        assert_eq!(changed("abc", "abcd"), Some((3..3, "d".to_owned())));
        assert_eq!(changed("abcd", "abc"), Some((3..4, String::new())));
        // Shared bytes inside a character aren't split.
        assert_eq!(changed("é", "è"), Some((0..2, "è".to_owned())));
        for (old, new) in [
            ("aaa", "aa"),
            ("aa", "aaa"),
            ("", "x"),
            ("x", ""),
            ("añb", "aób"),
        ] {
            assert_eq!(apply(old, new), new);
        }
    }

    #[test]
    fn a_read_can_be_some_of_the_lines() {
        let text = "one\ntwo\nthree\n";
        assert_eq!(lines(text, None, None), text);
        assert_eq!(lines(text, Some(2), None), "two\nthree\n");
        assert_eq!(lines(text, Some(2), Some(1)), "two\n");
        assert_eq!(lines(text, None, Some(2)), "one\ntwo\n");
        assert_eq!(lines(text, Some(9), None), "");
    }
}
