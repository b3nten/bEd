//! A hex viewer for workbench apps: any file's bytes (up to 256 MiB) as
//! offsets, hex and text, drawn by `bedtime_ui::hex::HexViewer`
//! (Cmd/Ctrl+F finds bytes or text, Cmd/Ctrl+G goes to an offset, the
//! right-click menu copies). It opens any file, so it's the viewer of last
//! resort for files nothing else shows.
use bedtime_document::DocumentModel;
use bedtime_filesystem::{ByteModel, FileModel};
use bedtime_ui::{UiContext, egui, hex::HexViewer};
#[cfg(feature = "persistence")]
use bedtime_workbench::persistence::ReflectPersist;
use bedtime_workbench::{Panel, PanelPresentation, ViewersAppExt, viewer_document};
use bevy::prelude::*;
use std::sync::Arc;

pub use bedtime_ui::hex::{
    BYTES_PER_ROW, HexColors, HexView, find, parse_hex, parse_offset, search_bytes,
};

pub struct HexPlugin;

impl Plugin for HexPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<HexPanel>()
            .register_viewer::<Bytes, HexPanel>("Hex")
            // Any file, as the furthest fit: what nothing else opens.
            .formats([bedtime_workbench::DATA]);
    }
}

/// A file's bytes, read-only.
#[derive(Clone, Default, PartialEq)]
pub struct Bytes(pub Arc<[u8]>);

impl DocumentModel for Bytes {
    type Change = std::convert::Infallible;
    type OpenState = ();
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String> {
        match *change {}
    }
}

impl ByteModel for Bytes {
    fn decode(bytes: &[u8]) -> Result<Self, String> {
        Ok(Self(bytes.into()))
    }
    fn encode(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        out.write_all(&self.0)
    }
}

impl FileModel for Bytes {}

#[bedtime_ui::panel]
#[derive(Reflect, Default)]
#[reflect(Component, Default)]
#[cfg_attr(feature = "persistence", reflect(Persist))]
pub struct HexPanel {
    #[reflect(ignore)]
    pub viewer: HexViewer,
}

impl HexPanel {
    pub fn cursor(&self) -> u64 {
        self.viewer.cursor()
    }
    pub fn selection(&self, len: u64) -> std::ops::Range<u64> {
        self.viewer.selection(len)
    }
}

impl Panel for HexPanel {
    const LABEL: &'static str = "Hex";

    fn draw(ui: &mut egui::Ui, world: &mut World, context: &UiContext<'_>, panel: Entity) {
        let Some(document) = viewer_document::<Bytes>(ui, world, panel) else {
            return;
        };
        let focused = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|presentation| presentation.focused);
        let Some(mut hex) = world.get_mut::<HexPanel>(panel) else {
            return;
        };
        ui.push_id(panel, |ui| {
            hex.viewer.show(ui, context, &document.model.0, focused)
        });
    }
}
