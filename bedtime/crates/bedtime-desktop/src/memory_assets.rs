//! A shared virtual asset source registered before Bevy builds AssetServer.
//! Optional feature plugins can populate their own namespace afterward.
use bevy::{
    asset::{
        AssetApp, AssetPath,
        io::{
            AssetSourceBuilder,
            memory::{Dir, MemoryAssetReader},
        },
    },
    prelude::*,
};
use std::{
    io,
    path::{Component, Path, PathBuf},
};

pub const MEMORY_ASSET_SOURCE: &str = "bedtime-memory";

#[derive(Resource, Clone)]
pub struct MemoryAssets {
    root: Dir,
}

impl MemoryAssets {
    /// Register once before AssetPlugin. DesktopPlugins installs this service;
    /// headless hosts can register it before adding their own AssetPlugin.
    pub fn register(app: &mut App) {
        if app.world().contains_resource::<Self>() {
            return;
        }
        assert!(
            !app.world().contains_resource::<bevy::asset::AssetServer>(),
            "MemoryAssets must be registered before AssetPlugin; DesktopPlugins provides it automatically"
        );
        let root = Dir::default();
        let reader = MemoryAssetReader { root: root.clone() };
        app.register_asset_source(
            MEMORY_ASSET_SOURCE,
            AssetSourceBuilder::new(move || Box::new(reader.clone())),
        );
        app.insert_resource(Self { root });
    }

    /// Shared directory handle, for workers and loaders that already hold a
    /// validated namespace path. Cloning it does not create another source.
    pub fn directory(&self) -> Dir {
        self.root.clone()
    }

    /// Store bytes under a plugin namespace and return the full virtual path.
    /// Relative paths cannot escape their namespace or address asset labels.
    pub fn write(&self, namespace: &str, relative: &Path, bytes: Vec<u8>) -> io::Result<PathBuf> {
        let namespace_path = Path::new(namespace);
        if namespace_path.components().count() != 1
            || !namespace_path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            || relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
            || relative.to_str().is_none()
            || namespace.contains('#')
            || relative.to_string_lossy().contains('#')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Memory asset paths must remain inside a single plugin namespace",
            ));
        }
        let path = namespace_path.join(relative);
        self.root.insert_asset(&path, bytes);
        Ok(path)
    }

    pub fn asset_path(path: &Path) -> AssetPath<'static> {
        AssetPath::from(path.to_owned()).with_source(MEMORY_ASSET_SOURCE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registering_a_memory_asset_consumer_after_asset_server_builds_fails_clearly() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            MemoryAssets::register(&mut app)
        }));
        assert!(result.is_err());
        assert!(!app.world().contains_resource::<MemoryAssets>());
    }

    #[test]
    fn plugins_can_write_independent_namespaces_after_asset_server_builds() {
        let mut app = App::new();
        MemoryAssets::register(&mut app);
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        let assets = app.world().resource::<MemoryAssets>();
        let first = assets
            .write("first", Path::new("same.bin"), vec![1])
            .unwrap();
        let second = assets
            .write("second", Path::new("same.bin"), vec![2])
            .unwrap();
        assert_ne!(first, second);
        assert!(assets.directory().remove_asset(&first).is_some());
        assert!(assets.directory().remove_asset(&second).is_some());
        assert!(
            assets
                .write("first", Path::new("../outside.bin"), vec![])
                .is_err()
        );
        assert!(
            assets
                .write("first/second", Path::new("inside.bin"), vec![])
                .is_err()
        );
    }
}
