// Ported from nealmick/ned editor/util/utf8.h and the byte/Unicode helpers in
// editor/util/editor_utils.h at 2d3e9b53b0ebc44c6da6901ff242a95edaa4a1ff.
// Upstream MIT/X Consortium; see NOTICE (ned section).

/// Decode one character as upstream does. Invalid bytes consume one byte and
/// produce U+FFFD. Upstream deliberately accepts encoded UTF-16 surrogates.
pub fn decode_utf8(s: &[u8], i: i32) -> (i32, u32) {
    if i < 0 || i as usize >= s.len() {
        return (0, 0);
    }
    let i = i as usize;
    let c0 = s[i];
    if c0 < 0x80 {
        return (1, c0 as u32);
    }
    if c0 & 0xe0 == 0xc0 && i + 1 < s.len() {
        let c1 = s[i + 1];
        if c1 & 0xc0 == 0x80 {
            let cp = ((c0 & 0x1f) as u32) << 6 | (c1 & 0x3f) as u32;
            if cp >= 0x80 {
                return (2, cp);
            }
        }
    } else if c0 & 0xf0 == 0xe0 && i + 2 < s.len() {
        let (c1, c2) = (s[i + 1], s[i + 2]);
        if c1 & 0xc0 == 0x80 && c2 & 0xc0 == 0x80 {
            let cp = ((c0 & 0x0f) as u32) << 12 | ((c1 & 0x3f) as u32) << 6 | (c2 & 0x3f) as u32;
            if cp >= 0x800 {
                return (3, cp);
            }
        }
    } else if c0 & 0xf8 == 0xf0 && i + 3 < s.len() {
        let (c1, c2, c3) = (s[i + 1], s[i + 2], s[i + 3]);
        if c1 & 0xc0 == 0x80 && c2 & 0xc0 == 0x80 && c3 & 0xc0 == 0x80 {
            let cp = ((c0 & 7) as u32) << 18
                | ((c1 & 0x3f) as u32) << 12
                | ((c2 & 0x3f) as u32) << 6
                | (c3 & 0x3f) as u32;
            if (0x10000..=0x10ffff).contains(&cp) {
                return (4, cp);
            }
        }
    }
    (1, 0xfffd)
}

pub fn utf16_units_for_codepoint(cp: u32) -> i32 {
    if cp > 0xffff { 2 } else { 1 }
}

pub fn utf8_byte_offset_to_utf16(s: &[u8], byte_offset: i32) -> i32 {
    let byte_offset = byte_offset.clamp(0, s.len() as i32);
    let (mut units, mut i) = (0, 0);
    while i < byte_offset {
        let (adv, cp) = decode_utf8(s, i);
        if adv <= 0 || i + adv > byte_offset {
            break;
        }
        units += utf16_units_for_codepoint(cp);
        i += adv;
    }
    units
}

pub fn utf16_to_utf8_byte_offset(s: &[u8], utf16_units: i32) -> i32 {
    if utf16_units <= 0 {
        return 0;
    }
    let (mut units, mut i) = (0, 0);
    while i < s.len() as i32 && units < utf16_units {
        let (adv, cp) = decode_utf8(s, i);
        if adv <= 0 {
            break;
        }
        units += utf16_units_for_codepoint(cp);
        i += adv;
    }
    i
}

pub fn snap_to_utf8_char_boundary(s: &[u8], mut idx: i32) -> i32 {
    if idx <= 0 || idx >= s.len() as i32 {
        return idx;
    }
    while idx > 0 && s[idx as usize] & 0xc0 == 0x80 {
        idx -= 1;
    }
    idx
}

pub fn append_utf8_codepoint(out: &mut Vec<u8>, cp: u32) -> i32 {
    if cp < 0x80 {
        out.push(cp as u8);
        1
    } else if cp < 0x800 {
        out.extend_from_slice(&[(0xc0 | (cp >> 6)) as u8, (0x80 | (cp & 0x3f)) as u8]);
        2
    } else if cp < 0x10000 {
        out.extend_from_slice(&[
            (0xe0 | (cp >> 12)) as u8,
            (0x80 | ((cp >> 6) & 0x3f)) as u8,
            (0x80 | (cp & 0x3f)) as u8,
        ]);
        3
    } else if cp <= 0x10ffff {
        out.extend_from_slice(&[
            (0xf0 | (cp >> 18)) as u8,
            (0x80 | ((cp >> 12) & 0x3f)) as u8,
            (0x80 | ((cp >> 6) & 0x3f)) as u8,
            (0x80 | (cp & 0x3f)) as u8,
        ]);
        4
    } else {
        0
    }
}

pub fn prev_utf8_char(s: &[u8], idx: i32) -> i32 {
    let mut idx = idx.clamp(0, s.len() as i32);
    if idx == 0 {
        return 0;
    }
    idx -= 1;
    while idx > 0 && s[idx as usize] & 0xc0 == 0x80 {
        idx -= 1;
    }
    idx
}

pub fn next_utf8_char(s: &[u8], idx: i32) -> i32 {
    let mut idx = idx.clamp(0, s.len() as i32);
    if idx == s.len() as i32 {
        return idx;
    }
    idx += 1;
    while idx < s.len() as i32 && s[idx as usize] & 0xc0 == 0x80 {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    // tests/editor/utf8_utils_test.cpp, including encoding helpers originally
    // in editor_utils.h (co-located here to keep GUI code out of this layer).
    #[test]
    fn append_ascii() {
        let mut out = Vec::new();
        assert_eq!(append_utf8_codepoint(&mut out, b'A' as u32), 1);
        assert_eq!(out, b"A");
    }
    #[test]
    fn append_two_bytes() {
        let mut out = Vec::new();
        assert_eq!(append_utf8_codepoint(&mut out, 0xe9), 2);
        assert_eq!(out, [0xc3, 0xa9]);
    }
    #[test]
    fn append_three_bytes() {
        let mut out = Vec::new();
        assert_eq!(append_utf8_codepoint(&mut out, 0x20ac), 3);
        assert_eq!(out, [0xe2, 0x82, 0xac]);
    }
    #[test]
    fn append_four_bytes() {
        let mut out = Vec::new();
        assert_eq!(append_utf8_codepoint(&mut out, 0x1f4da), 4);
        assert_eq!(out, [0xf0, 0x9f, 0x93, 0x9a]);
    }
    #[test]
    fn append_rejects_invalid() {
        let mut out = Vec::new();
        assert_eq!(append_utf8_codepoint(&mut out, 0x110000), 0);
        assert!(out.is_empty());
    }
    #[test]
    fn snap_mid_sequence() {
        let s = b"a\xf0\x9f\x93\x9ab";
        for (idx, expected) in [(0, 0), (1, 1), (2, 1), (3, 1), (4, 1), (5, 5)] {
            assert_eq!(snap_to_utf8_char_boundary(s, idx), expected);
        }
    }
    #[test]
    fn ascii_utf16_columns() {
        assert_eq!(utf8_byte_offset_to_utf16(b"hello", 0), 0);
        assert_eq!(utf8_byte_offset_to_utf16(b"hello", 5), 5);
        assert_eq!(utf16_to_utf8_byte_offset(b"hello", 3), 3);
    }
    #[test]
    fn bmp_utf16_columns() {
        let s = b"caf\xc3\xa9";
        assert_eq!(s.len(), 5);
        assert_eq!(utf8_byte_offset_to_utf16(s, 3), 3);
        assert_eq!(utf8_byte_offset_to_utf16(s, 5), 4);
        assert_eq!(utf16_to_utf8_byte_offset(s, 4), 5);
        assert_eq!(utf16_to_utf8_byte_offset(s, 3), 3);
    }
    #[test]
    fn supplementary_utf16_columns() {
        let s = b"a\xf0\x9f\x93\x9ab";
        assert_eq!(utf8_byte_offset_to_utf16(s, 1), 1);
        assert_eq!(utf8_byte_offset_to_utf16(s, 5), 3);
        assert_eq!(utf8_byte_offset_to_utf16(s, 6), 4);
        assert_eq!(utf16_to_utf8_byte_offset(s, 1), 1);
        assert_eq!(utf16_to_utf8_byte_offset(s, 3), 5);
        assert_eq!(utf16_to_utf8_byte_offset(s, 4), 6);
    }
    #[test]
    fn invalid_sequences_consume_one_byte() {
        for s in [
            &b"\xc0\x80"[..],
            &b"\xe0\x80\x80"[..],
            &b"\xf4\x90\x80\x80"[..],
            &b"\xff"[..],
            &b"\xc3"[..],
        ] {
            assert_eq!(decode_utf8(s, 0), (1, 0xfffd));
        }
        assert_eq!(decode_utf8(b"", 0), (0, 0));
        assert_eq!(decode_utf8(b"a", -1), (0, 0));
    }
    #[test]
    fn upstream_surrogate_behavior_is_preserved() {
        let mut out = Vec::new();
        assert_eq!(append_utf8_codepoint(&mut out, 0xd800), 3);
        assert_eq!(out, [0xed, 0xa0, 0x80]);
        assert_eq!(decode_utf8(&out, 0), (3, 0xd800));
    }
    #[test]
    fn utf16_mid_character_offsets_match_upstream() {
        let s = b"a\xf0\x9f\x93\x9ab";
        for off in 2..5 {
            assert_eq!(utf8_byte_offset_to_utf16(s, off), 1);
        }
        // Upstream snaps a UTF-16 offset in a surrogate pair forward.
        assert_eq!(utf16_to_utf8_byte_offset(s, 2), 5);
        assert_eq!(utf16_to_utf8_byte_offset(s, 99), 6);
        assert_eq!(utf8_byte_offset_to_utf16(s, 99), 4);
    }
}
