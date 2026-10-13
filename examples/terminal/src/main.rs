//! A complete desktop with one optional terminal panel.
use serde_json::json;

fn main() -> std::io::Result<()> {
    let exit = bed_launch::builder("example.terminal", "Terminal application")?
        .with_launchers(bed_plugin_terminal::launchers())
        .run(bed_launch::DesktopOptions::default(), |app| {
            app.add_plugins((
                bed_plugin_terminal::TerminalPlugin,
                bed_plugin_terminal::TerminalAppIntegrationPlugin,
            ));
            bed_launch::bed_core::bed_workbench::open_panel(
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
