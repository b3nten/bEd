//! Model preparation/dependency observation under the application's file lifecycle.
use bedtime_editor2::TextBuffer;
use bedtime_model2::{ModelScene, PreparedModel};
use bedtime_ui2::{UiContext, egui};
use bedtime_workbench2::*;
use bevy::prelude::*;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

type DiskStamp = Option<(u64, Option<SystemTime>)>;
#[derive(Clone, Debug)]
pub(crate) struct Dependency {
    path: PathBuf,
    stamp: DiskStamp,
}
#[derive(Clone, Debug)]
pub(crate) struct LoadedModel {
    prepared: Result<Arc<PreparedModel>, String>,
    dependencies: Vec<Dependency>,
}
fn stamp(path: &Path) -> DiskStamp {
    fs::metadata(path)
        .ok()
        .map(|m| (m.len(), m.modified().ok()))
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(format!(
            "{} must be a regular file of at most {} MiB",
            path.display(),
            limit / (1024 * 1024)
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Model resource exceeds its source limit".into());
    }
    Ok(bytes)
}
pub(crate) fn read_text(path: &Path) -> Result<TextBuffer, String> {
    String::from_utf8(read_bounded(path, 128 * 1024 * 1024)?)
        .map(TextBuffer::from)
        .map_err(|_| "glTF source is not UTF-8".into())
}
pub(crate) fn read(path: &Path) -> LoadedModel {
    match read_bounded(path, 128 * 1024 * 1024) {
        Ok(bytes) => prepare(&bytes, path, || false),
        Err(error) => LoadedModel {
            prepared: Err(error),
            dependencies: Vec::new(),
        },
    }
}
fn prepare(bytes: &[u8], path: &Path, cancelled: impl Fn() -> bool) -> LoadedModel {
    let mut dependencies = Vec::new();
    let directory = path.parent().expect("Absolute model source has a parent");
    let prepared = PreparedModel::prepare(bytes, path, |relative| {
        if cancelled() {
            return Err("Model preparation cancelled".into());
        }
        let path = directory.join(relative);
        let before = stamp(&path);
        dependencies.push(Dependency {
            path: path.clone(),
            stamp: before,
        });
        let bytes = read_bounded(&path, 64 * 1024 * 1024)?;
        if before != stamp(&path) {
            return Err(format!("{} changed while loading", path.display()));
        }
        Ok(bytes)
    })
    .and_then(|model| {
        if cancelled() {
            Err("Model preparation cancelled".into())
        } else {
            Ok(Arc::new(model))
        }
    });
    LoadedModel {
        prepared,
        dependencies,
    }
}

struct Preparation {
    stamp: (DocumentId, u64),
    task: WorkspaceTask<LoadedModel>,
}
#[derive(Component, Default)]
pub(crate) struct ModelPreview {
    observed: Option<(DocumentId, u64)>,
    observed_path: Option<PathBuf>,
    attempted: Option<(DocumentId, u64)>,
    changed: Option<Instant>,
    job: Option<Preparation>,
    dependencies: Vec<Dependency>,
    check: Option<WorkspaceTask<bool>>,
    checked: Option<Instant>,
    pub error: Option<String>,
}
impl Drop for ModelPreview {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.task.cancel();
        }
        if let Some(check) = &self.check {
            check.cancel();
        }
    }
}
pub(crate) fn accept(world: &mut World, panel: Entity, loaded: &LoadedModel) {
    let source = crate::files::panel_path(world, panel);
    let Some(mut preview) = world.get_mut::<ModelPreview>(panel) else {
        return;
    };
    if let Some(check) = preview.check.take() {
        check.cancel();
    }
    preview.observed_path = source;
    preview.dependencies = loaded.dependencies.clone();
    preview.checked = Some(Instant::now());
    preview.error = loaded.prepared.as_ref().err().cloned();
    match &loaded.prepared {
        Ok(model) => world
            .get_mut::<ModelScene>(panel)
            .unwrap()
            .set_model(model.clone()),
        Err(error) => world
            .get_mut::<ModelScene>(panel)
            .unwrap()
            .set_error(error.clone()),
    }
}
pub(crate) fn open_result(world: &World, panel: Entity) -> Option<Result<(), String>> {
    let preview = world.get::<ModelPreview>(panel)?;
    if let Some(error) = &preview.error {
        return Some(Err(error.clone()));
    }
    let scene = world.get::<ModelScene>(panel)?;
    if let Some(error) = scene.error() {
        return Some(Err(error.into()));
    }
    (scene.is_ready() && !scene.is_loading() && preview.job.is_none()).then_some(Ok(()))
}
pub(crate) fn draw(ui: &mut egui::Ui, world: &mut World, _: &UiContext<'_>, panel: Entity) {
    let focused = world
        .get::<PanelPresentation>(panel)
        .is_some_and(|p| p.focused);
    if focused
        && ui.is_enabled()
        && !ui.ctx().egui_wants_keyboard_input()
        && !egui::Popup::is_any_open(ui.ctx())
        && ui.input(|i| i.focused)
    {
        let (zoom_in, zoom_out, reset) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Plus)
                    || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Equals),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Minus),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Num0),
            )
        });
        if let Some(mut scene) = world.get_mut::<ModelScene>(panel) {
            if zoom_in {
                scene.zoom(1.0);
            }
            if zoom_out {
                scene.zoom(-1.0);
            }
            if reset {
                scene.view.camera = None;
            }
        }
    }
    if let Some(response) = bedtime_model2::ModelView::show(ui, world, panel) {
        response.context_menu(|ui| crate::opening::viewer_menu(ui, world, panel));
    }
}
pub(crate) fn update(world: &mut World) {
    let panels: Vec<_> = world
        .query_filtered::<Entity, With<ModelPreview>>()
        .iter(world)
        .collect();
    for panel in panels {
        let Some(workspace) = world.get::<InWorkspace>(panel).map(|w| w.0) else {
            continue;
        };
        if world
            .get::<WorkspaceWork>(workspace)
            .is_none_or(WorkspaceWork::is_closing)
        {
            continue;
        }
        let Some(path) = crate::files::panel_path(world, panel) else {
            continue;
        };
        let visible = world
            .get::<PanelPresentation>(panel)
            .is_some_and(|p| p.visible);
        let revision = world.get::<DocumentView>(panel).and_then(|v| {
            inspect_document(world, v.document())
                .ok()
                .map(|i| (v.document(), i.revision))
        });
        let mut state = world.entity_mut(panel).take::<ModelPreview>().unwrap();
        if state.observed_path.as_ref() != Some(&path) {
            state.observed_path = Some(path.clone());
            state.attempted = None;
            state.dependencies.clear();
            if let Some(check) = state.check.take() {
                check.cancel();
            }
            if let Some(job) = state.job.take() {
                job.task.cancel();
            }
        }
        if state.observed != revision {
            state.observed = revision;
            state.changed = Some(Instant::now());
            state.error = None;
            if let Some(job) = state.job.take() {
                job.task.cancel();
            }
        }
        let mut accepted = None;
        if let Some(job) = &state.job {
            let result = match job.task.try_recv() {
                Ok(loaded) => Some(loaded),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(LoadedModel {
                    prepared: Err("Model worker stopped".into()),
                    dependencies: Vec::new(),
                }),
            };
            if let Some(loaded) = result {
                let job = state.job.take().unwrap();
                if Some(job.stamp) == revision {
                    state.attempted = revision;
                    accepted = Some(loaded);
                }
            }
        }
        let dependency_changed = state
            .check
            .as_ref()
            .and_then(|check| match check.try_recv() {
                Ok(changed) => Some(changed),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(false),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
            });
        let reload = dependency_changed == Some(true);
        if dependency_changed.is_some() {
            state.check = None;
        }
        if reload {
            state.attempted = None;
            state.error = None;
        }
        if visible
            && state.check.is_none()
            && !state.dependencies.is_empty()
            && state
                .checked
                .is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
        {
            let dependencies = state.dependencies.clone();
            state.checked = Some(Instant::now());
            state.check = world
                .get_mut::<WorkspaceWork>(workspace)
                .unwrap()
                .spawn_task(Some(panel), "Check model resources", move |_| {
                    dependencies.iter().any(|d| stamp(&d.path) != d.stamp)
                })
                .ok();
        }
        if let Some(revision) = revision {
            let initial = state.attempted.is_none();
            if (visible || initial)
                && state.job.is_none()
                && Some(revision) != state.attempted
                && (initial
                    || state
                        .changed
                        .is_none_or(|t| t.elapsed() >= Duration::from_millis(150)))
            {
                let text = cast_document::<TextBuffer>(world, revision.0)
                    .ok()
                    .and_then(|h| read_document(world, h).ok())
                    .cloned();
                if let Some(text) = text {
                    let source = path.clone();
                    match world
                        .get_mut::<WorkspaceWork>(workspace)
                        .unwrap()
                        .spawn_task(Some(panel), "Prepare model", move |cancel| {
                            prepare(text.to_string().as_bytes(), &source, || {
                                cancel.is_cancelled()
                            })
                        }) {
                        Ok(task) => {
                            state.job = Some(Preparation {
                                stamp: revision,
                                task,
                            })
                        }
                        Err(error) => {
                            state.error = Some(error);
                            state.attempted = Some(revision);
                        }
                    }
                }
            }
        }
        world.entity_mut(panel).insert(state);
        if let Some(loaded) = accepted {
            accept(world, panel, &loaded);
        }
        if reload && revision.is_none() {
            let _ = crate::files::load_preview(world, panel, path, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sidecar_observation_tracks_missing_files_and_source_changes() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let directory = std::env::temp_dir().join(format!("bed-model-deps-{}", std::process::id()));
        fs::create_dir_all(directory.join("models")).unwrap();
        let path = directory.join("models/scene.gltf");
        let mut root: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../bedtime/tests/fixtures/gltf/cube.gltf"
        ))
        .unwrap();
        let bytes = STANDARD
            .decode(
                root["buffers"][0]["uri"]
                    .as_str()
                    .unwrap()
                    .split_once(',')
                    .unwrap()
                    .1,
            )
            .unwrap();
        root["buffers"][0]["uri"] = serde_json::json!("../geometry%20data.bin");
        let source = serde_json::to_vec(&root).unwrap();
        let missing = prepare(&source, &path, || false);
        assert!(missing.prepared.is_err());
        assert_eq!(missing.dependencies.len(), 1);
        assert_eq!(missing.dependencies[0].stamp, None);
        fs::write(directory.join("geometry data.bin"), &bytes).unwrap();
        assert!(
            missing
                .dependencies
                .iter()
                .any(|d| stamp(&d.path) != d.stamp)
        );
        let loaded = prepare(&source, &path, || false);
        assert!(loaded.prepared.is_ok());
        assert!(
            loaded
                .dependencies
                .iter()
                .all(|d| stamp(&d.path) == d.stamp)
        );
        fs::write(directory.join("geometry data.bin"), b"invalid").unwrap();
        assert!(
            loaded
                .dependencies
                .iter()
                .any(|d| stamp(&d.path) != d.stamp)
        );
        assert!(prepare(&source, &path, || false).prepared.is_err());
        assert!(prepare(&source, &path, || true).prepared.is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
