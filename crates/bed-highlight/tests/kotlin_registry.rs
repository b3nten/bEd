use bed_highlight::{capture_map::ThemeSlot, tree_sitter::TreeSitter};

fn assert_slot(source: &str, context: &str, token: &str, expected: ThemeSlot) {
    let start = source.find(context).expect("context occurs in source")
        + context.find(token).expect("token occurs in context");
    let row = source[..start].bytes().filter(|&b| b == b'\n').count();
    let column = source[..start]
        .rfind('\n')
        .map_or(start, |line_start| start - line_start - 1);
    let lines = TreeSitter::highlight_snippet("kt", source.as_bytes());
    for offset in 0..token.len() {
        let actual = lines[row]
            .iter()
            .find(|span| {
                span.start <= (column + offset) as i32 && (column + offset) < span.end as usize
            })
            .map(|span| span.slot);
        assert_eq!(
            actual,
            Some(expected),
            "{token:?} in {context:?}, byte {offset}"
        );
    }
}

#[test]
fn kotlin_registry_keeps_declaration_import_and_call_roles() {
    let source = "\
package sample.demo
import sample.Widget as LocalWidget
import sample.makeWidget as buildWidget
import sample.*
typealias Named<T> = List<T>
class Box {
    fun transform(input: String): String = input.trim()
}
fun invoke(receiver: Box) = receiver.value.toString()
val reference = ::transform
";
    for (context, token, slot) in [
        ("package sample.demo", "sample", ThemeSlot::Type),
        ("package sample.demo", "demo", ThemeSlot::Type),
        (
            "import sample.Widget as LocalWidget",
            "Widget",
            ThemeSlot::Type,
        ),
        (
            "import sample.Widget as LocalWidget",
            "LocalWidget",
            ThemeSlot::Type,
        ),
        (
            "import sample.makeWidget as buildWidget",
            "makeWidget",
            ThemeSlot::Function,
        ),
        (
            "import sample.makeWidget as buildWidget",
            "buildWidget",
            ThemeSlot::Function,
        ),
        ("import sample.*", "*", ThemeSlot::String),
        ("typealias Named<T>", "Named", ThemeSlot::Type),
        ("typealias Named<T>", "T", ThemeSlot::Type),
        ("List<T>", "List", ThemeSlot::Special),
        ("class Box", "Box", ThemeSlot::Type),
        ("fun transform", "transform", ThemeSlot::Function),
        ("input: String", "input", ThemeSlot::Parameter),
        ("input: String", "String", ThemeSlot::Special),
        ("input.trim()", "input", ThemeSlot::Variable),
        ("input.trim()", "trim", ThemeSlot::Function),
        ("receiver.value.toString()", "receiver", ThemeSlot::Variable),
        ("receiver.value.toString()", "value", ThemeSlot::Property),
        ("receiver.value.toString()", "toString", ThemeSlot::Function),
        ("::transform", "transform", ThemeSlot::Function),
    ] {
        assert_slot(source, context, token, slot);
    }
}

#[test]
fn kotlin_registry_keeps_numbers_comments_and_reserved_words() {
    let source = r#"/** documentation */
val hexadecimal = 0x2AUL
val binary = 0b1010
val integer = 42L
val fraction = 1.5e2
val yes = true
val no = false
val absent = null
val letter = '\n'
val message = "plain"
fun loop() { for (i in 1..10) { if (i > 5) break; continue } }
"#;
    for (context, token, slot) in [
        ("/** documentation */", "documentation", ThemeSlot::Comment),
        ("0x2AUL", "0x2AUL", ThemeSlot::Number),
        ("0b1010", "0b1010", ThemeSlot::Number),
        ("42L", "42L", ThemeSlot::Number),
        ("1.5e2", "1.5e2", ThemeSlot::Number),
        ("yes = true", "true", ThemeSlot::Keyword),
        ("no = false", "false", ThemeSlot::Keyword),
        ("absent = null", "null", ThemeSlot::Keyword),
        ("'\\n'", "\\n", ThemeSlot::Special),
        ("\"plain\"", "plain", ThemeSlot::String),
        ("break; continue", "break", ThemeSlot::Keyword),
        ("break; continue", "continue", ThemeSlot::Keyword),
    ] {
        assert_slot(source, context, token, slot);
    }
}

#[test]
fn kotlin_string_interpolation_preserves_expression_roles_and_literal_tails() {
    let source = r#"val quoted = "value $item tail ${obj.member} escaped \$literal"
val multiline = """value $item tail ${obj.member}"""
"#;
    for context in [
        r#"val quoted = "value $item tail ${obj.member} escaped \$literal""#,
        r#"val multiline = """value $item tail ${obj.member}""""#,
    ] {
        assert_slot(source, context, "value", ThemeSlot::String);
        assert_slot(source, context, "item", ThemeSlot::Variable);
        assert_slot(source, context, " tail ", ThemeSlot::String);
        assert_slot(source, context, "obj", ThemeSlot::Variable);
        assert_slot(source, context, "member", ThemeSlot::Property);
    }
    assert_slot(source, "escaped \\$literal", "literal", ThemeSlot::String);
}
