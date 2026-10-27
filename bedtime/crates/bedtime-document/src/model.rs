/// What a document is made of. Changes are pure and return their inverse, so
/// history can undo any model the same way.
pub trait DocumentModel: Clone + PartialEq + Send + Sync + 'static {
    type Change: Clone + Send + Sync + 'static;
    fn apply(&mut self, change: &Self::Change) -> Result<Self::Change, String>;
}
