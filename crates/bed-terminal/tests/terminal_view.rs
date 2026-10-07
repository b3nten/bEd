//! Verbatim pinned terminal DrawOp fixtures, independent of font discovery/GPU.
use bed_terminal::{
    terminal::Terminal,
    terminal_view::{DrawOp, PaintMetrics, render_ops},
};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn color(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}
fn op_json(op: &DrawOp) -> Value {
    match op {
        DrawOp::Rect { p0, p1, color: rgb } => {
            json!({"kind":"RECT","p0":p0,"p1":p1,"col":color(*rgb)})
        }
        DrawOp::Text {
            pos,
            color: rgb,
            style,
            character,
        } => {
            json!({"kind":"TEXT","p0":pos,"col":color(*rgb),"font":(["regular","bold","italic","bold_italic"][*style]),"text":character.to_string()})
        }
        DrawOp::PushClip { p0, p1 } => json!({"kind":"PUSH_CLIP","p0":p0,"p1":p1}),
        DrawOp::PopClip => json!({"kind":"POP_CLIP"}),
    }
}
fn compare_numbers(actual: &Value, expected: &Value, case: &str, path: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(e)) => assert!(
            (a.as_f64().unwrap() - e.as_f64().unwrap()).abs() < 0.0001,
            "{case} {path}: {a} != {e}"
        ),
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "{case} {path} array lengths");
            for (i, (a, e)) in a.iter().zip(e).enumerate() {
                compare_numbers(a, e, case, &format!("{path}[{i}]"));
            }
        }
        (Value::Object(a), Value::Object(e)) => {
            assert_eq!(a.len(), e.len(), "{case} {path} keys");
            for (k, e) in e {
                compare_numbers(a.get(k).unwrap(), e, case, &format!("{path}.{k}"));
            }
        }
        _ => assert_eq!(actual, expected, "{case} {path}"),
    }
}
#[test]
fn nine_original_terminal_draw_caches_match_all_operations() {
    let base = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .join("tests/fixtures/terminal_core");
    for name in [
        "hello",
        "attrs_basic",
        "colors_ansi_16",
        "colors_truecolor",
        "colors_osc4",
        "wide_cjk",
        "cursor_block",
        "cursor_underline",
        "cursor_bar",
    ] {
        let folder = base.join(name);
        let expected: Value =
            serde_json::from_slice(&fs::read(folder.join("expected.json")).unwrap()).unwrap();
        let mut terminal = Terminal::new(80, 24);
        terminal.feed(&fs::read(folder.join("output.bin")).unwrap());
        let m = PaintMetrics {
            cw: 8.0,
            ch: 16.0,
            ascent: 0.0,
            width: 640.0,
            height: 384.0,
        };
        let paint = render_ops(&terminal, m, true, true, false, [false; 4], |_, _, pos| pos);
        let rows: Vec<_> = paint
            .rows
            .iter()
            .enumerate()
            .map(|(row, ops)| json!({"row":row,"ops":ops.iter().map(op_json).collect::<Vec<_>>()}))
            .collect();
        compare_numbers(&json!(rows), &expected["rows"], name, "rows");
        compare_numbers(
            &json!(paint.overlay.iter().map(op_json).collect::<Vec<_>>()),
            &expected["overlay"],
            name,
            "overlay",
        );
        assert_eq!(
            u64::from(terminal.cursor().shape),
            expected["cursor_shape"].as_u64().unwrap(),
            "{name} cursor"
        );
        assert_eq!(terminal.title(), expected["title"].as_str().unwrap());
    }
}
