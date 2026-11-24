/// Editing a value the panel can only read.
///
/// A panel draws from read-only state and queues its changes, but a control
/// that edits wants a `&mut` to what it edits. `edit` gives it one to a copy,
/// and returns the copy if the control changed it:
///
/// ```ignore
/// if let Some(url) = ui.edit(&state.url, |ui, url| ui.add(TextField::new(url))) {
///     panel.update(move |state| state.url = url);
/// }
/// ```
pub trait UiEdit {
    fn edit<T: Clone + PartialEq, R>(
        &mut self,
        value: &T,
        add: impl FnOnce(&mut egui::Ui, &mut T) -> R,
    ) -> Option<T>;
}

impl UiEdit for egui::Ui {
    fn edit<T: Clone + PartialEq, R>(
        &mut self,
        value: &T,
        add: impl FnOnce(&mut egui::Ui, &mut T) -> R,
    ) -> Option<T> {
        let mut edited = value.clone();
        add(self, &mut edited);
        (edited != *value).then_some(edited)
    }
}
