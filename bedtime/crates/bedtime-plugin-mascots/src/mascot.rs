use crate::{MascotFrame, MascotKind, MascotView};
use bedtime_api::{
    features::{FeatureRegistrar, HostContext, MenuSlot},
    workbench::PanelPlacement,
};
use bedtime_ui::gpu::Canvas;
use dear_imgui_rs::{MouseButton, StyleColor, Ui};
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, buffer::SamplesBuffer};
use std::io::Cursor;

pub const PANEL_ID: &str = "bedtime.duck.panel";

pub(crate) fn register_bed_metadata(registrar: &mut FeatureRegistrar<'_>) {
    registrar.panel_options("bedtime.mascot.panel", true, PanelPlacement::Sidebar);
    registrar.command_ref("bedtime.mascot.open");
    registrar.menu(MenuSlot::Application, "bedtime.mascot.open");
}

pub(crate) fn register_duck_metadata(registrar: &mut FeatureRegistrar<'_>) {
    registrar.panel_options(PANEL_ID, true, PanelPlacement::Sidebar);
}

pub(crate) struct MascotPanel {
    pub(crate) scene: MascotView,
    audio: Option<(OutputStream, Sink)>,
    audio_failed: bool,
}

impl MascotPanel {
    pub(crate) fn new(kind: MascotKind) -> Self {
        Self {
            scene: MascotView::new(kind),
            audio: None,
            audio_failed: false,
        }
    }

    fn squeak(&mut self) {
        if self.audio_failed {
            return;
        }
        if self.audio.is_none() {
            let Ok(mut stream) = OutputStreamBuilder::open_default_stream() else {
                self.audio_failed = true;
                return;
            };
            stream.log_on_drop(false);
            let sink = Sink::connect_new(stream.mixer());
            self.audio = Some((stream, sink));
        }
        let sink = &self.audio.as_ref().unwrap().1;
        // Rapid clicks replace the previous sound rather than queueing seconds
        // of audio after the mascot has stopped moving.
        sink.clear();
        match self.scene.kind {
            MascotKind::Bed => {
                let source = Decoder::try_from(Cursor::new(
                    include_bytes!("../resources/bed.mp3").as_slice(),
                ));
                match source {
                    Ok(source) => sink.append(source),
                    Err(error) => {
                        eprintln!("Unable to decode Bed click sound: {error}");
                        self.audio_failed = true;
                        return;
                    }
                }
            }
            MascotKind::Duck => sink.append(SamplesBuffer::new(1, 22_050, squeak_samples())),
            MascotKind::Bedtime | MascotKind::WelcomeBed => unreachable!("Not a mascot panel"),
        }
        sink.play();
    }
}

impl MascotPanel {
    pub(crate) fn panel_title(&self, _: &HostContext<'_>) -> String {
        match self.scene.kind {
            MascotKind::Bed => "Bed",
            MascotKind::Duck => "Ducky",
            MascotKind::Bedtime | MascotKind::WelcomeBed => unreachable!("Not a mascot panel"),
        }
        .into()
    }

    pub(crate) fn draw_canvas(&mut self, ui: &Ui, host: &HostContext<'_>) {
        let origin = ui.cursor_screen_pos();
        let canvas = Canvas::show(ui, host.texture(self.scene.handle()), "##mascot_canvas");
        let mouse = ui.io().mouse_pos();
        let pointer = if ui.is_mouse_pos_valid() {
            [
                (mouse[0] - origin[0]) / canvas.size[0] * 2.0 - 1.0,
                (mouse[1] - origin[1]) / canvas.size[1] * 2.0 - 1.0,
            ]
        } else {
            [0.0; 2]
        };
        let pressed = canvas.hovered && ui.is_mouse_clicked(MouseButton::Left);
        if pressed {
            self.squeak();
        }
        self.scene.update(
            canvas.pixels,
            MascotFrame {
                background: [0.0; 4],
                accent: ui.style_color(StyleColor::CheckMark),
                mouse: pointer,
                animations: host.animations,
                pressed,
            },
        );
    }
}

fn squeak_samples() -> Vec<f32> {
    let rate = 22_050.0;
    let mut phase = 0.0;
    (0..5512)
        .map(|i| {
            let t = i as f32 / rate;
            let envelope = (t * 70.0).min(1.0) * (1.0 - t / 0.25).max(0.0).powi(2);
            let frequency = 950.0 + 1300.0 * (t * 13.0).sin().abs();
            phase += std::f32::consts::TAU * frequency / rate;
            (phase.sin() * 0.75 + (phase * 2.0).sin() * 0.25) * envelope * 0.16
        })
        .collect()
}
