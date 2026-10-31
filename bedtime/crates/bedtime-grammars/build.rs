use std::{io::Read, path::Path};

fn main() {
    // Each grammar gets its own include roots; generated headers differ by ABI.
    // This crate bundles desktop native parsers, without downloads or generation.
    for name in [
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
        "markdown_inline",
    ] {
        let root = Path::new("grammars").join(name);
        println!("cargo:rerun-if-changed={}", root.join("src").display());
        for path in [root.join("scanner.c"), root.join("common")] {
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
        let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join(name);
        std::fs::create_dir_all(&output).unwrap();
        let mut parser = Vec::new();
        flate2::read::GzDecoder::new(std::fs::File::open(root.join("src/parser.c.gz")).unwrap())
            .read_to_end(&mut parser)
            .unwrap();
        let parser_path = output.join("parser.c");
        std::fs::write(&parser_path, parser).unwrap();
        let mut build = cc::Build::new();
        build
            .include(root.join("src"))
            .include(&root)
            .warnings(false)
            .opt_level_str("z")
            .flag_if_supported("-Wno-trigraphs")
            .file(parser_path);
        for scanner in [root.join("scanner.c"), root.join("src/scanner.c")] {
            if scanner.exists() {
                build.file(scanner);
            }
        }
        build.compile(&format!("bed_{}", name.replace('-', "_")));
    }
}
