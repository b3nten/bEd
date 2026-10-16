//! Context-local raster cache. Managed texture retirement protects queued snapshots.
use crate::icons::*;
use bevy::{
    asset::{AssetId, LoadState, RecursiveDependencyLoadState, UntypedAssetId},
    prelude::*,
};
use egui::{Context, TextureHandle, TextureId};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

type ResolvedSet = BTreeMap<BuiltinIcon, (AssetId<Icon>, Icon)>;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Identity {
    Builtin(BuiltinIcon),
    Asset(AssetId<Icon>),
}

impl From<&IconSource> for Identity {
    fn from(source: &IconSource) -> Self {
        match source {
            IconSource::Builtin(slot) => Self::Builtin(*slot),
            IconSource::Asset(handle) => Self::Asset(handle.id()),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct RasterKey {
    asset: AssetId<Icon>,
    // A parsed SVG is immutable. Its Arc identity distinguishes artwork revisions;
    // the entry retains that Arc so its address cannot be reused while cached.
    artwork: usize,
    pixels: u32,
}

impl RasterKey {
    fn new(asset: AssetId<Icon>, icon: &Icon, pixels: u32) -> Self {
        Self {
            asset,
            artwork: Arc::as_ptr(&icon.tree) as usize,
            pixels,
        }
    }
}

struct Raster {
    _artwork: Icon,
    texture: TextureHandle,
    last_used: u64,
}

#[derive(Default)]
pub(crate) struct IconCache {
    resolved: ResolvedSet,
    sources: HashMap<AssetId<Icon>, Icon>,
    rasters: HashMap<RasterKey, Raster>,
    requests: HashSet<(Identity, u32)>,
    frame: u64,
    common_pixels: u32,
    pub status: UiIconStatus,
    pub rasterizations: u64,
    pub needs_redraw: bool,
}

impl IconCache {
    pub fn clear(&mut self) {
        for (_, raster) in self.rasters.drain() {
            drop(raster);
        }
        *self = Self::default();
    }

    pub fn prepare(
        &mut self,
        context: &mut Context,
        icons: &Assets<Icon>,
        sets: &Assets<IconSet>,
        bundled: &BundledIcons,
        requested: Option<&UiIcons>,
        server: &AssetServer,
        common_pixels: u32,
    ) -> std::io::Result<()> {
        self.frame += 1;
        self.common_pixels = common_pixels;
        self.needs_redraw = false;
        self.sources = icons.iter().map(|(id, icon)| (id, icon.clone())).collect();
        let request = requested.map(|icons| icons.0.clone());
        let mut candidate = None;
        self.status.pending = false;
        self.status.error = None;
        let replacement = request.as_ref().map(|handle| {
            if let Some(error) = load_error(server, handle.id().into()) {
                self.status.error = Some(error);
                return None;
            }
            if matches!(server.get_load_state(handle.id()), Some(LoadState::Loading))
                || matches!(
                    server.get_recursive_dependency_load_state(handle.id()),
                    Some(RecursiveDependencyLoadState::Loading)
                )
            {
                self.status.pending = true;
                return None;
            }
            let Some(set) = sets.get(handle) else {
                self.status.pending = true;
                return None;
            };
            for handle in set.overrides.values() {
                if let Some(error) = load_error(server, handle.id().into()) {
                    self.status.pending = false;
                    self.status.error = Some(error);
                    return None;
                }
                if icons.get(handle).is_none()
                    || matches!(server.get_load_state(handle.id()), Some(LoadState::Loading))
                {
                    self.status.pending = true;
                }
            }
            if self.status.pending {
                return None;
            }
            Some(set)
        });
        if request.is_none() || replacement.is_some_and(|set| set.is_some()) {
            let overrides = replacement.flatten().map(|set| &set.overrides);
            candidate = Some(
                bundled
                    .0
                    .iter()
                    .map(|(&slot, default)| {
                        let handle = overrides.and_then(|set| set.get(&slot)).unwrap_or(default);
                        let icon = icons
                            .get(handle)
                            .expect("Resolved icon dependency must exist");
                        (slot, (handle.id(), icon.clone()))
                    })
                    .collect::<ResolvedSet>(),
            );
        }
        // Always have bundled artwork while the first requested pack loads.
        if self.resolved.is_empty() {
            self.resolved = bundled
                .0
                .iter()
                .map(|(&slot, handle)| {
                    (
                        slot,
                        (
                            handle.id(),
                            icons
                                .get(handle)
                                .expect("Bundled icon asset exists")
                                .clone(),
                        ),
                    )
                })
                .collect();
        }
        let next = candidate.as_ref().unwrap_or(&self.resolved).clone();
        let requests = std::mem::take(&mut self.requests);
        // Prepare a normal-size raster once per loaded asset. Unusual sizes use
        // it for their first frame, then get an exact DPI raster on the next frame.
        let common: Vec<_> = self
            .sources
            .iter()
            .map(|(&id, icon)| (id, icon.clone()))
            .collect();
        for (id, icon) in common.into_iter().chain(next.values().cloned()) {
            self.ensure(context, id, &icon, common_pixels)?;
        }
        // Resolve last frame's semantic requests against the new pack before
        // committing it, so already visible large icons switch at full quality.
        for (source, pixels) in requests {
            let resolved = match source {
                Identity::Builtin(slot) => next.get(&slot).cloned(),
                Identity::Asset(id) => self.sources.get(&id).map(|icon| (id, icon.clone())),
            };
            if let Some((id, icon)) = resolved {
                self.ensure(context, id, &icon, pixels)?;
            } else if let Identity::Asset(id) = source {
                self.needs_redraw |= matches!(server.get_load_state(id), Some(LoadState::Loading));
            }
        }
        if let Some(candidate) = candidate {
            self.resolved = candidate;
            self.status.active = request;
        }
        // Old revisions, unloaded assets and abandoned sizes retire through the
        // managed registry, which waits for outstanding render snapshots.
        let expired: Vec<_> = self
            .rasters
            .iter()
            .filter(|(_, raster)| raster.last_used + 2 < self.frame)
            .map(|(&key, _)| key)
            .collect();
        for key in expired {
            let raster = self.rasters.remove(&key).unwrap();
            drop(raster);
        }
        self.needs_redraw |= self.status.pending;
        Ok(())
    }

    fn ensure(
        &mut self,
        context: &mut Context,
        id: AssetId<Icon>,
        icon: &Icon,
        pixels: u32,
    ) -> std::io::Result<()> {
        let key = RasterKey::new(id, icon, pixels);
        if let Some(raster) = self.rasters.get_mut(&key) {
            raster.last_used = self.frame;
            return Ok(());
        }
        let bytes = icon.rasterize(pixels)?;
        let texture = context.load_texture(
            "Bedtime icon",
            egui::ColorImage::from_rgba_unmultiplied([pixels as usize, pixels as usize], &bytes),
            egui::TextureOptions::LINEAR,
        );
        self.rasters.insert(
            key,
            Raster {
                _artwork: icon.clone(),
                texture,
                last_used: self.frame,
            },
        );
        self.rasterizations += 1;
        Ok(())
    }

    pub fn texture(&mut self, source: &IconSource, pixels: u32) -> Option<TextureId> {
        let identity = Identity::from(source);
        self.requests.insert((identity, pixels));
        let (id, icon) = match identity {
            Identity::Builtin(slot) => self.resolved.get(&slot).map(|(id, icon)| (*id, icon))?,
            Identity::Asset(id) => (id, self.sources.get(&id)?),
        };
        let key = RasterKey::new(id, icon, pixels);
        if let Some(raster) = self.rasters.get_mut(&key) {
            raster.last_used = self.frame;
            return Some(raster.texture.id());
        }
        self.needs_redraw = true;
        let fallback = RasterKey::new(id, icon, self.common_pixels);
        self.rasters
            .get(&fallback)
            .map(|raster| raster.texture.id())
    }

    pub fn textures(&self) -> usize {
        self.rasters.len()
    }
}

fn load_error(server: &AssetServer, id: UntypedAssetId) -> Option<String> {
    if let Some(LoadState::Failed(error)) = server.get_load_state(id) {
        return Some(error.to_string());
    }
    if let Some(RecursiveDependencyLoadState::Failed(error)) =
        server.get_recursive_dependency_load_state(id)
    {
        return Some(error.to_string());
    }
    None
}
