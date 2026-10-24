//! A dedicated source for validated model snapshots. Register before AssetPlugin.
use bevy::{
    asset::{
        AssetApp,
        io::{
            AssetSourceBuilder,
            memory::{Dir, MemoryAssetReader},
        },
    },
    prelude::*,
};
pub(crate) const MEMORY_ASSET_SOURCE: &str = "bedtime-model2";
#[derive(Resource, Clone)]
pub(crate) struct MemoryAssets(Dir);
impl MemoryAssets {
    pub fn register(app: &mut App) {
        if app.world().contains_resource::<Self>() {
            return;
        }
        assert!(
            !app.world().contains_resource::<AssetServer>(),
            "Register the model asset source before AssetPlugin"
        );
        let root = Dir::default();
        let reader = MemoryAssetReader { root: root.clone() };
        app.register_asset_source(
            MEMORY_ASSET_SOURCE,
            AssetSourceBuilder::new(move || Box::new(reader.clone())),
        );
        app.insert_resource(Self(root));
    }
    pub fn directory(&self) -> Dir {
        self.0.clone()
    }
}
