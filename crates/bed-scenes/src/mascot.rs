use crate::{SceneFrame, SceneKind, SceneView};
use bed_document_session::DocumentId;
use bed_workbench_api::{
    CommandContext, HostContext, HostRequest, MenuSlot, Module, ModulePanel, PanelPlacement,
    Registrar,
    gpu::{Canvas, GpuContext, RenderOutput, RenderTarget},
};
use dear_imgui_rs::{MouseButton, StyleColor, Ui};
use rodio::{Decoder, OutputStream, OutputStreamBuilder, Sink, buffer::SamplesBuffer};
use serde_json::Value;
use std::{any::Any, io::Cursor};

pub const PANEL_ID: &str = "bed.duck.panel";

pub struct BedModule;
impl Module for BedModule {
    fn id(&self) -> &'static str {
        "bed.mascot"
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel_options(
            "bed.mascot.panel",
            "Bed",
            true,
            PanelPlacement::Sidebar,
            None,
        );
        registrar.command("bed.mascot.open", "Bed", Some("bed"));
        registrar.menu(MenuSlot::Application, "bed.mascot.open");
    }
    fn command(
        &mut self,
        command: &str,
        _: &CommandContext,
        _: &HostContext<'_>,
        requests: &mut Vec<HostRequest>,
    ) {
        if command == "bed.mascot.open" {
            requests.push(HostRequest::OpenPanel {
                panel_type: "bed.mascot.panel".into(),
                document: None,
                state: Value::Null,
            });
        }
    }
    fn create_panel(
        &mut self,
        kind: &str,
        document: Option<DocumentId>,
        _: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if kind != "bed.mascot.panel" || document.is_some() {
            return Err("Bed is a tool panel without a document".into());
        }
        Ok(Box::new(MascotPanel::new(SceneKind::Bed)))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct DuckModule;
impl Module for DuckModule {
    fn id(&self) -> &'static str {
        "bed.duck"
    }
    fn register(&self, registrar: &mut Registrar<'_>) {
        registrar.panel_options(PANEL_ID, "Ducky", true, PanelPlacement::Sidebar, None);
    }
    fn command(
        &mut self,
        _: &str,
        _: &CommandContext,
        _: &HostContext<'_>,
        _: &mut Vec<HostRequest>,
    ) {
    }
    fn create_panel(
        &mut self,
        kind: &str,
        document: Option<DocumentId>,
        _: &Value,
    ) -> Result<Box<dyn ModulePanel>, String> {
        if kind != PANEL_ID || document.is_some() {
            return Err("Ducky is a tool panel without a document".into());
        }
        Ok(Box::new(MascotPanel::new(SceneKind::Duck)))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

struct MascotPanel {
    scene: SceneView,
    muted: bool,
    audio: Option<(OutputStream, Sink)>,
    audio_failed: bool,
}
impl MascotPanel {
    fn new(kind: SceneKind) -> Self {
        Self {
            scene: SceneView::new(kind),
            muted: false,
            audio: None,
            audio_failed: false,
        }
    }
    fn squeak(&mut self) {
        if self.muted || self.audio_failed {
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
            SceneKind::Bed => {
                let source = Decoder::try_from(Cursor::new(
                    include_bytes!("../../../assets/bed.mp3").as_slice(),
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
            SceneKind::Duck => sink.append(SamplesBuffer::new(1, 22_050, squeak_samples())),
            SceneKind::Bedtime | SceneKind::WelcomeBed => unreachable!("Not a mascot panel"),
        }
        sink.play();
    }
}
impl ModulePanel for MascotPanel {
    fn title(&self, _: &HostContext<'_>) -> String {
        match self.scene.kind {
            SceneKind::Bed => "Bed",
            SceneKind::Duck => "Ducky",
            SceneKind::Bedtime | SceneKind::WelcomeBed => unreachable!("Not a mascot panel"),
        }
        .into()
    }
    fn window_padding(&self) -> Option<[f32; 2]> {
        Some([0.0; 2])
    }
    fn draw(&mut self, ui: &Ui, host: &HostContext<'_>, _: &mut Vec<HostRequest>) {
        let settings_id = match self.scene.kind {
            SceneKind::Bed => "bed.mascot",
            SceneKind::Duck => "bed.duck",
            SceneKind::Bedtime | SceneKind::WelcomeBed => unreachable!("Not a mascot panel"),
        };
        let muted = !host.settings_for(settings_id)["squeaks_enabled"]
            .as_bool()
            .unwrap_or(true);
        if muted
            && !self.muted
            && let Some((_, sink)) = &self.audio
        {
            sink.clear();
        }
        self.muted = muted;
        let origin = ui.cursor_screen_pos();
        let canvas = Canvas::show(ui, host, self.scene.handle(), "##mascot_canvas");
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
            SceneFrame {
                background: [0.0; 4],
                accent: ui.style_color(StyleColor::CheckMark),
                mouse: pointer,
                animations: host.animations,
                pressed,
            },
        );
    }
    fn render_output(&self) -> Option<RenderOutput> {
        self.scene.render_output()
    }
    fn render(&mut self, gpu: &mut GpuContext<'_>, target: &RenderTarget) -> Result<(), String> {
        self.scene.render(gpu, target)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
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

#[cfg(test)]
mod tests {
    use super::*;
    use dear_imgui_rs::{Condition, Context, FramePrepareOptions, StyleVar, WindowFlags};
    use serde_json::json;
    use std::collections::HashMap;
    #[test]
    fn mascots_are_document_free_and_do_not_store_global_preferences_in_panel_state() {
        for (mut module, panel_id) in [
            (Box::new(BedModule) as Box<dyn Module>, "bed.mascot.panel"),
            (Box::new(DuckModule) as Box<dyn Module>, PANEL_ID),
        ] {
            let panel = module
                .create_panel(panel_id, None, &json!({"muted": true}))
                .unwrap();
            assert_eq!(panel.save_state(), Value::Null);
            assert!(
                module
                    .create_panel(panel_id, Some(DocumentId::next()), &Value::Null)
                    .is_err()
            );
        }
    }

    #[test]
    fn bed_replaces_ducky_in_menus_and_commands_but_keeps_its_panel() {
        let mut registry = bed_workbench_api::Registry::default();
        registry.register(&BedModule).unwrap();
        registry.register(&DuckModule).unwrap();
        assert!(registry.panel(PANEL_ID).is_some());
        assert!(
            registry
                .commands
                .iter()
                .all(|command| command.plugin != "bed.duck")
        );
        let application: Vec<_> = registry
            .menus
            .iter()
            .filter(|menu| menu.slot == MenuSlot::Application)
            .map(|menu| registry.command(menu.command).unwrap().label)
            .collect();
        assert_eq!(application, ["Bed"]);
    }
    #[test]
    fn mascot_canvases_fill_the_panel_without_header_controls() {
        let mut context = Context::create();
        context
            .set_ini_filename(None::<std::path::PathBuf>)
            .unwrap();
        context
            .font_atlas()
            .try_claim_legacy_renderer()
            .unwrap()
            .build();
        for (mut module, panel_id, settings_id) in [
            (
                Box::new(BedModule) as Box<dyn Module>,
                "bed.mascot.panel",
                "bed.mascot",
            ),
            (
                Box::new(DuckModule) as Box<dyn Module>,
                PANEL_ID,
                "bed.duck",
            ),
        ] {
            let mut panel = module
                .create_panel(panel_id, None, &json!({"muted":true}))
                .unwrap();
            let mut settings = json!({settings_id:{"squeaks_enabled":false}});
            for size in [[320.0, 600.0], [800.0, 400.0]] {
                context.prepare_frame(FramePrepareOptions::new(size, 1.0 / 60.0));
                let ui = context.frame();
                let _padding =
                    ui.push_style_var(StyleVar::WindowPadding(panel.window_padding().unwrap()));
                ui.window("Mascot fixture")
                    .position([0.0; 2], Condition::Always)
                    .size(size, Condition::Always)
                    .flags(WindowFlags::NO_TITLE_BAR)
                    .build(|| {
                        let expected = ui.content_region_avail().map(|value| value as u32);
                        panel.draw(
                            ui,
                            &HostContext {
                                remote: false,
                                default_viewers: &Value::Null,
                                viewer_menu: None,
                                documents: &[],
                                active_document: None,
                                settings: &settings,
                                textures: &HashMap::new(),
                                animations: false,
                                workspace: 1,
                                diagnostics: &Value::Null,
                            },
                            &mut Vec::new(),
                        );
                        assert_eq!(panel.render_output().unwrap().size, expected);
                        let mascot = panel.as_any().downcast_ref::<MascotPanel>().unwrap();
                        assert_eq!(
                            mascot.muted,
                            !settings[settings_id]["squeaks_enabled"].as_bool().unwrap()
                        );
                    });
                drop(_padding);
                drop(context.render_legacy());
                settings[settings_id]["squeaks_enabled"] = json!(true);
            }
        }
    }
    #[test]
    fn squeak_is_bounded_and_fades_out() {
        let samples = squeak_samples();
        assert!(
            samples
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 0.16)
        );
        assert!(samples.last().unwrap().abs() < 0.001);
    }
}
