//! App-owned, serializable SQL observations for scripts and interactive clients.
use super::*;
use bedtime::automation::{
    AutomationError, ExtensionDefinition, ExtensionStart, register_extension,
};

pub(super) fn register(world: &mut World) {
    register_extension(
        world,
        ExtensionDefinition {
            name: "bedside.state",
            description: "Connections, query documents, results and persistence",
            arguments: json!({"type":"object"}),
            result: json!({"type":"object"}),
            submit: inspect,
        },
    )
    .expect("Bedside observations register once");
}
fn inspect(world: &mut World, _: Value) -> Result<ExtensionStart, AutomationError> {
    let panels = world
        .query::<(Entity, &bedtime::api::workbench::PanelFrontend)>()
        .iter(world)
        .map(|(entity, frontend)| (frontend.0, entity))
        .collect::<HashMap<_, _>>();
    let queries = world.resource::<QueryPanels>().0.iter().map(|(entity, state)| {
        let panel = panels.get(entity).copied().unwrap_or(*entity);
        let handle = bedtime::automation::panel_handle(world, panel)?;
        let model = bedtime_core::model::<QueryDocument>(world, state.document)
            .ok_or_else(|| AutomationError::new(bedtime::automation::ErrorKind::Failed,"query document is missing"))?;
        let dirty = bedtime_core::document_dirty(world, state.document)
            .map_err(|error| AutomationError::new(bedtime::automation::ErrorKind::Failed,error.to_string()))?;
        let rows = state.result.as_ref().map(|data| {
            data.rows.iter().map(|row| row.iter().map(bedtime_sqlite::Cell::display_text).collect::<Vec<_>>()).collect::<Vec<_>>()
        });
        Ok(json!({"panel":bedtime::automation::wire::panel_value(handle),"query":state.query.0,"document":state.document.0,"sql":model.sql,"dirty":dirty,"input_center":state.input_center,"input_active":state.input_active,"running":state.serial.is_some(),"error":state.error,"rows":rows}))
    }).collect::<Result<Vec<_>,AutomationError>>()?;
    let connections=world.resource::<Connections>().entries.iter().map(|connection|json!({"id":connection.record.id.0,"path":connection.record.path,"ready":connection.schema.is_some(),"error":connection.error})).collect::<Vec<_>>();
    let saved = world
        .resource::<MetadataStore>()
        .queries()
        .map_err(|e| AutomationError::new(bedtime::automation::ErrorKind::Failed, e.to_string()))?;
    Ok(ExtensionStart::Complete(
        json!({"connections":connections,"queries":queries,"data_panels":world.resource::<DataPanels>().0.values().map(|data|json!({"table":data.table,"ready":data.result.is_some(),"error":data.error})).collect::<Vec<_>>(),"saved_queries":saved.iter().map(|q|json!({"query":q.id.0,"sql":q.sql})).collect::<Vec<_>>(),"session_persisted":world.resource::<SessionPersistence>().previous.is_some()}),
    ))
}
