use super::*;
use crate::EditorError;
use unicode_segmentation::UnicodeSegmentation;

fn reference_lines(text: &str) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut bytes = text.bytes().enumerate().peekable();
    while let Some((at, byte)) = bytes.next() {
        if byte == b'\r' || byte == b'\n' {
            lines.push(start..at);
            start = if byte == b'\r' && bytes.peek().is_some_and(|(_, byte)| *byte == b'\n') {
                bytes.next().unwrap().0 + 1
            } else {
                at + 1
            };
        }
    }
    lines.push(start..text.len());
    lines
}

#[test]
fn edits_and_line_metrics_match_flat_text_across_leaf_boundaries() {
    let mut flat = "abc\té🙂\r\nsecond\nthird\r".repeat(500);
    let mut rope = TextBuffer::from(flat.as_str());
    let original = rope.clone();
    let original_text = flat.clone();
    let replacements = ["", "é🙂", "\n", "\r\n", "\tX", "a\r"];
    let mut random = 17_u64;
    for step in 0..1000 {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        let a = rope.clamp_boundary(random as usize % (rope.len() + 1));
        let b = rope.clamp_boundary((a + (random >> 32) as usize % 17).min(rope.len()));
        let replacement = replacements[step % replacements.len()];
        BufferEdit::new(a..b, replacement).apply(&mut rope).unwrap();
        flat.replace_range(a..b, replacement);
        if step % 20 == 0 {
            assert_eq!(rope.to_string(), flat);
            let lines = reference_lines(&flat);
            assert_eq!(rope.line_count(), lines.len());
            for (row, line) in lines.iter().enumerate() {
                assert_eq!(rope.line_range(row), *line);
                assert_eq!(rope.line_of(line.start), row);
                let mut width = 0;
                for ch in flat[line.clone()].chars() {
                    width += cell_width(ch, width);
                }
                assert_eq!(rope.column(line.end), width);
                assert_eq!(rope.byte_at_column(row, usize::MAX), line.end);
            }
        }
    }
    assert_eq!(original.to_string(), original_text);
}

#[test]
fn snapshots_and_undo_restore_exact_shared_roots() {
    let mut rope = TextBuffer::from("unchanged\n".repeat(100_000));
    let original = rope.clone();
    BufferEdit::new(0..0, "").apply(&mut rope).unwrap();
    assert!(rope.ptr_eq(&original));
    BufferEdit::new(0..1, "u").apply(&mut rope).unwrap();
    assert!(
        rope.ptr_eq(&original),
        "A tiny no-op must not force whole-file equality"
    );
    let at = rope.line_start(99_999);
    let inverse = BufferEdit::new(at..at, "é🙂").apply(&mut rope).unwrap();
    assert!(!rope.ptr_eq(&original));
    let redo = inverse.apply(&mut rope).unwrap();
    assert!(rope.ptr_eq(&original));
    redo.apply(&mut rope).unwrap();
    assert_eq!(rope.range_text(at..at + "é🙂".len()), "é🙂");
    assert_eq!(original.line_count(), 100_001);
}

#[test]
fn repeated_edits_keep_lookup_and_old_snapshots_correct() {
    let mut rope = TextBuffer::default();
    let mut original = None;
    for i in 0..20_000 {
        let at = rope.len();
        BufferEdit::new(at..at, "line é🙂\n")
            .apply(&mut rope)
            .unwrap();
        if i == 10_000 {
            original = Some(rope.clone());
        }
    }
    assert_eq!(rope.line_count(), 20_001);
    assert_eq!(rope.line_start(19_999), 19_999 * "line é🙂\n".len());
    assert_eq!(original.unwrap().line_count(), 10_002);
}

#[test]
fn grapheme_movement_crosses_rope_leaves_and_keeps_crlf_together() {
    let flat = "e\u{301}👩‍🚀🇨🇦\r\n".repeat(1000);
    let rope = TextBuffer::from(flat.as_str());
    let mut byte = 0;
    for grapheme in flat.graphemes(true) {
        let next = byte + grapheme.len();
        assert_eq!(rope.next_grapheme(byte), next);
        assert_eq!(rope.previous_grapheme(next), byte);
        byte = next;
    }
}

#[test]
fn rejected_ranges_preserve_snapshots_and_fingerprints_ignore_tree_shape() {
    let mut rope = TextBuffer::from("é🙂\n".repeat(5000));
    let original = rope.clone();
    assert!(
        BufferEdit::new(1..2, "bad")
            .apply(&mut rope)
            .map_err(EditorError)
            .is_err()
    );
    assert!(rope.ptr_eq(&original));
    for _ in 0..100 {
        BufferEdit::new(0..0, "x").apply(&mut rope).unwrap();
    }
    let flat = TextBuffer::from(rope.to_string());
    assert_eq!(rope, flat);
    assert_eq!(rope.fingerprint(), flat.fingerprint());
}
