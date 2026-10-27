//! Pinned, statically bundled Tree-sitter grammars and their upstream queries.
pub mod rust {
    unsafe extern "C" {
        fn bed_tree_sitter_rust_orchard() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_rust_orchard) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/rust/highlights.scm");
    pub const INJECTIONS_QUERY: &str = include_str!("../grammars/rust/injections.scm");
    pub const LOCALS_QUERY: &str = "";
}
pub mod c {
    unsafe extern "C" {
        fn bed_tree_sitter_c() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_c) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/c/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod cpp {
    unsafe extern "C" {
        fn bed_tree_sitter_cpp() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_cpp) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/cpp/highlights.scm");
    pub const INJECTIONS_QUERY: &str = include_str!("../grammars/cpp/injections.scm");
    pub const LOCALS_QUERY: &str = "";
}
pub mod c_sharp {
    unsafe extern "C" {
        fn bed_tree_sitter_c_sharp() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_c_sharp) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/c-sharp/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod java {
    unsafe extern "C" {
        fn bed_tree_sitter_java() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_java) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/java/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod go {
    unsafe extern "C" {
        fn bed_tree_sitter_go() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_go) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/go/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod swift {
    unsafe extern "C" {
        fn bed_tree_sitter_swift() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_swift) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/swift/highlights.scm");
    pub const INJECTIONS_QUERY: &str = include_str!("../grammars/swift/injections.scm");
    pub const LOCALS_QUERY: &str = include_str!("../grammars/swift/locals.scm");
}
pub mod kotlin {
    unsafe extern "C" {
        fn bed_tree_sitter_kotlin() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_kotlin) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/kotlin/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod dart {
    unsafe extern "C" {
        fn bed_tree_sitter_dart() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_dart) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/dart/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod python {
    unsafe extern "C" {
        fn bed_tree_sitter_python() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_python) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/python/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod ruby {
    unsafe extern "C" {
        fn bed_tree_sitter_ruby() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_ruby) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/ruby/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod php {
    unsafe extern "C" {
        fn bed_tree_sitter_php() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_php) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/php/highlights.scm");
    pub const INJECTIONS_QUERY: &str = include_str!("../grammars/php/injections.scm");
    pub const LOCALS_QUERY: &str = "";
}
pub mod lua {
    unsafe extern "C" {
        fn bed_tree_sitter_lua() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_lua) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/lua/highlights.scm");
    pub const INJECTIONS_QUERY: &str = include_str!("../grammars/lua/injections.scm");
    pub const LOCALS_QUERY: &str = include_str!("../grammars/lua/locals.scm");
}
pub mod bash {
    unsafe extern "C" {
        fn bed_tree_sitter_bash() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_bash) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/bash/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod powershell {
    unsafe extern "C" {
        fn bed_tree_sitter_powershell() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_powershell) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/powershell/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod javascript {
    unsafe extern "C" {
        fn bed_tree_sitter_javascript() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_javascript) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/javascript/highlights.scm");
    pub const INJECTIONS_QUERY: &str = include_str!("../grammars/javascript/injections.scm");
    pub const LOCALS_QUERY: &str = include_str!("../grammars/javascript/locals.scm");
}
pub mod typescript {
    unsafe extern "C" {
        fn bed_tree_sitter_typescript() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_typescript) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/typescript/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = include_str!("../grammars/typescript/locals.scm");
}
pub mod tsx {
    unsafe extern "C" {
        fn bed_tree_sitter_tsx() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_tsx) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/tsx/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = include_str!("../grammars/tsx/locals.scm");
}
pub mod html {
    unsafe extern "C" {
        fn bed_tree_sitter_html() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_html) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/html/highlights.scm");
    pub const INJECTIONS_QUERY: &str = include_str!("../grammars/html/injections.scm");
    pub const LOCALS_QUERY: &str = "";
}
pub mod css {
    unsafe extern "C" {
        fn bed_tree_sitter_css() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_css) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/css/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod json {
    unsafe extern "C" {
        fn bed_tree_sitter_json() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_json) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/json/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod yaml {
    unsafe extern "C" {
        fn bed_tree_sitter_yaml() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_yaml) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/yaml/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod toml {
    unsafe extern "C" {
        fn bed_tree_sitter_toml() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_toml) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/toml/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod xml {
    unsafe extern "C" {
        fn bed_tree_sitter_xml() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_xml) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/xml/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod markdown {
    unsafe extern "C" {
        fn bed_tree_sitter_markdown() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_markdown) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/markdown/highlights.scm");
    pub const INJECTIONS_QUERY: &str = include_str!("../grammars/markdown/injections.scm");
    pub const LOCALS_QUERY: &str = "";
}
pub mod sql {
    unsafe extern "C" {
        fn bed_tree_sitter_sql() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_sql) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/sql/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod dockerfile {
    unsafe extern "C" {
        fn bed_tree_sitter_dockerfile() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_dockerfile) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/dockerfile/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod cmake {
    unsafe extern "C" {
        fn bed_tree_sitter_cmake() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_cmake) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/cmake/highlights.scm");
    pub const INJECTIONS_QUERY: &str = include_str!("../grammars/cmake/injections.scm");
    pub const LOCALS_QUERY: &str = "";
}
pub mod make {
    unsafe extern "C" {
        fn bed_tree_sitter_make() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_make) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/make/highlights.scm");
    pub const INJECTIONS_QUERY: &str = include_str!("../grammars/make/injections.scm");
    pub const LOCALS_QUERY: &str = "";
}
pub mod hcl {
    unsafe extern "C" {
        fn bed_tree_sitter_hcl() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_hcl) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/hcl/highlights.scm");
    pub const INJECTIONS_QUERY: &str = "";
    pub const LOCALS_QUERY: &str = "";
}
pub mod markdown_inline {
    unsafe extern "C" {
        fn bed_tree_sitter_markdown_inline() -> *const ();
    }
    pub fn language() -> tree_sitter_language::LanguageFn {
        // SAFETY: generated Tree-sitter entrypoint; symbols are private to this bundle.
        unsafe { tree_sitter_language::LanguageFn::from_raw(bed_tree_sitter_markdown_inline) }
    }
    pub const HIGHLIGHTS_QUERY: &str = include_str!("../grammars/markdown_inline/highlights.scm");
    pub const INJECTIONS_QUERY: &str = include_str!("../grammars/markdown_inline/injections.scm");
    pub const LOCALS_QUERY: &str = "";
}
