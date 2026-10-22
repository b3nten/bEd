use tree_sitter::Language;

pub const LANGUAGES: &[&str] = &[
    "rust",
    "c",
    "cpp",
    "c-sharp",
    "java",
    "go",
    "swift",
    "kotlin",
    "dart",
    "python",
    "ruby",
    "php",
    "lua",
    "bash",
    "powershell",
    "javascript",
    "typescript",
    "tsx",
    "html",
    "css",
    "json",
    "yaml",
    "toml",
    "xml",
    "markdown",
    "sql",
    "dockerfile",
    "cmake",
    "make",
    "hcl",
];

pub fn language_name(id_or_path: &str) -> Option<&'static str> {
    let id = id_or_path.trim().to_ascii_lowercase();
    let name = id.rsplit(['/', '\\']).next()?.trim_start_matches('.');
    let detected = match name {
        "makefile" | "gnumakefile" | "mk" => "make",
        "dockerfile" | "containerfile" => "dockerfile",
        "cmakelists.txt" => "cmake",
        "bashrc" | "bash_profile" | "profile" | "sh" | "shell" => "bash",
        "csharp" | "c#" | "cs" => "c-sharp",
        "jsx" | "js" => "javascript",
        "py" | "python3" => "python",
        "tfvars" | "tf" => "hcl",
        "cc" | "h" | "hh" | "hxx" | "ipp" | "tpp" => "cpp",
        "mdown" | "mkd" | "mkdn" => "markdown",
        "cshtml" | "razor" => "html",
        _ => {
            let suffix = name.rsplit('.').next()?;
            match suffix {
                "rs" => "rust",
                "c" => "c",
                "cpp" | "cc" | "cxx" | "h" | "hpp" | "hxx" => "cpp",
                "cs" => "c-sharp",
                "java" => "java",
                "go" => "go",
                "swift" => "swift",
                "kt" | "kts" => "kotlin",
                "dart" => "dart",
                "py" | "pyw" => "python",
                "rb" | "rake" => "ruby",
                "php" | "phtml" => "php",
                "lua" => "lua",
                "sh" | "bash" => "bash",
                "ps1" | "psm1" | "psd1" => "powershell",
                "js" | "jsx" | "mjs" | "cjs" => "javascript",
                "ts" | "mts" | "cts" => "typescript",
                "tsx" => "tsx",
                "html" | "htm" => "html",
                "css" => "css",
                "json" | "jsonc" => "json",
                "yaml" | "yml" => "yaml",
                "toml" => "toml",
                "xml" | "svg" | "xsl" => "xml",
                "md" | "markdown" | "mdown" | "mkd" => "markdown",
                "sql" => "sql",
                "dockerfile" => "dockerfile",
                "cmake" => "cmake",
                "mk" => "make",
                "tf" | "tfvars" | "hcl" => "hcl",
                other => LANGUAGES.iter().copied().find(|l| *l == other)?,
            }
        }
    };
    LANGUAGES.iter().copied().find(|name| *name == detected)
}

pub(crate) struct GrammarSource {
    pub language: Language,
    pub highlights: String,
    pub injections: String,
    pub locals: &'static str,
}
pub(crate) fn grammar(name: &str) -> Option<GrammarSource> {
    macro_rules! sources { ($($name:literal => $module:ident),* $(,)?) => { match name {
        $($name => Some(GrammarSource {
            language: bedtime_grammars2::$module::language().into(),
            highlights: bedtime_grammars2::$module::HIGHLIGHTS_QUERY.into(),
            injections: bedtime_grammars2::$module::INJECTIONS_QUERY.into(),
            locals: bedtime_grammars2::$module::LOCALS_QUERY,
        }),)*
        "markdown_inline" => Some(GrammarSource {
            language: bedtime_grammars2::markdown_inline::language().into(),
            highlights: bedtime_grammars2::markdown_inline::HIGHLIGHTS_QUERY.into(),
            injections: bedtime_grammars2::markdown_inline::INJECTIONS_QUERY.into(), locals: "",
        }),
        _ => None,
    } }; }
    let mut source = sources!(
        "rust" => rust,
        "c" => c,
        "cpp" => cpp,
        "c-sharp" => c_sharp,
        "java" => java,
        "go" => go,
        "swift" => swift,
        "kotlin" => kotlin,
        "dart" => dart,
        "python" => python,
        "ruby" => ruby,
        "php" => php,
        "lua" => lua,
        "bash" => bash,
        "powershell" => powershell,
        "javascript" => javascript,
        "typescript" => typescript,
        "tsx" => tsx,
        "html" => html,
        "css" => css,
        "json" => json,
        "yaml" => yaml,
        "toml" => toml,
        "xml" => xml,
        "markdown" => markdown,
        "sql" => sql,
        "dockerfile" => dockerfile,
        "cmake" => cmake,
        "make" => make,
        "hcl" => hcl,
    )?;
    if name == "markdown" {
        source.injections.push_str(
            "\n((inline) @injection.content (#set! injection.language \"markdown_inline\"))",
        );
    }
    if name == "cmake" {
        source.highlights = source
            .highlights
            .replace("#lua-match?", "#match?")
            .replace("^[%u@][%u%d_]+$", "^[A-Z@][A-Z0-9_]+$");
    }
    if name == "kotlin" {
        source.highlights.push_str(KOTLIN_ADDITIONS);
    }
    Some(source)
}

const KOTLIN_ADDITIONS: &str = r#"
(package_header
  (identifier
    (simple_identifier) @module))

(import_header
  (identifier
    (simple_identifier) @type @_import)
  (import_alias
    (type_identifier) @type.definition)?
  (#match? @_import "^[A-Z]"))

(import_header
  (identifier
    (simple_identifier) @function @_import .)
  (import_alias
    (type_identifier) @function)?
  (#match? @_import "^[a-z]"))

(wildcard_import) @character.special

(callable_reference
  (simple_identifier) @function.call)

(interpolated_identifier) @variable
"#;
