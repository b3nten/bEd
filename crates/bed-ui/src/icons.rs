//! Translated from ned util/icons.{h,cpp}; see LICENSE and NOTICE.
//! CPU rasterization is independent of GPU upload; the host supplies texture IDs.
use crate::presentation::FileIcons;
use dear_imgui_rs::TextureId;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

pub const ICON_FILES: &[&str] = &[
    "tf.svg",
    "terminal.svg",
    "edit-hover.svg",
    "edit.svg",
    "terminal-hover.svg",
    "cshtml.svg",
    "green-dot.svg",
    "rs.svg",
    "cmake.svg",
    "maximize-mac.svg",
    "close-mac.svg",
    "minimize-mac.svg",
    "maximize-mac-hover.svg",
    "close-mac-hover.svg",
    "minimize-mac-hover.svg",
    "clangd.svg",
    "ini.svg",
    "clang-format.svg",
    "rb.svg",
    "Dockerfile.svg",
    "sh.svg",
    "cs.svg",
    "csproj.svg",
    "md.svg",
    "tsx.svg",
    "jsx.svg",
    "ts.svg",
    "brain.svg",
    "close.svg",
    "gear.svg",
    "gear-hover.svg",
    "py.svg",
    "h.svg",
    "hpp.svg",
    "gitignore.svg",
    "gitmodule.svg",
    "js.svg",
    "R.svg",
    "apple.svg",
    "argdown.svg",
    "asm.svg",
    "audio.svg",
    "babel.svg",
    "bazel.svg",
    "bicep.svg",
    "bower.svg",
    "bsl.svg",
    "c-sharp.svg",
    "c.svg",
    "cake.svg",
    "cake_php.svg",
    "checkbox-unchecked.svg",
    "checkbox.svg",
    "cjsx.svg",
    "clock.svg",
    "clojure.svg",
    "code-climate.svg",
    "code-search.svg",
    "coffee.svg",
    "coffee_erb.svg",
    "coldfusion.svg",
    "config.svg",
    "cpp.svg",
    "crystal.svg",
    "crystal_embedded.svg",
    "css.svg",
    "csv.svg",
    "cu.svg",
    "d.svg",
    "dart.svg",
    "db.svg",
    "default.svg",
    "deprecation-cop.svg",
    "docker.svg",
    "editorconfig.svg",
    "ejs.svg",
    "elixir.svg",
    "elixir_script.svg",
    "elm.svg",
    "error.svg",
    "eslint.svg",
    "ethereum.svg",
    "f-sharp.svg",
    "favicon.svg",
    "firebase.svg",
    "firefox.svg",
    "font.svg",
    "git.svg",
    "git_folder.svg",
    "folder.svg",
    "folder-open.svg",
    "git_ignore.svg",
    "github.svg",
    "gitlab.svg",
    "go.svg",
    "go2.svg",
    "godot.svg",
    "gradle.svg",
    "grails.svg",
    "graphql.svg",
    "grunt.svg",
    "gulp.svg",
    "hacklang.svg",
    "haml.svg",
    "happenings.svg",
    "haskell.svg",
    "haxe.svg",
    "heroku.svg",
    "hex.svg",
    "html.svg",
    "html_erb.svg",
    "ignored.svg",
    "illustrator.svg",
    "image.svg",
    "info.svg",
    "ionic.svg",
    "jade.svg",
    "java.svg",
    "javascript.svg",
    "jenkins.svg",
    "jinja.svg",
    "js_erb.svg",
    "json.svg",
    "julia.svg",
    "karma.svg",
    "kotlin.svg",
    "less.svg",
    "license.svg",
    "liquid.svg",
    "livescript.svg",
    "lock.svg",
    "lua.svg",
    "makefile.svg",
    "markdown.svg",
    "maven.svg",
    "mdo.svg",
    "mustache.svg",
    "new-file.svg",
    "nim.svg",
    "notebook.svg",
    "npm.svg",
    "npm_ignored.svg",
    "nunjucks.svg",
    "ocaml.svg",
    "odata.svg",
    "pddl.svg",
    "pdf.svg",
    "perl.svg",
    "photoshop.svg",
    "php.svg",
    "pipeline.svg",
    "plan.svg",
    "platformio.svg",
    "powershell.svg",
    "prisma.svg",
    "project.svg",
    "prolog.svg",
    "pug.svg",
    "puppet.svg",
    "purescript.svg",
    "python.svg",
    "rails.svg",
    "react.svg",
    "reasonml.svg",
    "rescript.svg",
    "rollup.svg",
    "ruby.svg",
    "rust.svg",
    "salesforce.svg",
    "sass.svg",
    "sbt.svg",
    "scala.svg",
    "search.svg",
    "settings.svg",
    "shell.svg",
    "slim.svg",
    "smarty.svg",
    "spring.svg",
    "stylelint.svg",
    "stylus.svg",
    "sublime.svg",
    "svelte.svg",
    "svg.svg",
    "png.svg",
    "swift.svg",
    "terraform.svg",
    "tex.svg",
    "time-cop.svg",
    "todo.svg",
    "tsconfig.svg",
    "twig.svg",
    "typescript.svg",
    "vala.svg",
    "video.svg",
    "vue.svg",
    "wasm.svg",
    "wat.svg",
    "webpack.svg",
    "wgt.svg",
    "windows.svg",
    "word.svg",
    "xls.svg",
    "xml.svg",
    "yarn.svg",
    "yml.svg",
    "zig.svg",
    "zip.svg",
];

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
    pub fn load(resources_root: &Path) -> Self {
        let mut icons = Self::default();
        for file in ICON_FILES {
            let Some(path) = resolve_icon_path(resources_root, file) else {
                continue;
            };
            match load_svg(&path) {
                Ok(mut image) => {
                    let key = file.split('.').next().unwrap_or(file);
                    if monochrome_icon(key) {
                        for pixel in image.pixels.as_chunks_mut::<4>().0 {
                            pixel[..3].fill(255);
                        }
                    }
                    icons.images.insert(key.to_owned(), image);
                }
                Err(error) => eprintln!("Error loading SVG {}: {error}", path.display()),
            }
        }
        icons
            .images
            .entry("default".to_owned())
            .or_insert_with(|| RgbaImage {
                width: 2,
                height: 1,
                pixels: vec![255, 255, 255, 255, 0, 0, 0, 255],
            });
        icons
    }
    pub fn set_texture(&mut self, key: &str, texture: TextureId) {
        self.textures.insert(key.to_owned(), texture);
    }
    pub fn get(&self, name: &str) -> Option<TextureId> {
        self.textures
            .get(name)
            .or_else(|| self.textures.get("default"))
            .copied()
    }
    pub fn get_for_file(&self, filename: &str) -> Option<TextureId> {
        self.get(icon_key_for_file(filename))
    }
    pub fn file_icon_tint(&self, filename: &str, text: [f32; 4]) -> [f32; 4] {
        if self.get_for_file(filename) == self.get("default") {
            text
        } else {
            [1.0; 4]
        }
    }
}
fn monochrome_icon(key: &str) -> bool {
    matches!(
        key,
        "default"
            | "terminal"
            | "terminal-hover"
            | "edit"
            | "edit-hover"
            | "brain"
            | "close"
            | "gear"
            | "gear-hover"
            | "folder"
            | "folder-open"
            | "search"
            | "settings"
            | "code-search"
            | "new-file"
    )
}
impl FileIcons for Icons {
    fn get(&self, name: &str) -> Option<TextureId> {
        self.get(name)
    }
    fn get_for_file(&self, filename: &str) -> Option<TextureId> {
        self.get_for_file(filename)
    }
}
pub fn icon_key_for_file(filename: &str) -> &str {
    let path = Path::new(filename);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    match name {
        "CMakeLists.txt" | "cmake" => "cmake",
        ".clangd" | ".clang-format" => "clangd",
        "Dockerfile" => "Dockerfile",
        ".gitignore" => "gitignore",
        ".gitmodules" => "gitmodule",
        _ => path
            .extension()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("default"),
    }
}
pub fn resolve_icon_path(resources_root: &Path, filename: &str) -> Option<PathBuf> {
    let mut roots = vec![
        resources_root.join("resources/icons"),
        PathBuf::from("resources/icons"),
    ];
    if !cfg!(target_os = "macos") {
        roots.push(PathBuf::from("/usr/share/Bed/resources/icons"));
    }
    roots
        .into_iter()
        .map(|dir| dir.join(filename))
        .find(|path| path.exists())
}
pub fn load_svg(path: &Path) -> io::Result<RgbaImage> {
    let bytes = fs::read(path)?;
    let options = resvg::usvg::Options {
        dpi: 96.0,
        ..resvg::usvg::Options::default()
    };
    let tree = resvg::usvg::Tree::from_data(&bytes, &options).map_err(io::Error::other)?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(32, 32)
        .ok_or_else(|| io::Error::other("Unable to allocate icon pixels"))?;
    let scale = 32.0 / tree.size().width();
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
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
    #[test]
    fn exact_special_names_and_case_sensitive_extensions_match_upstream() {
        for (file, key) in [
            ("src/main.cpp", "cpp"),
            ("x/main.CPP", "CPP"),
            ("CMakeLists.txt", "cmake"),
            ("cmake", "cmake"),
            (".clang-format", "clangd"),
            (".clangd", "clangd"),
            ("Dockerfile", "Dockerfile"),
            (".gitignore", "gitignore"),
            (".gitmodules", "gitmodule"),
            (".env", "default"),
            ("README", "default"),
            ("file.", "default"),
        ] {
            assert_eq!(icon_key_for_file(file), key);
        }
    }
    #[test]
    fn uploaded_textures_use_default_for_unknown_keys() {
        let mut icons = Icons::default();
        icons.set_texture("default", TextureId::new(1));
        icons.set_texture("rs", TextureId::new(2));
        assert_eq!(icons.get_for_file("lib.rs"), Some(TextureId::new(2)));
        assert_eq!(icons.get("missing"), Some(TextureId::new(1)));
    }
    #[test]
    fn bundled_icons_rasterize() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let icons = Icons::load(&root);
        assert_eq!(icons.images.len(), 194);
        assert!(icons.images.contains_key("default"));
        assert!(icons.images.contains_key("folder-open"));
        for image in icons.images.values() {
            assert_eq!(
                (image.width, image.height, image.pixels.len()),
                (32, 32, 4096)
            );
            assert!(image.pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 0));
        }
        for key in [
            "close",
            "gear",
            "folder",
            "folder-open",
            "search",
            "default",
        ] {
            assert!(
                icons.images[key]
                    .pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|pixel| pixel[..3] == [255; 3])
            );
        }
        assert_eq!(
            icons.images["rs"],
            load_svg(&root.join("resources/icons/rs.svg")).unwrap()
        );
    }
}
