// Translated from nealmick/ned editor/services/highlight/capture_map.h.
// Source revision and attribution in NOTICE; upstream MIT/X Consortium license in NOTICE (ned section).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ThemeSlot {
    #[default]
    Text = 0,
    Comment,
    Keyword,
    String,
    Number,
    Function,
    Type,
    Variable,
    Parameter,
    Property,
    Constant,
    Operator,
    Punctuation,
    Special,
}
pub const THEME_KEYS: [&str; 14] = [
    "text",
    "comment",
    "keyword",
    "string",
    "number",
    "function",
    "type",
    "variable",
    "parameter",
    "property",
    "constant",
    "operator",
    "punctuation",
    "special",
];
pub const THEME_SLOTS: [ThemeSlot; 14] = [
    ThemeSlot::Text,
    ThemeSlot::Comment,
    ThemeSlot::Keyword,
    ThemeSlot::String,
    ThemeSlot::Number,
    ThemeSlot::Function,
    ThemeSlot::Type,
    ThemeSlot::Variable,
    ThemeSlot::Parameter,
    ThemeSlot::Property,
    ThemeSlot::Constant,
    ThemeSlot::Operator,
    ThemeSlot::Punctuation,
    ThemeSlot::Special,
];
#[derive(Clone, Copy)]
struct CaptureRule {
    pattern: &'static str,
    prefix: bool,
    slot: ThemeSlot,
    priority: i32,
}
const RULES: &[CaptureRule] = &[
    CaptureRule {
        pattern: "none",
        prefix: false,
        slot: ThemeSlot::Text,
        priority: -1,
    },
    CaptureRule {
        pattern: "default",
        prefix: false,
        slot: ThemeSlot::Text,
        priority: 10,
    },
    CaptureRule {
        pattern: "text",
        prefix: false,
        slot: ThemeSlot::Text,
        priority: 10,
    },
    CaptureRule {
        pattern: "conceal",
        prefix: false,
        slot: ThemeSlot::Text,
        priority: 10,
    },
    CaptureRule {
        pattern: "spell",
        prefix: false,
        slot: ThemeSlot::Text,
        priority: 10,
    },
    CaptureRule {
        pattern: "nospell",
        prefix: false,
        slot: ThemeSlot::Text,
        priority: 10,
    },
    CaptureRule {
        pattern: "embedded",
        prefix: false,
        slot: ThemeSlot::Text,
        priority: 10,
    },
    CaptureRule {
        pattern: "markup.",
        prefix: true,
        slot: ThemeSlot::Text,
        priority: 10,
    },
    CaptureRule {
        pattern: "diff.",
        prefix: true,
        slot: ThemeSlot::Text,
        priority: 10,
    },
    CaptureRule {
        pattern: "comment",
        prefix: false,
        slot: ThemeSlot::Comment,
        priority: 100,
    },
    CaptureRule {
        pattern: "comment.",
        prefix: true,
        slot: ThemeSlot::Comment,
        priority: 100,
    },
    CaptureRule {
        pattern: "keyword.function",
        prefix: false,
        slot: ThemeSlot::Keyword,
        priority: 100,
    },
    CaptureRule {
        pattern: "keyword",
        prefix: false,
        slot: ThemeSlot::Keyword,
        priority: 100,
    },
    CaptureRule {
        pattern: "keyword.",
        prefix: true,
        slot: ThemeSlot::Keyword,
        priority: 100,
    },
    CaptureRule {
        pattern: "boolean",
        prefix: false,
        slot: ThemeSlot::Keyword,
        priority: 100,
    },
    CaptureRule {
        pattern: "string.escape",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 100,
    },
    CaptureRule {
        pattern: "string.escape.",
        prefix: true,
        slot: ThemeSlot::Special,
        priority: 100,
    },
    CaptureRule {
        pattern: "escape",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 100,
    },
    CaptureRule {
        pattern: "string",
        prefix: false,
        slot: ThemeSlot::String,
        priority: 100,
    },
    CaptureRule {
        pattern: "string.",
        prefix: true,
        slot: ThemeSlot::String,
        priority: 100,
    },
    CaptureRule {
        pattern: "character",
        prefix: false,
        slot: ThemeSlot::String,
        priority: 100,
    },
    CaptureRule {
        pattern: "character.",
        prefix: true,
        slot: ThemeSlot::String,
        priority: 100,
    },
    CaptureRule {
        pattern: "number",
        prefix: false,
        slot: ThemeSlot::Number,
        priority: 100,
    },
    CaptureRule {
        pattern: "number.",
        prefix: true,
        slot: ThemeSlot::Number,
        priority: 100,
    },
    CaptureRule {
        pattern: "float",
        prefix: false,
        slot: ThemeSlot::Number,
        priority: 100,
    },
    CaptureRule {
        pattern: "function.macro",
        prefix: false,
        slot: ThemeSlot::Function,
        priority: 110,
    },
    CaptureRule {
        pattern: "function.macro.",
        prefix: true,
        slot: ThemeSlot::Function,
        priority: 110,
    },
    CaptureRule {
        pattern: "macro",
        prefix: false,
        slot: ThemeSlot::Function,
        priority: 110,
    },
    CaptureRule {
        pattern: "function.builtin",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 105,
    },
    CaptureRule {
        pattern: "function",
        prefix: false,
        slot: ThemeSlot::Function,
        priority: 95,
    },
    CaptureRule {
        pattern: "function.",
        prefix: true,
        slot: ThemeSlot::Function,
        priority: 95,
    },
    CaptureRule {
        pattern: "method",
        prefix: false,
        slot: ThemeSlot::Function,
        priority: 95,
    },
    CaptureRule {
        pattern: "constructor",
        prefix: false,
        slot: ThemeSlot::Type,
        priority: 95,
    },
    CaptureRule {
        pattern: "type.builtin",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 105,
    },
    CaptureRule {
        pattern: "type",
        prefix: false,
        slot: ThemeSlot::Type,
        priority: 90,
    },
    CaptureRule {
        pattern: "type.",
        prefix: true,
        slot: ThemeSlot::Type,
        priority: 90,
    },
    CaptureRule {
        pattern: "tag",
        prefix: false,
        slot: ThemeSlot::Type,
        priority: 90,
    },
    CaptureRule {
        pattern: "tag.",
        prefix: true,
        slot: ThemeSlot::Type,
        priority: 90,
    },
    CaptureRule {
        pattern: "module.builtin",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 105,
    },
    CaptureRule {
        pattern: "module",
        prefix: false,
        slot: ThemeSlot::Type,
        priority: 90,
    },
    CaptureRule {
        pattern: "module.",
        prefix: true,
        slot: ThemeSlot::Type,
        priority: 90,
    },
    CaptureRule {
        pattern: "namespace.builtin",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 105,
    },
    CaptureRule {
        pattern: "namespace",
        prefix: false,
        slot: ThemeSlot::Type,
        priority: 90,
    },
    CaptureRule {
        pattern: "label",
        prefix: false,
        slot: ThemeSlot::Type,
        priority: 90,
    },
    CaptureRule {
        pattern: "label.",
        prefix: true,
        slot: ThemeSlot::Type,
        priority: 90,
    },
    CaptureRule {
        pattern: "variable.parameter",
        prefix: false,
        slot: ThemeSlot::Parameter,
        priority: 75,
    },
    CaptureRule {
        pattern: "variable.parameter.",
        prefix: true,
        slot: ThemeSlot::Parameter,
        priority: 75,
    },
    CaptureRule {
        pattern: "parameter",
        prefix: false,
        slot: ThemeSlot::Parameter,
        priority: 75,
    },
    CaptureRule {
        pattern: "variable.member",
        prefix: false,
        slot: ThemeSlot::Property,
        priority: 70,
    },
    CaptureRule {
        pattern: "variable.builtin",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 105,
    },
    CaptureRule {
        pattern: "constant.builtin",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 105,
    },
    CaptureRule {
        pattern: "property",
        prefix: false,
        slot: ThemeSlot::Property,
        priority: 70,
    },
    CaptureRule {
        pattern: "property.",
        prefix: true,
        slot: ThemeSlot::Property,
        priority: 70,
    },
    CaptureRule {
        pattern: "field",
        prefix: false,
        slot: ThemeSlot::Property,
        priority: 70,
    },
    CaptureRule {
        pattern: "attribute",
        prefix: false,
        slot: ThemeSlot::Property,
        priority: 70,
    },
    CaptureRule {
        pattern: "attribute.",
        prefix: true,
        slot: ThemeSlot::Property,
        priority: 70,
    },
    CaptureRule {
        pattern: "constant",
        prefix: false,
        slot: ThemeSlot::Constant,
        priority: 90,
    },
    CaptureRule {
        pattern: "constant.",
        prefix: true,
        slot: ThemeSlot::Constant,
        priority: 90,
    },
    CaptureRule {
        pattern: "variable",
        prefix: false,
        slot: ThemeSlot::Variable,
        priority: 40,
    },
    CaptureRule {
        pattern: "variable.",
        prefix: true,
        slot: ThemeSlot::Variable,
        priority: 40,
    },
    CaptureRule {
        pattern: "operator",
        prefix: false,
        slot: ThemeSlot::Operator,
        priority: 100,
    },
    CaptureRule {
        pattern: "punctuation",
        prefix: false,
        slot: ThemeSlot::Punctuation,
        priority: 50,
    },
    CaptureRule {
        pattern: "punctuation.",
        prefix: true,
        slot: ThemeSlot::Punctuation,
        priority: 50,
    },
    CaptureRule {
        pattern: "delimiter",
        prefix: false,
        slot: ThemeSlot::Punctuation,
        priority: 50,
    },
    CaptureRule {
        pattern: "delimiter.",
        prefix: true,
        slot: ThemeSlot::Punctuation,
        priority: 50,
    },
    CaptureRule {
        pattern: "special",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 100,
    },
    CaptureRule {
        pattern: "hook",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 100,
    },
    CaptureRule {
        pattern: "preprocessor",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 100,
    },
    CaptureRule {
        pattern: "preproc",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 100,
    },
    CaptureRule {
        pattern: "preproc.",
        prefix: true,
        slot: ThemeSlot::Special,
        priority: 100,
    },
    CaptureRule {
        pattern: "define",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 100,
    },
    CaptureRule {
        pattern: "include",
        prefix: false,
        slot: ThemeSlot::Special,
        priority: 100,
    },
];
fn match_rule(name: &str) -> Option<&CaptureRule> {
    RULES.iter().find(|r| {
        if r.prefix {
            name.starts_with(r.pattern)
        } else {
            name == r.pattern
        }
    })
}
pub fn is_none_capture(name: &str) -> bool {
    name == "none"
}
pub fn is_string_capture(name: &str) -> bool {
    name == "string" || (name.len() > 7 && name.starts_with("string."))
}
pub fn capture_priority(name: &str) -> i32 {
    match_rule(name).map_or(30, |r| r.priority)
}
pub fn theme_slot_for_capture(name: &str) -> ThemeSlot {
    match_rule(name).map_or(ThemeSlot::Text, |r| r.slot)
}
pub fn theme_key_for_capture(name: &str) -> &'static str {
    THEME_KEYS[theme_slot_for_capture(name) as usize]
}
pub fn theme_slot_for_key(key: &str) -> ThemeSlot {
    THEME_KEYS
        .iter()
        .position(|&k| k == key)
        .map_or(ThemeSlot::Text, |i| THEME_SLOTS[i])
}
pub fn subtract_ranges(start: u32, end: u32, holes: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    if start >= end {
        return out;
    }
    let mut cur = start;
    for &(hs, he) in holes {
        if he <= cur {
            continue;
        }
        if hs >= end {
            break;
        }
        if hs > cur {
            out.push((cur, hs.min(end)));
        }
        cur = cur.max(he);
        if cur >= end {
            return out;
        }
    }
    if cur < end {
        out.push((cur, end));
    }
    out
}
