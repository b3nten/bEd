//! Regression fixtures for controlled saves and preloaded models.
//! This module is excluded from ordinary builds; it is not a document opening API.
use crate::*;
use bevy::prelude::*;

pub struct DocumentSaveBinding<M: DocumentModel>(pub(crate) crate::documents::SaveBinding<M>);
impl<M: DocumentModel> Clone for DocumentSaveBinding<M> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<M: DocumentModel> DocumentSaveBinding<M> {
    pub fn new(
        destination: Option<String>,
        callback: impl Fn(DocumentSaveSnapshot<M>) -> Result<Option<SourceVersion>, DocumentSaveError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(crate::documents::SaveBinding::new(destination, callback))
    }
}
pub fn open_memory_document<M: DocumentModel>(
    world: &mut World,
    model: M,
    options: DocumentOptions,
) -> Result<DocumentHandle<M>, DocumentError> {
    crate::documents::create_memory_snapshot(world, model, options)
}
pub fn bind_document_save<M: DocumentModel>(
    world: &mut World,
    handle: DocumentHandle<M>,
    binding: DocumentSaveBinding<M>,
) -> Result<(), DocumentError> {
    crate::documents::install_save_binding(world, handle, binding.0)
}
pub fn unbind_document_save(world: &mut World, id: DocumentId) -> Result<(), DocumentError> {
    crate::documents::remove_save_binding(world, id)
}
