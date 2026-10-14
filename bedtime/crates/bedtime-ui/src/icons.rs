//! Shared monochrome icon vocabulary. Applications replace packs; plugins consume keys.
//! The desktop owns native/GPU realization and texture lifetimes.
use crate::presentation::FileIcons;
use dear_imgui_rs::{StyleColor, TextureId, Ui};
use std::{collections::BTreeMap, fs, io, path::Path, sync::OnceLock};

#[path = "bundled_icons.rs"]
mod bundled_icons;
#[cfg(test)]
const ICON_CATALOG: &str = include_str!("../assets/icons/catalog.tsv");

/// SVG overrides keyed by the shared vocabulary in assets/icons/catalog.tsv.
/// An empty pack restores defaults; omitted keys always use embedded artwork.
#[derive(Clone, Debug, Default)]
pub struct IconPack(pub BTreeMap<String, Vec<u8>>);

impl IconPack {
    /// Read <key>.svg files from a flat directory. Non-SVG files are ignored so
    /// packs can include attribution. replace_pack validates keys and artwork.
    pub fn from_directory(directory: &Path) -> io::Result<Self> {
        let mut overrides = BTreeMap::new();
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "svg") {
                continue;
            }
            let key = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Icon filename must be UTF-8")
                })?;
            overrides.insert(key.to_owned(), fs::read(&path)?);
        }
        Ok(Self(overrides))
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

pub struct Icons {
    images: BTreeMap<String, RgbaImage>,
    textures: BTreeMap<String, TextureId>,
}

fn defaults() -> &'static BTreeMap<String, RgbaImage> {
    static DEFAULTS: OnceLock<BTreeMap<String, RgbaImage>> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        bundled_icons::BUNDLED_ICONS
            .iter()
            .map(|(key, svg)| {
                let mut image = rasterize_svg(svg).expect("embedded icon artwork is valid");
                monochrome(&mut image);
                ((*key).to_owned(), image)
            })
            .collect()
    })
}

impl Default for Icons {
    fn default() -> Self {
        Self {
            images: defaults().clone(),
            textures: BTreeMap::new(),
        }
    }
}

impl Icons {
    pub fn bundled() -> Self {
        Self::default()
    }

    /// Replace the active pack atomically, including at runtime. Resolve against
    /// defaults rather than the previous pack. Failure preserves live artwork and
    /// texture IDs. The desktop reconciles changed images before the next draw;
    /// consumers must look up keys each frame rather than retain texture IDs.
    pub fn replace_pack(&mut self, pack: &IconPack) -> io::Result<()> {
        let mut images = defaults().clone();
        for (key, svg) in &pack.0 {
            if !defaults().contains_key(key) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Unknown icon key: {key}"),
                ));
            }
            let mut image = rasterize_svg(svg)
                .map_err(|error| io::Error::new(error.kind(), format!("Icon {key}: {error}")))?;
            monochrome(&mut image);
            images.insert(key.clone(), image);
        }
        self.images = images;
        Ok(())
    }

    pub fn images(&self) -> &BTreeMap<String, RgbaImage> {
        &self.images
    }

    /// Desktop realization API; plugins only read texture IDs.
    pub fn set_texture(&mut self, key: &str, texture: TextureId) {
        self.textures.insert(key.to_owned(), texture);
    }

    pub fn clear_textures(&mut self) {
        self.textures.clear();
    }

    /// Exact shared-key lookup. Missing control icons are not generic file icons.
    pub fn get(&self, name: &str) -> Option<TextureId> {
        self.find(name)
    }

    pub fn find(&self, name: &str) -> Option<TextureId> {
        self.textures.get(name).copied()
    }

    pub fn get_for_file(&self, filename: &str) -> Option<TextureId> {
        self.find(icon_key_for_file(filename))
            .or_else(|| self.find("default"))
    }

    pub fn file_icon_tint(&self, _filename: &str, text: [f32; 4]) -> [f32; 4] {
        text
    }
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
    ("CMakeLists.txt", "cmake"),
    ("cmake", "cmake"),
    (".clangd", "llvm"),
    (".clang-format", "llvm"),
    ("Dockerfile", "docker"),
    ("Containerfile", "docker"),
    (".dockerignore", "docker"),
    ("docker-compose.yml", "docker"),
    ("docker-compose.yaml", "docker"),
    (".gitignore", "git"),
    (".gitmodules", "git"),
    (".gitattributes", "git"),
    (".editorconfig", "editorconfig"),
    (".env", "config"),
    ("Cargo.toml", "rust"),
    ("Cargo.lock", "lock"),
    ("package.json", "npm"),
    ("package-lock.json", "lock"),
    ("yarn.lock", "lock"),
    ("tsconfig.json", "typescript"),
    (".babelrc", "babel"),
    (".eslintrc", "eslint"),
    ("Makefile", "config"),
    ("makefile", "config"),
    ("GNUmakefile", "config"),
    ("LICENSE", "license"),
    ("LICENSE.txt", "license"),
    ("LICENSE.md", "license"),
    ("COPYING", "license"),
    ("README", "text"),
    ("NOTICE", "text"),
];

// One canonical asset per language/tool or generic file category.
const FILE_EXTENSIONS: &[(&[&str], &str)] = &[
    (&["rs", "rust"], "rust"),
    (&["py", "pyi", "pyw", "python"], "python"),
    (&["js", "mjs", "cjs", "javascript"], "javascript"),
    (
        &["ts", "mts", "cts", "typescript", "tsconfig"],
        "typescript",
    ),
    (&["jsx", "tsx", "cjsx", "react"], "react"),
    (&["rb", "ruby"], "ruby"),
    (&["cmake"], "cmake"),
    (&["clangd", "clang-format"], "llvm"),
    (&["docker", "dockerfile"], "docker"),
    (&["sh", "bash", "zsh", "fish", "shell"], "bash"),
    (&["c", "h"], "c"),
    (&["cpp", "cc", "cxx", "hpp", "hh", "hxx"], "cpp"),
    (
        &["cs", "csproj", "c-sharp", "vb", "vbproj", "fsproj"],
        "dotnet",
    ),
    (&["fs", "fsi", "fsx", "f-sharp"], "fsharp"),
    (&["r", "rmd"], "r-language"),
    (&["html", "htm", "xhtml", "html_erb", "cshtml"], "html"),
    (&["css"], "css"),
    (&["md", "markdown", "mdx", "mdo"], "markdown"),
    (&["json", "jsonc", "json5"], "json"),
    (&["yml", "yaml"], "yaml"),
    (
        &["git", "gitignore", "gitmodule", "git_ignore", "git_folder"],
        "git",
    ),
    (&["github"], "github"),
    (&["gitlab"], "gitlab"),
    (&["go", "go2", "mod", "sum"], "go"),
    (&["gd", "godot", "tscn", "tres"], "godot"),
    (&["dart"], "dart"),
    (&["ex", "exs", "elixir", "elixir_script"], "elixir"),
    (&["elm"], "elm"),
    (&["eslint"], "eslint"),
    (&["ethereum", "sol"], "ethereum"),
    (&["firebase"], "firebase"),
    (&["firefox"], "firefox"),
    (&["gradle"], "gradle"),
    (&["graphql", "gql"], "graphql"),
    (&["grunt"], "grunt"),
    (&["gulp"], "gulp"),
    (&["hs", "lhs", "haskell"], "haskell"),
    (&["hx", "haxe"], "haxe"),
    (&["ionic"], "ionic"),
    (&["jenkins"], "jenkins"),
    (&["jinja", "jinja2", "j2"], "jinja"),
    (&["jl", "julia"], "julia"),
    (&["kt", "kts", "kotlin"], "kotlin"),
    (&["less"], "less"),
    (&["lua"], "lua"),
    (&["maven"], "maven"),
    (&["nim", "nims"], "nim"),
    (&["npm", "npm_ignored"], "npm"),
    (&["nunjucks", "njk"], "nunjucks"),
    (&["ml", "mli", "ocaml"], "ocaml"),
    (&["pl", "pm", "perl"], "perl"),
    (&["php", "phtml", "cake_php"], "php"),
    (&["platformio"], "platformio"),
    (&["prisma"], "prisma"),
    (&["pug", "jade"], "pug"),
    (&["pp", "puppet"], "puppet"),
    (&["purs", "purescript"], "purescript"),
    (&["re", "rei", "reasonml"], "reason"),
    (&["res", "resi", "rescript"], "rescript"),
    (&["rollup"], "rollup"),
    (&["rails", "erb"], "rails"),
    (&["sass", "scss"], "code"),
    (&["scala", "sc"], "scala"),
    (&["spring"], "spring"),
    (&["stylelint"], "stylelint"),
    (&["styl", "stylus"], "stylus"),
    (
        &["sublime", "sublime-project", "sublime-workspace"],
        "sublime",
    ),
    (&["svelte"], "svelte"),
    (&["swift"], "swift"),
    (&["tf", "tfvars", "terraform"], "terraform"),
    (&["tex", "latex", "bib"], "tex"),
    (&["vala", "vapi"], "code"),
    (&["vue"], "code"),
    (&["wasm", "wat"], "wasm"),
    (&["webpack"], "code"),
    (&["yarn"], "yarn"),
    (&["zig", "zon"], "zig"),
    (&["babel"], "babel"),
    (&["bazel", "bzl"], "bazel"),
    (&["bower"], "bower"),
    (&["coffee", "coffee_erb", "cake"], "coffee"),
    (&["cr", "crystal", "crystal_embedded"], "crystal"),
    (&["editorconfig"], "editorconfig"),
    (&["groovy", "grails"], "groovy"),
    (&["clj", "cljs", "cljc", "edn", "clojure"], "clojure"),
    (&["ejs"], "ejs"),
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
        "config",
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
        "code",
    ),
    (
        &[
            "txt", "text", "log", "rst", "rtf", "doc", "docx", "word", "pdf", "argdown",
        ],
        "text",
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
        "image",
    ),
    (
        &[
            "mp3", "wav", "flac", "ogg", "m4a", "aac", "aiff", "opus", "audio",
        ],
        "audio",
    ),
    (
        &["mp4", "mov", "mkv", "avi", "webm", "m4v", "video"],
        "video",
    ),
    (
        &["zip", "tar", "gz", "bz2", "xz", "7z", "rar", "zst"],
        "archive",
    ),
    (&["csv", "tsv", "xls", "xlsx", "ods"], "spreadsheet"),
    (&["ttf", "otf", "woff", "woff2", "font"], "font"),
    (&["db", "sqlite", "sqlite3", "sql"], "database"),
    (&["lock"], "lock"),
    (&["ipynb", "notebook"], "notebook"),
    (&["license"], "license"),
    (&["bin", "hex"], "hex"),
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
        .map_or("default", |(_, key)| key)
}

/// Map existing command identifiers to the artwork used in panels.
pub fn command_icon_key(name: Option<&str>) -> &'static str {
    match name {
        Some("files" | "filetree" | "sidebar") => "files",
        Some("terminal") => "terminal",
        Some("search") => "search",
        Some("structure") => "structure",
        Some("diagnostics") => "diagnostics",
        Some("debug") => "debug",
        Some("git") => "git-branch",
        Some("split_right") => "split_right",
        Some("split_down") => "split_down",
        Some("gear" | "settings") => "settings",
        Some("image") => "viewer-image",
        Some("hex") => "hex",
        _ => "extension",
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
    use std::{collections::BTreeSet, path::PathBuf};

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
    fn packs_replace_defaults_without_inheriting_previous_overrides() {
        let mut icons = Icons::default();
        let baseline = icons.images.clone();
        icons.set_texture("close", TextureId::new(4));
        icons
            .replace_pack(&IconPack(BTreeMap::from([
                ("close".into(), SOLID.to_vec()),
                ("plus".into(), DOT.to_vec()),
            ])))
            .unwrap();
        assert_ne!(icons.images["close"], baseline["close"]);
        assert_eq!(icons.images["folder"], baseline["folder"]);
        icons
            .replace_pack(&IconPack(BTreeMap::from([("close".into(), DOT.to_vec())])))
            .unwrap();
        assert_eq!(icons.images["plus"], baseline["plus"]);
        assert_eq!(icons.find("close"), Some(TextureId::new(4)));
        icons.replace_pack(&IconPack::default()).unwrap();
        assert_eq!(icons.images, baseline);
    }

    #[test]
    fn invalid_packs_preserve_artwork_and_live_texture_ids() {
        let mut icons = Icons::default();
        icons
            .replace_pack(&IconPack(BTreeMap::from([("close".into(), DOT.to_vec())])))
            .unwrap();
        icons.set_texture("close", TextureId::new(4));
        let before = icons.images.clone();
        for invalid in ["unknown", "../close", "bedtime.core.close", "plus"] {
            let pack = IconPack(BTreeMap::from([
                ("close".into(), SOLID.to_vec()),
                (invalid.into(), b"invalid SVG".to_vec()),
            ]));
            assert!(icons.replace_pack(&pack).is_err());
            assert_eq!(icons.images, before);
            assert_eq!(icons.find("close"), Some(TextureId::new(4)));
        }
    }

    #[test]
    fn directory_pack_overrides_shared_keys_and_preserves_missing_defaults() {
        let directory = TemporaryDirectory::new();
        fs::write(directory.0.join("close.svg"), DOT).unwrap();
        fs::write(directory.0.join("ATTRIBUTION.txt"), "Artwork attribution").unwrap();
        let pack = IconPack::from_directory(&directory.0).unwrap();
        let mut icons = Icons::default();
        let baseline = icons.images.clone();
        icons.replace_pack(&pack).unwrap();
        assert_ne!(icons.images["close"], baseline["close"]);
        assert_eq!(icons.images["folder"], baseline["folder"]);
        fs::write(directory.0.join("plus.svg"), "invalid SVG").unwrap();
        let before = icons.images.clone();
        assert!(
            icons
                .replace_pack(&IconPack::from_directory(&directory.0).unwrap())
                .is_err()
        );
        assert_eq!(icons.images, before);
    }

    #[test]
    fn full_pack_can_replace_every_default_and_control_lookup_is_exact() {
        let mut icons = Icons::default();
        let pack = IconPack(
            icon_keys()
                .map(|key| (key.to_owned(), DOT.to_vec()))
                .collect(),
        );
        icons.replace_pack(&pack).unwrap();
        assert_eq!(icons.images.len(), icon_keys().count());
        assert!(
            icons
                .images
                .values()
                .all(|image| image == &icons.images["close"])
        );
        icons.set_texture("close", TextureId::new(4));
        icons.set_texture("default", TextureId::new(5));
        assert_eq!(icons.get("close"), Some(TextureId::new(4)));
        assert_eq!(icons.get("missing"), None);
        assert_eq!(icons.find("bedtime.core.close"), None);
        assert_eq!(icons.get_for_file("unknown.xyz"), Some(TextureId::new(5)));
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
            assert_eq!(icon_key_for_file(file), key, "{file}");
        }
    }

    #[test]
    fn textures_fall_back_and_all_file_icons_follow_theme_ink() {
        let mut icons = Icons::default();
        icons.set_texture("default", TextureId::new(1));
        icons.set_texture("rust", TextureId::new(2));
        assert_eq!(icons.get_for_file("lib.rs"), Some(TextureId::new(2)));
        assert_eq!(
            icons.get_for_file("missing.rs-without-icon"),
            Some(TextureId::new(1))
        );
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
            let image = &icons.images[*key];
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
