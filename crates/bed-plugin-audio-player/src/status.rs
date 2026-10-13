use crate::{AudioStates, audio};
use bed_api::documents::DocumentFrame;
use bed_api::{
    status::{
        StatusCommand, StatusContext, StatusDrawContext, StatusItem, StatusItems, StatusText,
    },
    workbench::WorkbenchAction,
};
use bevy::prelude::*;
use dear_imgui_rs::Ui;

pub(crate) const PLAY_COMMAND: &str = "bed.audio.status.play";
pub(crate) const PAUSE_COMMAND: &str = "bed.audio.status.pause";

pub(crate) fn items() -> StatusItems {
    StatusItems(vec![
        StatusItem {
            id: "bed.audio.document",
            label: "Audio",
            priority: 50,
            icon: "audio",
            prefer_icon: false,
            measure,
            draw,
            tooltip,
        },
        StatusItem {
            id: "bed.audio.playback",
            label: "Audio playback",
            priority: 150,
            icon: "audio",
            prefer_icon: false,
            measure,
            draw,
            tooltip,
        },
    ])
}

pub(super) fn presentation(world: &World, context: StatusContext) -> Option<StatusText> {
    let panel = world
        .get_non_send::<AudioStates>()?
        .get(context.publisher)?;
    let playing = panel.playing();
    if context.item_id == "bed.audio.playback" {
        if !playing {
            return None;
        }
    } else if !context.is_focused() || playing {
        return None;
    }
    let frame = world.get_resource::<DocumentFrame>()?;
    let document = frame
        .documents
        .iter()
        .find(|document| document.id == panel.document)?;
    let mut details = document.path.clone();
    let summary = if let Some(audio) = &panel.audio {
        details.push_str(&format!(
            "\n{} Hz · {} channels · {} frames\nVolume {:.0}%",
            audio.sample_rate,
            audio.channels,
            audio.frames(),
            panel.volume * 100.0
        ));
        let name = if playing && !context.is_focused() {
            format!(
                "{} · ",
                document.path.rsplit(['/', '\\']).next().unwrap_or("Audio")
            )
        } else {
            String::new()
        };
        format!(
            "{name}{} · {} / {}",
            if playing { "Playing" } else { "Paused" },
            audio::time_label(panel.position),
            audio::time_label(audio.duration())
        )
    } else if panel.error.is_some() {
        "Audio unavailable".into()
    } else {
        "Decoding audio…".into()
    };
    for error in [&panel.error, &panel.analysis_error, &panel.gpu_error]
        .into_iter()
        .flatten()
    {
        details.push_str(&format!("\n{error}"));
    }
    if panel.audio.is_some() && panel.analysis.is_none() && panel.analysis_error.is_none() {
        details.push_str("\nAnalyzing audio…");
    }
    let mut text = StatusText::new(summary, details);
    text.error =
        panel.error.is_some() || panel.analysis_error.is_some() || panel.gpu_error.is_some();
    if let Some(revision) = panel.revision {
        text.commands.push(StatusCommand {
            label: if playing { "Pause" } else { "Play" },
            command: if playing { PAUSE_COMMAND } else { PLAY_COMMAND },
            arguments: serde_json::json!({"revision": revision}),
            enabled: panel.audio.is_some(),
        });
    }
    Some(text)
}

fn measure(world: &World, ui: &Ui, context: StatusContext) -> Option<f32> {
    Some(presentation(world, context)?.measure(ui))
}

fn draw(
    world: &mut World,
    ui: &Ui,
    context: StatusDrawContext,
    actions: &mut Vec<WorkbenchAction>,
) {
    if let Some(text) = presentation(world, context.context) {
        text.draw(ui, context, actions);
    }
}

fn tooltip(world: &World, ui: &Ui, context: StatusContext) {
    if let Some(text) = presentation(world, context) {
        text.tooltip(ui);
    }
}
