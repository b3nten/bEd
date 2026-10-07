#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticItem {
    pub start_line: i32,
    pub start_character: i32,
    pub end_line: i32,
    pub end_character: i32,
    pub severity: i32,
    pub message: String,
    pub source: String,
}
impl Default for DiagnosticItem {
    fn default() -> Self {
        Self {
            start_line: 0,
            start_character: 0,
            end_line: 0,
            end_character: 0,
            severity: 1,
            message: String::new(),
            source: String::new(),
        }
    }
}

/// Upstream diagnostic ends are inclusive, including zero-width ranges.
pub fn diagnostic_contains(item: &DiagnosticItem, line: i32, utf16_column: i32) -> bool {
    let after_start =
        line > item.start_line || (line == item.start_line && utf16_column >= item.start_character);
    let before_end =
        line < item.end_line || (line == item.end_line && utf16_column <= item.end_character);
    after_start && before_end
}
