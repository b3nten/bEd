//! Artwork is a Bevy asset; surfaces select packs independently of color themes.
use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    prelude::*,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io, sync::Arc};

macro_rules! icon_catalog {
    ($($variant:ident => $key:literal,)*) => {
        /// The fixed bundled vocabulary. Custom artwork can also be drawn by handle.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub enum BuiltinIcon {
            $(#[serde(rename = $key)] $variant,)*
        }
        impl BuiltinIcon {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];
            pub const fn key(self) -> &'static str {
                match self { $(Self::$variant => $key,)* }
            }
            fn svg(self) -> &'static [u8] {
                match self {
                    $(Self::$variant => include_bytes!(concat!("../assets/icons/", $key, ".svg")),)*
                }
            }
        }
    }
}
include!("builtin_icons.rs");

/// Validated monochrome SVG artwork. Clones share the parsed immutable tree.
#[derive(Asset, TypePath, Clone)]
pub struct Icon {
    pub(crate) tree: Arc<resvg::usvg::Tree>,
}

impl Icon {
    pub fn from_svg(bytes: &[u8]) -> io::Result<Self> {
        let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Self {
            tree: Arc::new(tree),
        })
    }

    pub(crate) fn rasterize(&self, pixels: u32) -> io::Result<Vec<u8>> {
        let mut image = resvg::tiny_skia::Pixmap::new(pixels, pixels)
            .ok_or_else(|| io::Error::other("Unable to allocate icon pixels"))?;
        let size = self.tree.size();
        let scale = pixels as f32 / size.width().max(size.height());
        let transform = resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(
            (pixels as f32 - size.width() * scale) * 0.5,
            (pixels as f32 - size.height() * scale) * 0.5,
        );
        resvg::render(&self.tree, transform, &mut image.as_mut());
        // Alpha is the silhouette. White RGB allows theme tinting at draw time,
        // and avoids treating tiny-skia's premultiplied RGB as straight alpha.
        for pixel in image.data_mut().chunks_exact_mut(4) {
            pixel[..3].fill(255);
        }
        Ok(image.take())
    }
}

/// A partial replacement pack. Omitted slots inherit the bundled artwork.
/// Handles retain dependencies; replacing the asset can hot-swap a pack in place.
#[derive(Asset, TypePath, Clone, Default)]
pub struct IconSet {
    #[dependency]
    pub overrides: BTreeMap<BuiltinIcon, Handle<Icon>>,
}

/// Requested pack for a UI surface. Removing this selects the bundled set.
#[derive(Component, Clone, Debug)]
pub struct UiIcons(pub Handle<IconSet>);

/// An object's icon reference, also accepted directly by immediate-mode controls.
#[derive(Component, Clone, Debug)]
pub enum IconSource {
    Builtin(BuiltinIcon),
    Asset(Handle<Icon>),
}

impl From<BuiltinIcon> for IconSource {
    fn from(icon: BuiltinIcon) -> Self {
        Self::Builtin(icon)
    }
}

impl From<Handle<Icon>> for IconSource {
    fn from(icon: Handle<Icon>) -> Self {
        Self::Asset(icon)
    }
}

impl From<&Handle<Icon>> for IconSource {
    fn from(icon: &Handle<Icon>) -> Self {
        Self::Asset(icon.clone())
    }
}

impl From<&IconSource> for IconSource {
    fn from(icon: &IconSource) -> Self {
        icon.clone()
    }
}

/// Actual presentation state. A pending or failed replacement preserves `active`.
#[derive(Component, Default, Clone, Debug)]
pub struct UiIconStatus {
    /// `None` denotes the bundled set.
    pub active: Option<Handle<IconSet>>,
    pub pending: bool,
    pub error: Option<String>,
}

#[derive(Resource)]
pub(crate) struct BundledIcons(pub BTreeMap<BuiltinIcon, Handle<Icon>>);

pub(crate) fn install(app: &mut App) {
    app.init_asset::<Icon>()
        .init_asset::<IconSet>()
        .init_asset_loader::<IconLoader>()
        .init_asset_loader::<IconSetLoader>();
    let mut assets = app.world_mut().resource_mut::<Assets<Icon>>();
    let bundled = BuiltinIcon::ALL
        .iter()
        .map(|&slot| {
            let icon = Icon::from_svg(slot.svg()).expect("Bundled SVG must be valid");
            (slot, assets.add(icon))
        })
        .collect();
    app.insert_resource(BundledIcons(bundled));
}

#[derive(Default, TypePath)]
struct IconLoader;
impl AssetLoader for IconLoader {
    type Asset = Icon;
    type Settings = ();
    type Error = io::Error;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> io::Result<Icon> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Icon::from_svg(&bytes)
    }
    fn extensions(&self) -> &[&str] {
        &["svg"]
    }
}

#[derive(Default, TypePath)]
struct IconSetLoader;
impl AssetLoader for IconSetLoader {
    type Asset = IconSet;
    type Settings = ();
    type Error = io::Error;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        context: &mut LoadContext<'_>,
    ) -> io::Result<IconSet> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let paths: BTreeMap<BuiltinIcon, String> = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut overrides = BTreeMap::new();
        for (slot, path) in paths {
            let path = context.path().resolve_embed(&path.into());
            overrides.insert(slot, context.load(path));
        }
        Ok(IconSet { overrides })
    }
    fn extensions(&self) -> &[&str] {
        &["icons.json"]
    }
}
