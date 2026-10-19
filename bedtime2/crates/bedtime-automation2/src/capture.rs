use crate::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use std::{
    hash::{Hash, Hasher},
    path::PathBuf,
};

pub(crate) fn start(
    world: &mut World,
    args: &Value,
    token: u64,
    frame: u64,
) -> Result<Entity, String> {
    #[cfg(not(feature = "workbench"))]
    if args.get("panel").is_some() {
        return Err("Panel capture requires the workbench feature".into());
    }
    #[cfg(not(feature = "ui"))]
    if args.get("viewport").is_some() {
        return Err("Viewport capture requires the ui feature".into());
    }
    if args.get("panel").is_some() && args.get("viewport").is_some() {
        return Err("Choose a panel or viewport capture target".into());
    }
    if !world.contains_resource::<bevy::render::renderer::RenderDevice>() {
        return Err("Capture requires Bevy's renderer".into());
    }
    let path = PathBuf::from(required::<String>(args, "path")?);
    if path.extension().and_then(|s| s.to_str()) != Some("png") {
        return Err("Capture path must end in .png".into());
    }
    let crop: Option<[u32; 4]> = optional(args, "crop")?;
    let screenshot;
    #[cfg(feature = "ui")]
    if args.get("viewport").is_some() {
        let e = entity(world, args, "viewport")?;
        let viewport = world
            .get::<bedtime_ui2::UiViewport>(e)
            .ok_or("Entity is not a UiViewport")?;
        screenshot = Screenshot::image(viewport.image.clone());
    } else {
        let window = window(world, args)?;
        screenshot = Screenshot::window(window);
    }
    #[cfg(not(feature = "ui"))]
    {
        screenshot = Screenshot::window(window(world, args)?);
    }
    #[cfg(feature = "workbench")]
    let crop = if args.get("panel").is_some() {
        let panel = entity(world, args, "panel")?;
        let p = world
            .get::<bedtime_workbench2::PanelPresentation>(panel)
            .ok_or("Entity is not a panel")?;
        if !p.visible {
            return Err("Panel is not visible".into());
        }
        let rect = p.rect;
        let window = window(world, args)?;
        let cameras: Vec<_> = world
            .query::<(Entity, &bedtime_ui2::UiCamera)>()
            .iter(world)
            .filter_map(|(e, c)| (c.0 == window).then_some(e))
            .collect();
        let camera = *cameras.first().ok_or("No UI camera for panel capture")?;
        let scale = world
            .get_mut::<bevy_egui::EguiContext>(camera)
            .ok_or("No egui context")?
            .get_mut()
            .pixels_per_point();
        Some([
            (rect.min.x * scale).round().max(0.0) as u32,
            (rect.min.y * scale).round().max(0.0) as u32,
            (rect.width() * scale).round() as u32,
            (rect.height() * scale).round() as u32,
        ])
    } else {
        crop
    };
    let client = world.resource::<Client>().clone();
    let proxy = world
        .get_resource::<bevy::winit::EventLoopProxyWrapper>()
        .map(|p| (**p).clone());
    let entity = world.spawn(screenshot).observe(move |capture:On<ScreenshotCaptured>| {
        let image = capture.image.clone();
        let path = path.clone();
        let sender = client.sender.clone();
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let mut image = image.try_into_dynamic().map_err(|e|e.to_string())?.to_rgba8();
                if let Some([x,y,w,h]) = crop {
                    if w == 0 || h == 0 || x.checked_add(w).is_none_or(|v|v>image.width()) || y.checked_add(h).is_none_or(|v|v>image.height()) { return Err("Crop lies outside captured image".into()); }
                    image = image::imageops::crop_imm(&image,x,y,w,h).to_image();
                }
                if let Some(parent) = path.parent().filter(|p|!p.as_os_str().is_empty()) { std::fs::create_dir_all(parent).map_err(|e|e.to_string())?; }
                image.save_with_format(&path,image::ImageFormat::Png).map_err(|e|e.to_string())?;
                let mut hash = std::hash::DefaultHasher::new();
                image.as_raw().hash(&mut hash);
                let nontransparent = image.pixels().filter(|p|p.0[3]>0).count();
                Ok(json!({"path":path,"width":image.width(),"height":image.height(),"capture_frame":frame,"hash":format!("{:016x}",hash.finish()),"nontransparent_pixels":nontransparent}))
            })();
            let _ = sender.send(Inbound::Captured(token,result));
            if let Some(proxy) = proxy { let _ = proxy.send_event(bevy::winit::WinitUserEvent::WakeUp); }
        });
    }).id();
    Ok(entity)
}
