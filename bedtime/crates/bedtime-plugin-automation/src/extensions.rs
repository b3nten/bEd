//! Extensions submit to their existing owner and poll its receipt on the app thread.
use super::*;
use serde_json::json;
use std::collections::BTreeMap;

pub type ExtensionPoll =
    Box<dyn FnMut(&mut World) -> Option<Result<Value, AutomationError>> + Send + Sync>;
pub type ExtensionSubmit = fn(&mut World, Value) -> Result<ExtensionStart, AutomationError>;

pub enum ExtensionStart {
    Complete(Value),
    Pending(ExtensionPoll),
}

impl ExtensionStart {
    pub fn pending(
        poll: impl FnMut(&mut World) -> Option<Result<Value, AutomationError>> + Send + Sync + 'static,
    ) -> Self {
        Self::Pending(Box::new(poll))
    }
}

#[derive(Clone)]
pub struct ExtensionDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub arguments: Value,
    pub result: Value,
    pub submit: ExtensionSubmit,
}

#[derive(Resource, Default)]
pub(super) struct Extensions(BTreeMap<&'static str, ExtensionDefinition>);

pub fn register_extension(
    world: &mut World,
    definition: ExtensionDefinition,
) -> Result<(), AutomationError> {
    if !bedtime_api::workbench::valid_id(definition.name) || !definition.name.contains('.') {
        return Err(AutomationError::new(
            ErrorKind::InvalidRequest,
            "invalid extension name",
        ));
    }
    let mut registry = world.resource_mut::<Extensions>();
    if registry.0.contains_key(definition.name) {
        return Err(AutomationError::new(
            ErrorKind::InvalidRequest,
            "duplicate extension name",
        ));
    }
    registry.0.insert(definition.name, definition);
    Ok(())
}

pub(super) fn start(
    world: &mut World,
    name: &str,
    arguments: Value,
) -> Result<Pending, AutomationError> {
    let submit = world
        .resource::<Extensions>()
        .0
        .get(name)
        .ok_or_else(|| {
            AutomationError::new(ErrorKind::Unavailable, format!("unknown extension: {name}"))
        })?
        .submit;
    Ok(match submit(world, arguments)? {
        ExtensionStart::Complete(value) => {
            Pending::Immediate(Some(Ok(OperationValue::Json(value))))
        }
        ExtensionStart::Pending(poll) => Pending::Extension(poll),
    })
}

pub(super) fn describe(world: &World) -> Value {
    json!({"protocol": 1, "operations": wire::BUILTINS, "extensions": world.resource::<Extensions>().0.values().map(|d|
        json!({"name":d.name,"description":d.description,"arguments":d.arguments,"result":d.result})
    ).collect::<Vec<_>>()})
}
