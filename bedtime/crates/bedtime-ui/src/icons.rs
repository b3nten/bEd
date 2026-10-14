//! Plugin-owned monochrome SVG icons. The desktop owns realization and lifetimes.
use crate::presentation::FileIcons;
use dear_imgui_rs::{StyleColor, TextureId, Ui};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

#[path = "bundled_icons.rs"]
mod bundled_icons;
#[cfg(test)]
const ICON_CATALOG: &str = include_str!("../assets/icons/catalog.tsv");

/// One local filename in a plugin's icon contract. A directory can replace its
/// embedded artwork; missing files retain the embedded default when supplied.
#[derive(Clone, Copy, Debug)]
pub struct IconDefinition {
    pub name: &'static str,
    pub filename: &'static str,
    pub embedded_svg: Option<&'static [u8]>,
}

/// Applications choose source directories; plugin registration reads its own entry.
#[derive(Clone, Debug, Default)]
pub struct IconDirectories(pub BTreeMap<String, PathBuf>);

impl IconDirectories {
    pub fn get(&self, plugin: &str) -> Option<&Path> {
        self.0.get(plugin).map(PathBuf::as_path)
    }
}

#[cfg(test)]
fn icon_keys() -> impl Iterator<Item = &'static str> {
    ICON_CATALOG
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split_whitespace().next().expect("icon catalog key"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Default)]
pub struct Icons {
    pub images: BTreeMap<String, RgbaImage>,
    pub textures: BTreeMap<String, TextureId>,
}

impl Icons {
    /// Shared defaults work without a repository or application asset directory.
    pub fn bundled() -> Self {
        Self::bundled_with_directory(None).expect("embedded core icon contract is valid")
    }

    pub fn bundled_with_directory(directory: Option<&Path>) -> io::Result<Self> {
        let mut icons = Self::default();
        icons.register("bedtime.core", bundled_icons::BUNDLED_ICONS, directory)?;
        Ok(icons)
    }

    /// Fill shared defaults while preserving source images supplied by the host.
    /// Resolve the complete directory first so an invalid source publishes nothing.
    pub fn register_core_defaults(&mut self, directory: Option<&Path>) -> io::Result<()> {
        let defaults = Self::bundled_with_directory(directory)?;
        for (key, image) in defaults.images {
            self.images.entry(key).or_insert(image);
        }
        Ok(())
    }

    /// Access shared embedded artwork when declaring a plugin's own defaults.
    pub fn bundled_svg(name: &str) -> Option<&'static [u8]> {
        bundled_icons::BUNDLED_ICONS
            .iter()
            .find(|icon| icon.name == name)
            .and_then(|icon| icon.embedded_svg)
    }

    /// Publish one complete plugin contribution. Failed source/contract validation
    /// leaves the existing registry intact. Registered images may later be updated
    /// in place; the desktop reconciles native and GPU realizations before drawing.
    pub fn register(
        &mut self,
        plugin: &str,
        definitions: &[IconDefinition],
        directory: Option<&Path>,
    ) -> io::Result<()> {
        if !valid_identifier(plugin) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid icon plugin namespace",
            ));
        }
        let mut pending = BTreeMap::new();
        let mut filenames = BTreeSet::new();
        for icon in definitions {
            let key = format!("{plugin}.{}", icon.name);
            let file = Path::new(icon.filename);
            if !valid_identifier(icon.name)
                || icon.name.contains('.')
                || file.components().count() != 1
                || file.file_name().and_then(|name| name.to_str()) != Some(icon.filename)
                || file.extension().and_then(|ext| ext.to_str()) != Some("svg")
                || !filenames.insert(icon.filename)
                || self.images.contains_key(&key)
                || pending.contains_key(&key)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Invalid or duplicate icon contract: {key} ({})",
                        icon.filename
                    ),
                ));
            }
            let bytes = match directory.map(|directory| fs::read(directory.join(icon.filename))) {
                Some(Ok(bytes)) => bytes,
                Some(Err(error)) if error.kind() != io::ErrorKind::NotFound => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("Icon {key} ({}): {error}", icon.filename),
                    ));
                }
                _ => icon.embedded_svg.map(<[u8]>::to_vec).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("Missing icon {key} ({})", icon.filename),
                    )
                })?,
            };
            let mut image = rasterize_svg(&bytes).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("Icon {key} ({}): {error}", icon.filename),
                )
            })?;
            monochrome(&mut image);
            pending.insert(key, image);
        }
        self.images.extend(pending);
        Ok(())
    }

    pub fn set_texture(&mut self, key: &str, texture: TextureId) {
        self.textures.insert(key.to_owned(), texture);
    }

    pub fn get(&self, name: &str) -> Option<TextureId> {
        self.find(name)
            .or_else(|| self.find("bedtime.core.default"))
    }

    /// Resolve an exact namespaced key; no legacy shorthand is accepted.
    pub fn find(&self, name: &str) -> Option<TextureId> {
        self.textures.get(name).copied()
    }

    pub fn get_for_file(&self, filename: &str) -> Option<TextureId> {
        self.get(icon_key_for_file(filename))
    }

    pub fn file_icon_tint(&self, _filename: &str, text: [f32; 4]) -> [f32; 4] {
        text
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

fn monochrome(image: &mut RgbaImage) {
    // Alpha defines the silhouette; each draw site owns the ink.
    for pixel in image.pixels.as_chunks_mut::<4>().0 {
        pixel[..3].fill(255);
    }
}

impl FileIcons for Icons {
    fn get(&self, name: &str) -> Option<TextureId> {
        self.get(name)
    }

    fn get_for_file(&self, filename: &str) -> Option<TextureId> {
        self.get_for_file(filename)
    }

    fn file_icon_tint(&self, filename: &str, text: [f32; 4]) -> [f32; 4] {
        self.file_icon_tint(filename, text)
    }
}

// Exact filenames take precedence over extension associations.
const FILE_NAMES: &[(&str, &str)] = &[
    ("CMakeLists.txt", "bedtime.core.cmake"),
    ("cmake", "bedtime.core.cmake"),
    (".clangd", "bedtime.core.llvm"),
    (".clang-format", "bedtime.core.llvm"),
    ("Dockerfile", "bedtime.core.docker"),
    ("Containerfile", "bedtime.core.docker"),
    (".dockerignore", "bedtime.core.docker"),
    ("docker-compose.yml", "bedtime.core.docker"),
    ("docker-compose.yaml", "bedtime.core.docker"),
    (".gitignore", "bedtime.core.git"),
    (".gitmodules", "bedtime.core.git"),
    (".gitattributes", "bedtime.core.git"),
    (".editorconfig", "bedtime.core.editorconfig"),
    (".env", "bedtime.core.config"),
    ("Cargo.toml", "bedtime.core.rust"),
    ("Cargo.lock", "bedtime.core.lock"),
    ("package.json", "bedtime.core.npm"),
    ("package-lock.json", "bedtime.core.lock"),
    ("yarn.lock", "bedtime.core.lock"),
    ("tsconfig.json", "bedtime.core.typescript"),
    (".babelrc", "bedtime.core.babel"),
    (".eslintrc", "bedtime.core.eslint"),
    ("Makefile", "bedtime.core.config"),
    ("makefile", "bedtime.core.config"),
    ("GNUmakefile", "bedtime.core.config"),
    ("LICENSE", "bedtime.core.license"),
    ("LICENSE.txt", "bedtime.core.license"),
    ("LICENSE.md", "bedtime.core.license"),
    ("COPYING", "bedtime.core.license"),
    ("README", "bedtime.core.text"),
    ("NOTICE", "bedtime.core.text"),
];

// One canonical asset per language/tool or generic file category.
const FILE_EXTENSIONS: &[(&[&str], &str)] = &[
    (&["rs", "rust"], "bedtime.core.rust"),
    (&["py", "pyi", "pyw", "python"], "bedtime.core.python"),
    (
        &["js", "mjs", "cjs", "javascript"],
        "bedtime.core.javascript",
    ),
    (
        &["ts", "mts", "cts", "typescript", "tsconfig"],
        "bedtime.core.typescript",
    ),
    (&["jsx", "tsx", "cjsx", "react"], "bedtime.core.react"),
    (&["rb", "ruby"], "bedtime.core.ruby"),
    (&["cmake"], "bedtime.core.cmake"),
    (&["clangd", "clang-format"], "bedtime.core.llvm"),
    (&["docker", "dockerfile"], "bedtime.core.docker"),
    (&["sh", "bash", "zsh", "fish", "shell"], "bedtime.core.bash"),
    (&["c", "h"], "bedtime.core.c"),
    (
        &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
        "bedtime.core.cpp",
    ),
    (
        &["cs", "csproj", "c-sharp", "vb", "vbproj", "fsproj"],
        "bedtime.core.dotnet",
    ),
    (&["fs", "fsi", "fsx", "f-sharp"], "bedtime.core.fsharp"),
    (&["r", "rmd"], "bedtime.core.r-language"),
    (
        &["html", "htm", "xhtml", "html_erb", "cshtml"],
        "bedtime.core.html",
    ),
    (&["css"], "bedtime.core.css"),
    (&["md", "markdown", "mdx", "mdo"], "bedtime.core.markdown"),
    (&["json", "jsonc", "json5"], "bedtime.core.json"),
    (&["yml", "yaml"], "bedtime.core.yaml"),
    (
        &["git", "gitignore", "gitmodule", "git_ignore", "git_folder"],
        "bedtime.core.git",
    ),
    (&["github"], "bedtime.core.github"),
    (&["gitlab"], "bedtime.core.gitlab"),
    (&["go", "go2", "mod", "sum"], "bedtime.core.go"),
    (&["gd", "godot", "tscn", "tres"], "bedtime.core.godot"),
    (&["dart"], "bedtime.core.dart"),
    (
        &["ex", "exs", "elixir", "elixir_script"],
        "bedtime.core.elixir",
    ),
    (&["elm"], "bedtime.core.elm"),
    (&["eslint"], "bedtime.core.eslint"),
    (&["ethereum", "sol"], "bedtime.core.ethereum"),
    (&["firebase"], "bedtime.core.firebase"),
    (&["firefox"], "bedtime.core.firefox"),
    (&["gradle"], "bedtime.core.gradle"),
    (&["graphql", "gql"], "bedtime.core.graphql"),
    (&["grunt"], "bedtime.core.grunt"),
    (&["gulp"], "bedtime.core.gulp"),
    (&["hs", "lhs", "haskell"], "bedtime.core.haskell"),
    (&["hx", "haxe"], "bedtime.core.haxe"),
    (&["ionic"], "bedtime.core.ionic"),
    (&["jenkins"], "bedtime.core.jenkins"),
    (&["jinja", "jinja2", "j2"], "bedtime.core.jinja"),
    (&["jl", "julia"], "bedtime.core.julia"),
    (&["kt", "kts", "kotlin"], "bedtime.core.kotlin"),
    (&["less"], "bedtime.core.less"),
    (&["lua"], "bedtime.core.lua"),
    (&["maven"], "bedtime.core.maven"),
    (&["nim", "nims"], "bedtime.core.nim"),
    (&["npm", "npm_ignored"], "bedtime.core.npm"),
    (&["nunjucks", "njk"], "bedtime.core.nunjucks"),
    (&["ml", "mli", "ocaml"], "bedtime.core.ocaml"),
    (&["pl", "pm", "perl"], "bedtime.core.perl"),
    (&["php", "phtml", "cake_php"], "bedtime.core.php"),
    (&["platformio"], "bedtime.core.platformio"),
    (&["prisma"], "bedtime.core.prisma"),
    (&["pug", "jade"], "bedtime.core.pug"),
    (&["pp", "puppet"], "bedtime.core.puppet"),
    (&["purs", "purescript"], "bedtime.core.purescript"),
    (&["re", "rei", "reasonml"], "bedtime.core.reason"),
    (&["res", "resi", "rescript"], "bedtime.core.rescript"),
    (&["rollup"], "bedtime.core.rollup"),
    (&["rails", "erb"], "bedtime.core.rails"),
    (&["sass", "scss"], "bedtime.core.code"),
    (&["scala", "sc"], "bedtime.core.scala"),
    (&["spring"], "bedtime.core.spring"),
    (&["stylelint"], "bedtime.core.stylelint"),
    (&["styl", "stylus"], "bedtime.core.stylus"),
    (
        &["sublime", "sublime-project", "sublime-workspace"],
        "bedtime.core.sublime",
    ),
    (&["svelte"], "bedtime.core.svelte"),
    (&["swift"], "bedtime.core.swift"),
    (&["tf", "tfvars", "terraform"], "bedtime.core.terraform"),
    (&["tex", "latex", "bib"], "bedtime.core.tex"),
    (&["vala", "vapi"], "bedtime.core.code"),
    (&["vue"], "bedtime.core.code"),
    (&["wasm", "wat"], "bedtime.core.wasm"),
    (&["webpack"], "bedtime.core.code"),
    (&["yarn"], "bedtime.core.yarn"),
    (&["zig", "zon"], "bedtime.core.zig"),
    (&["babel"], "bedtime.core.babel"),
    (&["bazel", "bzl"], "bedtime.core.bazel"),
    (&["bower"], "bedtime.core.bower"),
    (&["coffee", "coffee_erb", "cake"], "bedtime.core.coffee"),
    (
        &["cr", "crystal", "crystal_embedded"],
        "bedtime.core.crystal",
    ),
    (&["editorconfig"], "bedtime.core.editorconfig"),
    (&["groovy", "grails"], "bedtime.core.groovy"),
    (
        &["clj", "cljs", "cljc", "edn", "clojure"],
        "bedtime.core.clojure",
    ),
    (&["ejs"], "bedtime.core.ejs"),
    (
        &[
            "ini",
            "cfg",
            "conf",
            "config",
            "toml",
            "env",
            "makefile",
            "pipeline",
            "plan",
            "todo",
            "happenings",
        ],
        "bedtime.core.config",
    ),
    (
        &[
            "xml",
            "xsl",
            "xslt",
            "xsd",
            "svg",
            "odata",
            "plist",
            "java",
            "class",
            "jar",
            "sbt",
            "asm",
            "s",
            "cu",
            "cuh",
            "bsl",
            "d",
            "hacklang",
            "hack",
            "haml",
            "liquid",
            "livescript",
            "ls",
            "mustache",
            "pddl",
            "powershell",
            "ps1",
            "psm1",
            "prolog",
            "pro",
            "salesforce",
            "slim",
            "smarty",
            "tpl",
            "twig",
            "wgt",
            "coldfusion",
            "cfm",
            "cfc",
            "bicep",
            "js_erb",
            "gltf",
            "glb",
            "obj",
            "stl",
            "blend",
        ],
        "bedtime.core.code",
    ),
    (
        &[
            "txt", "text", "log", "rst", "rtf", "doc", "docx", "word", "pdf", "argdown",
        ],
        "bedtime.core.text",
    ),
    (
        &[
            "png",
            "jpg",
            "jpeg",
            "gif",
            "webp",
            "avif",
            "bmp",
            "ico",
            "tif",
            "tiff",
            "heic",
            "exr",
            "hdr",
            "dds",
            "qoi",
            "tga",
            "psd",
            "photoshop",
            "ai",
            "illustrator",
            "favicon",
            "image",
        ],
        "bedtime.core.image",
    ),
    (
        &[
            "mp3", "wav", "flac", "ogg", "m4a", "aac", "aiff", "opus", "audio",
        ],
        "bedtime.core.audio",
    ),
    (
        &["mp4", "mov", "mkv", "avi", "webm", "m4v", "video"],
        "bedtime.core.video",
    ),
    (
        &["zip", "tar", "gz", "bz2", "xz", "7z", "rar", "zst"],
        "bedtime.core.archive",
    ),
    (
        &["csv", "tsv", "xls", "xlsx", "ods"],
        "bedtime.core.spreadsheet",
    ),
    (
        &["ttf", "otf", "woff", "woff2", "font"],
        "bedtime.core.font",
    ),
    (&["db", "sqlite", "sqlite3", "sql"], "bedtime.core.database"),
    (&["lock"], "bedtime.core.lock"),
    (&["ipynb", "notebook"], "bedtime.core.notebook"),
    (&["license"], "bedtime.core.license"),
    (&["bin", "hex"], "bedtime.core.hex"),
];

pub fn icon_key_for_file(filename: &str) -> &'static str {
    let path = Path::new(filename);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if let Some((_, key)) = FILE_NAMES.iter().find(|(candidate, _)| *candidate == name) {
        return key;
    }
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    FILE_EXTENSIONS
        .iter()
        .find(|(extensions, _)| {
            extensions
                .iter()
                .any(|ext| ext.eq_ignore_ascii_case(extension))
        })
        .map_or("bedtime.core.default", |(_, key)| key)
}

/// Map existing command identifiers to the artwork used in panels.
pub fn command_icon_key(name: Option<&str>) -> &'static str {
    match name {
        Some("files" | "filetree" | "sidebar") => "bedtime.core.files",
        Some("terminal") => "bedtime.core.terminal",
        Some("search") => "bedtime.core.search",
        Some("structure") => "bedtime.core.structure",
        Some("diagnostics") => "bedtime.core.diagnostics",
        Some("debug") => "bedtime.core.debug",
        Some("git") => "bedtime.core.git-branch",
        Some("split_right") => "bedtime.core.split_right",
        Some("split_down") => "bedtime.core.split_down",
        Some("gear" | "settings") => "bedtime.core.settings",
        Some("image") => "bedtime.core.viewer-image",
        Some("hex") => "bedtime.core.hex",
        _ => "bedtime.core.extension",
    }
}

/// Retain standard button interaction and overlay the shared icon. Text remains
/// usable in embedding hosts that have not supplied textures.
pub(crate) fn draw_icon_button(
    ui: &Ui,
    icons: Option<&Icons>,
    key: &str,
    id: &str,
    fallback: &str,
    size: [f32; 2],
) -> bool {
    let size = [
        if size[0] == 0.0 {
            ui.calc_text_size(fallback)[0] + ui.clone_style().frame_padding()[0] * 2.0
        } else {
            size[0]
        },
        size[1],
    ];
    let Some(texture) = icons.and_then(|icons| icons.find(key)) else {
        return ui.button_with_size(format!("{fallback}###{id}"), size);
    };
    let clicked = ui.button_with_size(format!("###{id}"), size);
    let min = ui.item_rect_min();
    let max = ui.item_rect_max();
    let side = ui
        .text_line_height()
        .min(max[0] - min[0])
        .min(max[1] - min[1]);
    let origin = [
        (min[0] + max[0] - side) * 0.5,
        (min[1] + max[1] - side) * 0.5,
    ];
    let mut ink = ui.style_color(StyleColor::Text);
    ink[3] *= ui.clone_style().alpha();
    ui.get_window_draw_list().add_image(
        texture,
        origin,
        [origin[0] + side, origin[1] + side],
        [0.0; 2],
        [1.0; 2],
        ink,
    );
    clicked
}

fn rasterize_svg(bytes: &[u8]) -> io::Result<RgbaImage> {
    let options = resvg::usvg::Options {
        dpi: 96.0,
        ..resvg::usvg::Options::default()
    };
    let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(io::Error::other)?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(32, 32)
        .ok_or_else(|| io::Error::other("Unable to allocate icon pixels"))?;
    let scale = 32.0 / tree.size().width().max(tree.size().height());
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(
        (32.0 - tree.size().width() * scale) * 0.5,
        (32.0 - tree.size().height() * scale) * 0.5,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut pixels = pixmap.data().to_vec();
    // tiny-skia emits premultiplied RGBA; NanoSVG and ImGui textures use straight RGBA.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = pixel[3] as u32;
        for channel in &mut pixel[..3] {
            *channel = (*channel as u32 * 255 + alpha / 2)
                .checked_div(alpha)
                .unwrap_or(*channel as u32)
                .min(255) as u8;
        }
    }
    Ok(RgbaImage {
        width: 32,
        height: 32,
        pixels,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const SOLID: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><rect width="32" height="32" fill="white"/></svg>"#;
    const DOT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><circle cx="16" cy="16" r="3" fill="white"/></svg>"#;

    struct TemporaryDirectory(PathBuf);
    impl TemporaryDirectory {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory =
                std::env::temp_dir().join(format!("bed-icon-test-{}-{nonce}", std::process::id()));
            fs::create_dir_all(&directory).unwrap();
            Self(directory)
        }
    }
    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn plugin_namespaces_keep_identical_local_filenames_independent() {
        let mut icons = Icons::default();
        for (plugin, embedded_svg) in [("example.alpha", SOLID), ("example.beta", DOT)] {
            icons
                .register(
                    plugin,
                    &[IconDefinition {
                        name: "close",
                        filename: "close.svg",
                        embedded_svg: Some(embedded_svg),
                    }],
                    None,
                )
                .unwrap();
        }
        assert_ne!(
            icons.images["example.alpha.close"],
            icons.images["example.beta.close"]
        );
        icons.set_texture("example.alpha.close", TextureId::new(1));
        icons.set_texture("example.beta.close", TextureId::new(2));
        assert_eq!(icons.find("example.alpha.close"), Some(TextureId::new(1)));
        assert_eq!(icons.find("example.beta.close"), Some(TextureId::new(2)));
        assert_eq!(icons.find("close"), None);
    }

    #[test]
    fn directory_replaces_its_filename_and_missing_files_keep_embedded_defaults() {
        let directory = TemporaryDirectory::new();
        fs::write(directory.0.join("close.svg"), DOT).unwrap();
        let mut icons = Icons::default();
        icons
            .register(
                "example.alpha",
                &[
                    IconDefinition {
                        name: "close",
                        filename: "close.svg",
                        embedded_svg: Some(SOLID),
                    },
                    IconDefinition {
                        name: "add",
                        filename: "add.svg",
                        embedded_svg: Some(SOLID),
                    },
                ],
                Some(&directory.0),
            )
            .unwrap();
        let mut embedded = Icons::default();
        embedded
            .register(
                "example.beta",
                &[IconDefinition {
                    name: "close",
                    filename: "close.svg",
                    embedded_svg: Some(SOLID),
                }],
                None,
            )
            .unwrap();
        assert_ne!(
            icons.images["example.alpha.close"],
            embedded.images["example.beta.close"]
        );
        assert_eq!(
            icons.images["example.alpha.add"],
            embedded.images["example.beta.close"]
        );
    }

    #[test]
    fn missing_or_invalid_contributions_do_not_publish_partial_batches() {
        let directory = TemporaryDirectory::new();
        let mut icons = Icons::default();
        let definitions = [
            IconDefinition {
                name: "close",
                filename: "close.svg",
                embedded_svg: Some(SOLID),
            },
            IconDefinition {
                name: "add",
                filename: "add.svg",
                embedded_svg: None,
            },
        ];
        let error = icons
            .register("example.alpha", &definitions, Some(&directory.0))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("example.alpha.add"));
        assert!(icons.images.is_empty());
        fs::write(directory.0.join("add.svg"), "invalid SVG").unwrap();
        assert!(
            icons
                .register("example.alpha", &definitions, Some(&directory.0))
                .is_err()
        );
        assert!(icons.images.is_empty());
        icons
            .register("example.alpha", &definitions[..1], None)
            .unwrap();
        assert!(
            icons
                .register("example.alpha", &definitions[..1], None)
                .is_err()
        );
        assert_eq!(icons.images.len(), 1);
    }

    #[test]
    fn embedded_core_defaults_require_namespaced_texture_keys() {
        let mut icons = Icons::bundled();
        assert_eq!(icons.images.len(), icon_keys().count());
        assert!(icons.images.contains_key("bedtime.core.close"));
        icons.set_texture("bedtime.core.close", TextureId::new(4));
        icons.set_texture("bedtime.core.default", TextureId::new(5));
        assert_eq!(icons.find("close"), None);
        assert_eq!(icons.find("bedtime.core.close"), Some(TextureId::new(4)));
        assert_eq!(icons.get("unknown.plugin.icon"), Some(TextureId::new(5)));
        assert_eq!(icons.find("unknown.plugin.icon"), None);
    }

    #[test]
    fn core_defaults_preserve_host_images_and_reject_invalid_directory_atomically() {
        let directory = TemporaryDirectory::new();
        let mut icons = Icons::default();
        let custom = rasterize_svg(DOT).unwrap();
        icons
            .images
            .insert("bedtime.core.close".into(), custom.clone());
        fs::write(directory.0.join("plus.svg"), "invalid SVG").unwrap();
        assert!(icons.register_core_defaults(Some(&directory.0)).is_err());
        assert_eq!(icons.images.len(), 1);
        icons.register_core_defaults(None).unwrap();
        assert_eq!(icons.images["bedtime.core.close"], custom);
        assert_eq!(icons.images.len(), icon_keys().count());
    }

    #[test]
    fn icon_buttons_keep_clicks_hit_rectangles_and_disabled_theme_tint() {
        use dear_imgui_rs::{Condition, Context, FramePrepareOptions, MouseButton, sys};
        let _lock = crate::IMGUI_TEST_LOCK.lock().unwrap();
        let mut context = Context::create();
        context.set_ini_filename(None::<PathBuf>).unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        let mut icons = Icons::default();
        icons.set_texture("plus", TextureId::new(99));
        for ink in [[0.9, 0.9, 0.9, 1.0], [0.1, 0.1, 0.1, 1.0]] {
            context.style_mut().set_color(StyleColor::Text, ink);
            for disabled in [false, true] {
                let mut clicks = 0;
                let mut ids = BTreeSet::new();
                // Start with fallback text and upload the texture while the
                // button is pressed. Its identity and hit target must persist.
                for (loaded, down) in [(false, false), (false, false), (false, true), (true, false)]
                {
                    context.io_mut().add_mouse_pos_event([35.0, 55.0]);
                    context
                        .io_mut()
                        .add_mouse_button_event(MouseButton::Left, down);
                    context.prepare_frame(FramePrepareOptions::new([240.0, 180.0], 1.0 / 60.0));
                    let ui = context.frame();
                    ui.window("Icon buttons")
                        .position([0.0; 2], Condition::Always)
                        .size([240.0, 180.0], Condition::Always)
                        .build(|| {
                            ui.set_cursor_screen_pos([20.0, 40.0]);
                            let _disabled = ui.begin_disabled_with_cond(disabled);
                            let alpha = ui.clone_style().alpha();
                            clicks += usize::from(crate::components::icon_button(
                                ui,
                                &crate::UiFrameContext::for_ui(ui),
                                loaded.then_some(&icons),
                                "plus",
                                "add",
                                "+",
                                [30.0; 2],
                            ));
                            assert_eq!(ui.item_rect_min(), [20.0, 40.0]);
                            assert_eq!(ui.item_rect_max(), [50.0, 70.0]);
                            // Inspect the rendered ink so the disabled overlay
                            // cannot accidentally remain at full opacity.
                            unsafe {
                                ids.insert((*sys::igGetCurrentContext()).LastItemData.ID);
                                if loaded {
                                    let window =
                                        &*sys::igFindWindowByName(c"Icon buttons".as_ptr());
                                    let draw = &*window.DrawList;
                                    let commands = std::slice::from_raw_parts(
                                        draw.CmdBuffer.Data,
                                        draw.CmdBuffer.Size as usize,
                                    );
                                    let command = commands
                                        .iter()
                                        .find(|command| command.TexRef._TexID == 99)
                                        .unwrap();
                                    let index = *draw.IdxBuffer.Data.add(command.IdxOffset as usize)
                                        as usize;
                                    let vertex = &*draw
                                        .VtxBuffer
                                        .Data
                                        .add(index + command.VtxOffset as usize);
                                    let red = (vertex.col & 0xff) as f32 / 255.0;
                                    let actual_alpha = (vertex.col >> 24) as f32 / 255.0;
                                    assert!((red - ink[0]).abs() < 0.01);
                                    assert!((actual_alpha - alpha).abs() < 0.01);
                                }
                            }
                        });
                    drop(context.render_legacy());
                }
                assert_eq!(ids.len(), 1, "texture upload changed the button ID");
                assert_eq!(clicks, usize::from(!disabled));
            }
        }
    }

    #[test]
    fn filenames_override_extensions_and_extensions_ignore_ascii_case() {
        for (file, key) in [
            ("src/main.cpp", "cpp"),
            ("x/main.CPP", "cpp"),
            ("CMakeLists.txt", "cmake"),
            ("cmake", "cmake"),
            (".clang-format", "llvm"),
            (".clangd", "llvm"),
            ("Dockerfile", "docker"),
            (".gitignore", "git"),
            (".gitmodules", "git"),
            (".env", "config"),
            ("README", "text"),
            ("file.", "default"),
            ("index.html", "html"),
            ("data.json", "json"),
            ("package.json", "npm"),
            ("Cargo.toml", "rust"),
            ("Cargo.lock", "lock"),
            ("Main.java", "code"),
            ("test.JpG", "image"),
            ("db.sqlite3", "database"),
            ("archive.tar.gz", "archive"),
            ("unknown.xyz", "default"),
            ("no-extension", "default"),
        ] {
            assert_eq!(
                icon_key_for_file(file),
                format!("bedtime.core.{key}"),
                "{file}"
            );
        }
    }

    #[test]
    fn textures_fall_back_and_all_file_icons_follow_theme_ink() {
        let mut icons = Icons::default();
        icons.set_texture("bedtime.core.default", TextureId::new(1));
        icons.set_texture("bedtime.core.rust", TextureId::new(2));
        assert_eq!(icons.get_for_file("lib.rs"), Some(TextureId::new(2)));
        assert_eq!(icons.get("missing"), Some(TextureId::new(1)));
        for ink in [[0.9, 0.9, 0.9, 1.0], [0.1, 0.1, 0.1, 1.0]] {
            let provider: &dyn FileIcons = &icons;
            for filename in ["lib.rs", "main.py", "Cargo.lock", "unknown.xyz"] {
                assert_eq!(provider.file_icon_tint(filename, ink), ink);
            }
        }
    }

    #[test]
    fn catalog_and_associations_have_visible_monochrome_artwork() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let icons = Icons::bundled();
        let keys: BTreeSet<_> = icon_keys().collect();
        assert_eq!(keys.len(), icon_keys().count(), "duplicate catalog key");
        let assets: BTreeSet<_> = fs::read_dir(root.join("assets/icons"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
            .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(assets, keys.iter().map(|key| (*key).to_owned()).collect());
        for key in &keys {
            let image = &icons.images[&format!("bedtime.core.{key}")];
            assert_eq!(
                (image.width, image.height, image.pixels.len()),
                (32, 32, 4096)
            );
            assert!(
                image.pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 0),
                "empty: {key}"
            );
            assert!(
                image
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|p| p[..3] == [255; 3]),
                "colored: {key}"
            );
        }
        for key in FILE_NAMES
            .iter()
            .map(|(_, key)| key)
            .chain(FILE_EXTENSIONS.iter().map(|(_, key)| key))
        {
            assert!(
                icons.images.contains_key(*key),
                "missing association: {key}"
            );
        }
        let mut extensions = BTreeSet::new();
        for (aliases, _) in FILE_EXTENSIONS {
            for alias in *aliases {
                assert!(extensions.insert(*alias), "duplicate extension: {alias}");
            }
        }
        for name in [
            "files",
            "filetree",
            "sidebar",
            "terminal",
            "search",
            "structure",
            "diagnostics",
            "debug",
            "git",
            "split_right",
            "split_down",
            "gear",
            "settings",
            "image",
            "hex",
            "other",
        ] {
            assert!(
                icons.images.contains_key(command_icon_key(Some(name))),
                "missing command: {name}"
            );
        }
    }
}
