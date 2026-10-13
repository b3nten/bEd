//! A complete desktop with one optional terminal panel.
use serde_json::json;

fn main() -> std::io::Result<()> {
    let exit = bed_app::Application::new("example.terminal", "Terminal application")?
        .register(bed_plugin_terminal::launch_registration())
        .run(|app| {
            bed_app::bed_core::bed_workbench::open_panel(
                app.world_mut(),
                json!({"panel_type": bed_plugin_terminal::PANEL_ID}),
                None,
                None,
            )
            .map_err(std::io::Error::other)?;
            Ok(())
        })?;
    std::process::exit(exit);
}
