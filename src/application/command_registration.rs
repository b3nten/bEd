//! bEd commands and defaults registered on the shared workbench.
use super::runtime::*;
use bed_api::workbench::PanelTarget;
use bed_api::workbench::{CommandInvocation, CommandResult};
use bevy::prelude::*;

pub fn register(world: &mut World) {
    bed_api::workbench::register(world, "bed.workbench", |registrar| {
        registrar.command("bed.workbench.split_right", "Split Right", command);
        registrar.command("bed.workbench.split_down", "Split Down", command);
        registrar.global_shortcut("bed.workbench.split_right", &["Primary+Backslash"]);
    })
    .expect("bEd layout commands are registered once");
    bed_api::workbench::register(world, "bed.app", |registrar| {
        registrar.command("bed.app.Debug", "Debug", command);
        registrar.command("bed.app.NewDocument", "New Document", command);
        registrar.command("bed.app.NewTerminal", "New Terminal", command);
        registrar.command("bed.app.NewExplorer", "New Explorer", command);
        registrar.command("bed.app.NewSettings", "New Settings", command);
        #[cfg(feature = "workspaces")]
        registrar.command("bed.app.NewProjects", "New Workspaces Panel", command);
        registrar.command("bed.app.NewDiagnostics", "New Diagnostics", command);
        registrar.command("bed.app.NewStructure", "New Structure", command);
        registrar.command("bed.app.NewLspDashboard", "New Lsp Dashboard", command);
        registrar.command("bed.app.NewContentSearch", "New Content Search", command);
        registrar.command("bed.app.NewReferences", "New References", command);
        registrar.command("bed.app.DuplicateView", "Duplicate View", command);
        #[cfg(feature = "workspaces")]
        registrar.command("bed.app.Projects", "Workspaces", command);
        registrar.command("bed.app.Diagnostics", "Diagnostics", command);
        registrar.command("bed.app.Structure", "Structure", command);
        registrar.command("bed.app.OpenFolder", "Open Folder", command);
        registrar.command("bed.app.Close", "Close", command);
        registrar.command("bed.app.Quit", "Quit", command);
        registrar.command("bed.app.Explorer", "Explorer", command);
        registrar.command("bed.app.Terminal", "Terminal", command);
        registrar.command("bed.app.Settings", "Settings", command);
        registrar.command("bed.app.LspDashboard", "Lsp Dashboard", command);
        registrar.command("bed.app.FindFile", "Find File", command);
        registrar.command("bed.app.FindProject", "Search Workspace", command);
        registrar.global_shortcut("bed.app.NewDocument", &["Primary+N"]);
        registrar.global_shortcut("bed.app.Close", &["Primary+W"]);
        registrar.global_shortcut("bed.app.Quit", &["Primary+Q"]);
        registrar.command("bed.app.SetCurrentLayout", "Set Current Layout", command);
        registrar.command(
            "bed.app.layout_startup",
            "Switch to Startup Layout",
            command,
        );
        registrar.command("bed.app.layout_1", "Switch to Layout 1", command);
        registrar.global_shortcut("bed.app.layout_1", &["Primary+1"]);
        registrar.command("bed.app.layout_2", "Switch to Layout 2", command);
        registrar.global_shortcut("bed.app.layout_2", &["Primary+2"]);
        registrar.command("bed.app.layout_3", "Switch to Layout 3", command);
        registrar.global_shortcut("bed.app.layout_3", &["Primary+3"]);
        registrar.command("bed.app.layout_4", "Switch to Layout 4", command);
        registrar.global_shortcut("bed.app.layout_4", &["Primary+4"]);
        registrar.command("bed.app.layout_5", "Switch to Layout 5", command);
        registrar.global_shortcut("bed.app.layout_5", &["Primary+5"]);
        registrar.command("bed.app.layout_6", "Switch to Layout 6", command);
        registrar.global_shortcut("bed.app.layout_6", &["Primary+6"]);
        registrar.command("bed.app.layout_7", "Switch to Layout 7", command);
        registrar.global_shortcut("bed.app.layout_7", &["Primary+7"]);
        registrar.command("bed.app.layout_8", "Switch to Layout 8", command);
        registrar.global_shortcut("bed.app.layout_8", &["Primary+8"]);
        registrar.command("bed.app.layout_9", "Switch to Layout 9", command);
        registrar.global_shortcut("bed.app.layout_9", &["Primary+9"]);
        registrar.command("bed.app.focus_1", "Focus Panel 1", focus);
        registrar.global_shortcut("bed.app.focus_1", &["Primary+Shift+1"]);
        registrar.command("bed.app.focus_2", "Focus Panel 2", focus);
        registrar.global_shortcut("bed.app.focus_2", &["Primary+Shift+2"]);
        registrar.command("bed.app.focus_3", "Focus Panel 3", focus);
        registrar.global_shortcut("bed.app.focus_3", &["Primary+Shift+3"]);
        registrar.command("bed.app.focus_4", "Focus Panel 4", focus);
        registrar.global_shortcut("bed.app.focus_4", &["Primary+Shift+4"]);
        registrar.command("bed.app.focus_5", "Focus Panel 5", focus);
        registrar.global_shortcut("bed.app.focus_5", &["Primary+Shift+5"]);
        registrar.command("bed.app.focus_6", "Focus Panel 6", focus);
        registrar.global_shortcut("bed.app.focus_6", &["Primary+Shift+6"]);
        registrar.command("bed.app.focus_7", "Focus Panel 7", focus);
        registrar.global_shortcut("bed.app.focus_7", &["Primary+Shift+7"]);
        registrar.command("bed.app.focus_8", "Focus Panel 8", focus);
        registrar.global_shortcut("bed.app.focus_8", &["Primary+Shift+8"]);
        registrar.command("bed.app.focus_9", "Focus Panel 9", focus);
        registrar.global_shortcut("bed.app.focus_9", &["Primary+Shift+9"]);
    })
    .expect("application commands are registered once");
}

fn command(In(invocation): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let command = match invocation.command.as_str() {
        "bed.app.Debug" => WindowCommand::Debug,
        "bed.app.NewDocument" => WindowCommand::NewDocument,
        "bed.app.NewTerminal" => WindowCommand::NewTerminal,
        "bed.app.NewExplorer" => WindowCommand::NewExplorer,
        "bed.app.NewSettings" => WindowCommand::NewSettings,
        "bed.app.NewProjects" => WindowCommand::NewWorkspaces,
        "bed.app.NewDiagnostics" => WindowCommand::NewDiagnostics,
        "bed.app.NewStructure" => WindowCommand::NewStructure,
        "bed.app.NewLspDashboard" => WindowCommand::NewLspDashboard,
        "bed.app.NewContentSearch" => WindowCommand::NewContentSearch,
        "bed.app.NewReferences" => WindowCommand::NewReferences,
        "bed.app.DuplicateView" => WindowCommand::DuplicateView,
        "bed.workbench.split_right" => WindowCommand::SplitRight,
        "bed.workbench.split_down" => WindowCommand::SplitDown,
        "bed.app.SetCurrentLayout" => WindowCommand::SetCurrentLayout,
        "bed.app.ResetLayout" => WindowCommand::ResetLayout,
        "bed.app.SaveDefaultLayout" => WindowCommand::SaveDefaultLayout,
        "bed.app.ResetDefaultLayout" => WindowCommand::ResetDefaultLayout,
        "bed.app.Projects" => WindowCommand::Workspaces,
        "bed.app.Diagnostics" => WindowCommand::Diagnostics,
        "bed.app.Structure" => WindowCommand::Structure,
        "bed.app.OpenFolder" => WindowCommand::OpenFolder,
        "bed.app.Close" => WindowCommand::Close,
        "bed.app.Quit" => WindowCommand::Quit,
        "bed.app.Explorer" => WindowCommand::Explorer,
        "bed.app.Terminal" => WindowCommand::Terminal,
        "bed.app.Settings" => WindowCommand::Settings,
        "bed.app.LspDashboard" => WindowCommand::LspDashboard,
        "bed.app.FindFile" => WindowCommand::FindFile,
        "bed.app.FindProject" => WindowCommand::FindWorkspace,
        id if id.starts_with("bed.app.layout_") => {
            WindowCommand::UseLayout(LayoutSlot::from_key(&id[15..]).ok_or("Invalid layout slot")?)
        }
        _ => return Err("Unknown application command".into()),
    };
    let mut request = capture_app_request_from(
        world,
        AppAction::Command(command),
        PanelTarget::LargestArea,
        invocation.source,
    );
    request.area = invocation.area;
    world.resource_mut::<AppRequests>().enqueue(request);
    Ok(Vec::new())
}

fn focus(In(invocation): In<CommandInvocation>, world: &mut World) -> CommandResult {
    let index = invocation
        .command
        .strip_prefix("bed.app.focus_")
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or("Invalid panel focus command")?;
    Ok(active_panel_entities(world)
        .get(index - 1)
        .map(|entity| vec![bed_api::workbench::WorkbenchAction::FocusPanel(*entity)])
        .unwrap_or_default())
}
