use super::*;
use bedtime_treesitter::{AnalysisSession, Observation};
use std::{collections::HashSet, sync::Arc};

/// Explicit document language; callers may detect a filename first, or override.
#[derive(Component, Clone)]
pub struct CodeLanguage(pub String);
#[derive(Component)]
pub struct DocumentAnalysis {
    pub session: AnalysisSession,
}

/// Installs execution only. Panel/file/document creation remains host-owned.
pub struct EditorAnalysisPlugin;
impl Plugin for EditorAnalysisPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update_document_analysis);
    }
}
pub fn analysis_for_panel(world: &World, panel: Entity) -> Option<Arc<Observation>> {
    world.get::<WorkbenchEditor>(panel)?;
    world.get::<PanelInstance>(panel)?;
    let document = world.get::<DocumentReference>(panel)?.document();
    let revision = inspect_document(world, document).ok()?.revision;
    let language = bedtime_treesitter::language_name(&world.get::<CodeLanguage>(document.0)?.0)?;
    world
        .get::<DocumentAnalysis>(document.0)?
        .session
        .observation()
        .filter(|o| o.key.revision == revision && o.language == language)
        .cloned()
}
/// Eligibility includes a supported pending observation, so source identity
/// survives refreshes while the viewer displays no stale clickable entries.
pub fn code_document_for_panel(world: &World, panel: Entity) -> Option<DocumentId> {
    world.get::<WorkbenchEditor>(panel)?;
    world.get::<PanelInstance>(panel)?;
    let document = world.get::<DocumentReference>(panel)?.document();
    cast_document::<TextBuffer>(world, document).ok()?;
    bedtime_treesitter::language_name(&world.get::<CodeLanguage>(document.0)?.0)?;
    Some(document)
}
pub fn update_document_analysis(world: &mut World) {
    let consumers: HashSet<DocumentId> = world
        .query_filtered::<&DocumentReference, With<WorkbenchEditor>>()
        .iter(world)
        .map(DocumentReference::document)
        .collect();
    let retired: Vec<Entity> = world
        .query_filtered::<Entity, With<DocumentAnalysis>>()
        .iter(world)
        .filter(|entity| {
            !consumers.contains(&DocumentId(*entity))
                || world.get::<CodeLanguage>(*entity).is_none()
        })
        .collect();
    for entity in retired {
        world.entity_mut(entity).remove::<DocumentAnalysis>();
    }
    for document in consumers {
        let Some(language) = world.get::<CodeLanguage>(document.0).map(|l| l.0.clone()) else {
            continue;
        };
        let Ok(handle) = cast_document::<TextBuffer>(world, document) else {
            continue;
        };
        let Ok(info) = inspect_document(world, document) else {
            continue;
        };
        if world.get::<DocumentAnalysis>(document.0).is_none() {
            let (session, worker) = AnalysisSession::with_worker();
            let task = Workbench::for_entity(world, document.0).and_then(|mut w| {
                w.work()
                    .spawn_task(Some(document.0), "Code analysis", move |cancel| {
                        worker.run(|| cancel.is_cancelled());
                        Some(())
                    })
            });
            match task {
                Ok(_) => {
                    world
                        .entity_mut(document.0)
                        .insert(DocumentAnalysis { session });
                }
                Err(_) => continue,
            }
        }
        let snapshot = TextSnapshot {
            text: read_document(world, handle).unwrap().clone(),
            revision: info.revision,
        };
        let base = world
            .get::<DocumentAnalysis>(document.0)
            .unwrap()
            .session
            .key()
            .and_then(|key| {
                document_changes_since(world, handle, key.revision)
                    .ok()
                    .flatten()
                    .filter(|commits| commits.iter().all(|c| c.changes.is_some()))
                    .map(|_| key.revision)
            });
        let mut analysis = world.get_mut::<DocumentAnalysis>(document.0).unwrap();
        analysis
            .session
            .request(Arc::new(snapshot.text), snapshot.revision, &language, base);
        analysis.session.poll();
    }
}
