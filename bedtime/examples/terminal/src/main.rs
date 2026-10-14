//! A complete desktop with one optional terminal panel.
use serde_json::json;

fn main() -> std::io::Result<()> {
    let exit = bedtime::awaken("example.terminal", "Terminal application")?
        .with_launchers(bedtime_plugin_terminal::launchers())
        .run(bedtime::DesktopOptions::default(), |app| {
            app.add_plugins(bedtime_plugin_terminal::TerminalPlugin);
            bedtime::core::bedtime_workbench::open_panel(
                app.world_mut(),
                json!({"panel_type": bedtime_plugin_terminal::PANEL_ID}),
                None,
                None,
            )
            .map_err(std::io::Error::other)?;
            Ok(())
        })?;
    std::process::exit(exit);
}
