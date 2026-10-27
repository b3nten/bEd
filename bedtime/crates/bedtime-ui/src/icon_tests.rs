use crate::{icon_cache::IconCache, icons::BundledIcons, *};
use bevy::asset::AssetPlugin;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const DOT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="6"/></svg>"#;
const BOX: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24"/></svg>"#;

fn app(plugin: AssetPlugin) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, plugin));
    crate::icons::install(&mut app);
    app
}

fn prepare(
    cache: &mut IconCache,
    context: &mut egui::Context,
    app: &App,
    requested: Option<&UiIcons>,
    pixels: u32,
) {
    let world = app.world();
    cache
        .prepare(
            context,
            world.resource::<Assets<Icon>>(),
            world.resource::<Assets<IconSet>>(),
            world.resource::<BundledIcons>(),
            requested,
            world.resource::<AssetServer>(),
            pixels,
        )
        .unwrap();
}

#[test]
fn bundled_catalog_is_valid_monochrome_artwork_and_external_svg_is_validated() {
    let app = app(AssetPlugin::default());
    let world = app.world();
    let assets = world.resource::<Assets<Icon>>();
    let bundled = world.resource::<BundledIcons>();
    let catalog = include_str!("../assets/icons/catalog.tsv");
    let expected: std::collections::BTreeSet<_> = catalog
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(
        expected,
        BuiltinIcon::ALL.iter().map(|icon| icon.key()).collect()
    );
    for slot in BuiltinIcon::ALL {
        let pixels = assets.get(&bundled.0[slot]).unwrap().rasterize(32).unwrap();
        assert!(
            pixels.chunks_exact(4).any(|p| p[3] > 0),
            "{} must have a visible silhouette",
            slot.key()
        );
        assert!(pixels.chunks_exact(4).all(|p| p[..3] == [255; 3]));
    }
    assert!(Icon::from_svg(b"broken SVG").is_err());
    assert!(
        serde_json::from_str::<std::collections::BTreeMap<BuiltinIcon, String>>(
            r#"{"typo":"custom.svg"}"#
        )
        .is_err()
    );
}

#[test]
fn partial_packs_swap_atomically_preserve_explicit_icons_and_reuse_rasters() {
    let mut app = app(AssetPlugin::default());
    let mut context = egui::Context::default();
    let mut cache = IconCache::default();
    prepare(&mut cache, &mut context, &app, None, 32);
    let folder = IconSource::from(BuiltinIcon::Folder);
    let rust = IconSource::from(BuiltinIcon::Rust);
    let explicit =
        IconSource::Asset(app.world().resource::<BundledIcons>().0[&BuiltinIcon::Folder].clone());
    let default_folder = cache.texture(&folder, 32).unwrap();
    let default_rust = cache.texture(&rust, 32).unwrap();
    let source = app
        .world_mut()
        .resource_mut::<Assets<Icon>>()
        .add(Icon::from_svg(DOT).unwrap());
    let pack = app
        .world_mut()
        .resource_mut::<Assets<IconSet>>()
        .add(IconSet {
            overrides: [(BuiltinIcon::Folder, source.clone())].into(),
        });
    let selected = UiIcons(pack);
    prepare(&mut cache, &mut context, &app, Some(&selected), 32);
    let replaced = cache.texture(&folder, 32).unwrap();
    assert_ne!(replaced, default_folder);
    assert_eq!(
        cache.texture(&rust, 32),
        Some(default_rust),
        "Omitted slots must inherit bundled artwork"
    );
    assert_eq!(
        cache.texture(&explicit, 32),
        Some(default_folder),
        "Explicit assets must bypass pack overrides"
    );
    let count = cache.rasterizations;
    for _ in 0..4 {
        prepare(&mut cache, &mut context, &app, Some(&selected), 32);
        for _ in 0..10_000 {
            assert_eq!(cache.texture(&folder, 32), Some(replaced));
        }
    }
    assert_eq!(
        cache.rasterizations, count,
        "Repeated rows and frames must reuse prepared artwork"
    );

    let missing = app.world().resource::<Assets<Icon>>().reserve_handle();
    let pending = UiIcons(
        app.world_mut()
            .resource_mut::<Assets<IconSet>>()
            .add(IconSet {
                overrides: [(BuiltinIcon::Folder, missing.clone())].into(),
            }),
    );
    prepare(&mut cache, &mut context, &app, Some(&pending), 32);
    assert!(cache.status.pending);
    assert_eq!(cache.status.active.as_ref(), Some(&selected.0));
    assert_eq!(cache.texture(&folder, 32), Some(replaced));
    app.world_mut()
        .resource_mut::<Assets<Icon>>()
        .insert(missing.id(), Icon::from_svg(BOX).unwrap())
        .unwrap();
    prepare(&mut cache, &mut context, &app, Some(&pending), 32);
    assert!(!cache.status.pending);
    assert_eq!(cache.status.active.as_ref(), Some(&pending.0));
    assert_ne!(cache.texture(&folder, 32), Some(replaced));

    // Replacing artwork at the same asset ID must invalidate its rasters.
    let previous = cache.texture(&folder, 32).unwrap();
    app.world_mut()
        .resource_mut::<Assets<Icon>>()
        .insert(missing.id(), Icon::from_svg(DOT).unwrap())
        .unwrap();
    prepare(&mut cache, &mut context, &app, Some(&pending), 32);
    assert_ne!(cache.texture(&folder, 32), Some(previous));
    prepare(&mut cache, &mut context, &app, None, 32);
    assert!(cache.status.active.is_none());
    assert_eq!(cache.texture(&folder, 32), Some(default_folder));
    cache.clear();
    assert!(context.tex_manager().read().meta(default_folder).is_none());
    prepare(&mut cache, &mut context, &app, None, 32);
    assert_ne!(
        cache.texture(&folder, 32),
        Some(default_folder),
        "A replacement surface must get live textures rather than stale context IDs"
    );
}

#[test]
fn requested_sizes_and_dpi_get_exact_rasters_without_repeated_uploads() {
    let app = app(AssetPlugin::default());
    let mut context = egui::Context::default();
    let mut cache = IconCache::default();
    let source = BuiltinIcon::Folder.into();
    prepare(&mut cache, &mut context, &app, None, 17);
    let initial = cache.texture(&source, 48).unwrap();
    assert_eq!(
        context.tex_manager().read().meta(initial).unwrap().size[0],
        17
    );
    assert!(cache.needs_redraw);
    prepare(&mut cache, &mut context, &app, None, 17);
    let exact = cache.texture(&source, 48).unwrap();
    assert_eq!(
        context.tex_manager().read().meta(exact).unwrap().size[0],
        48
    );
    let count = cache.rasterizations;
    prepare(&mut cache, &mut context, &app, None, 17);
    assert_eq!(cache.texture(&source, 48), Some(exact));
    assert_eq!(cache.rasterizations, count);
    // A 2x display uses 34 physical pixels for a normal 17-point icon.
    prepare(&mut cache, &mut context, &app, None, 34);
    let high_dpi = cache.texture(&source, 34).unwrap();
    assert_eq!(
        context.tex_manager().read().meta(high_dpi).unwrap().size[0],
        34
    );
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("bedtime-ui-icons-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn asset_server_loads_relative_pack_dependencies_and_failed_loads_preserve_presentation() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.0.join("pack")).unwrap();
    std::fs::write(fixture.0.join("pack/custom.svg"), DOT).unwrap();
    std::fs::write(
        fixture.0.join("pack/good.icons.json"),
        r#"{"folder":"custom.svg"}"#,
    )
    .unwrap();
    std::fs::write(fixture.0.join("pack/bad.svg"), b"broken SVG").unwrap();
    std::fs::write(
        fixture.0.join("pack/bad.icons.json"),
        r#"{"folder":"bad.svg"}"#,
    )
    .unwrap();
    let mut app = app(AssetPlugin {
        file_path: fixture.0.to_string_lossy().into_owned(),
        watch_for_changes_override: Some(false),
        ..default()
    });
    let mut context = egui::Context::default();
    let mut cache = IconCache::default();
    let good = UiIcons(
        app.world()
            .resource::<AssetServer>()
            .load("pack/good.icons.json"),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.update();
        prepare(&mut cache, &mut context, &app, Some(&good), 32);
        if cache.status.active.as_ref() == Some(&good.0) {
            break;
        }
        assert!(cache.status.error.is_none(), "{:?}", cache.status.error);
        assert!(Instant::now() < deadline, "Pack did not finish loading");
        std::thread::sleep(Duration::from_millis(1));
    }
    let source = BuiltinIcon::Folder.into();
    let displayed = cache.texture(&source, 32).unwrap();
    let bad = UiIcons(
        app.world()
            .resource::<AssetServer>()
            .load("pack/bad.icons.json"),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.update();
        prepare(&mut cache, &mut context, &app, Some(&bad), 32);
        assert_eq!(cache.status.active.as_ref(), Some(&good.0));
        assert_eq!(cache.texture(&source, 32), Some(displayed));
        if cache.status.error.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Failed dependency was not reported"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!cache.status.pending);
}
