//! Arborium owns grammar discovery and query compilation. Markdown needs one
//! additional inline grammar because its bundled grammar parses only blocks.
//! Small Kotlin additions preserve import and interpolation roles Bed supports.
use arborium::advanced::{CompiledGrammar, GrammarConfig};
use std::sync::{Arc, OnceLock};

pub(crate) fn grammar_store() -> &'static Arc<arborium::GrammarStore> {
    static STORE: OnceLock<Arc<arborium::GrammarStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        let store = Arc::new(arborium::GrammarStore::new());
        let inline = CompiledGrammar::new(GrammarConfig {
            language: tree_sitter_md::INLINE_LANGUAGE.into(),
            highlights_query: tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
            injections_query: tree_sitter_md::INJECTION_QUERY_INLINE,
            locals_query: "",
        })
        .expect("bundled Markdown inline queries must compile");
        store.insert("markdown_inline", Arc::new(inline));

        let injections = format!(
            "{}\n((inline) @injection.content (#set! injection.language \"markdown_inline\"))",
            arborium::lang_markdown::INJECTIONS_QUERY
        );
        let markdown = CompiledGrammar::new(GrammarConfig {
            language: arborium::lang_markdown::language().into(),
            highlights_query: arborium::lang_markdown::HIGHLIGHTS_QUERY,
            injections_query: &injections,
            locals_query: arborium::lang_markdown::LOCALS_QUERY,
        })
        .expect("bundled Markdown block queries must compile");
        store.insert("markdown", Arc::new(markdown));

        let highlights = format!(
            "{}\n{}",
            arborium::lang_kotlin::HIGHLIGHTS_QUERY,
            KOTLIN_ADDITIONS
        );
        let kotlin = CompiledGrammar::new(GrammarConfig {
            language: arborium::lang_kotlin::language().into(),
            highlights_query: &highlights,
            injections_query: arborium::lang_kotlin::INJECTIONS_QUERY,
            locals_query: arborium::lang_kotlin::LOCALS_QUERY,
        })
        .expect("bundled Kotlin highlight queries must compile");
        store.insert("kotlin", Arc::new(kotlin));
        store
    })
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

pub(crate) fn language_name(id: &str) -> Option<&'static str> {
    let id = id.trim().trim_start_matches('.').to_ascii_lowercase();
    let name = id
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&id)
        .trim_start_matches('.');
    // Keep the few file aliases that Arborium's generated registry omits here.
    // The language and grammar registry itself stays upstream.
    match name {
        "makefile" | "gnumakefile" => Some("make"),
        "dockerfile" | "containerfile" => Some("dockerfile"),
        "cmakelists.txt" => Some("cmake"),
        "meson.build" | "meson.options" | "meson_options.txt" => Some("meson"),
        "justfile" => Some("just"),
        "caddyfile" => Some("caddy"),
        "ssh_config" | "sshd_config" => Some("ssh-config"),
        "nginx.conf" => Some("nginx"),
        "bashrc" | "bash_profile" | "bash_login" | "bash_logout" | "profile" => Some("bash"),
        "zshrc" | "zshenv" | "zprofile" | "zlogin" | "zlogout" => Some("zsh"),
        _ => match name.rsplit('.').next().unwrap_or(name) {
            "cc" | "h" | "hh" | "hxx" | "ipp" | "tpp" => Some("cpp"),
            "cshtml" | "razor" => Some("html"),
            "mdown" | "mkd" | "mkdn" => Some("markdown"),
            "tfvars" => Some("hcl"),
            "mk" => Some("make"),
            _ => arborium::detect_language(name),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_upstream_languages_and_bed_file_aliases() {
        for (id, expected) in [
            (".MD", "markdown"),
            ("markdown", "markdown"),
            ("python3", "python"),
            ("C-Sharp", "c-sharp"),
            ("swift", "swift"),
            ("YML", "yaml"),
            ("cc", "cpp"),
            ("/tmp/main.cc", "cpp"),
            ("h", "cpp"),
            ("cshtml", "html"),
            ("Makefile", "make"),
            ("/tmp/Dockerfile", "dockerfile"),
            ("CMakeLists.txt", "cmake"),
            (".zshrc", "zsh"),
        ] {
            assert_eq!(language_name(id), Some(expected), "{id}");
        }
        assert_eq!(language_name("txt"), None);
        assert_eq!(language_name("unknown-language"), None);
    }

    #[test]
    fn markdown_combines_block_inline_and_fenced_language_highlights() {
        let source = "# Heading\n\n**bold** and *emphasis*, `inline code`, [link](https://example.com).\n\n```rust\nfn main() { let value = 42; }\n```\n";
        let mut highlighter = arborium::Highlighter::with_store(Arc::clone(grammar_store()));
        let spans = highlighter.highlight_spans("md", source).unwrap();
        for (text, capture) in [
            ("Heading", "text.title"),
            ("**bold**", "text.strong"),
            ("*emphasis*", "text.emphasis"),
            ("`inline code`", "text.literal"),
            ("https://example.com", "text.uri"),
            ("fn", "keyword"),
            ("42", "constant.builtin"),
        ] {
            assert!(
                spans.iter().any(|span| {
                    span.capture == capture
                        && &source[span.start as usize..span.end as usize] == text
                }),
                "missing {capture} capture for {text}: {spans:?}"
            );
        }
    }
}
