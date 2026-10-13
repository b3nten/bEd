//! bEd's editable keybinding draft. The core preference owner persists it.
use bed_keybindings::{Overrides, ShortcutAction, Stroke};
use std::io;

#[derive(Default)]
pub struct KeybindingsDraft {
    pub overrides: Overrides,
}
impl KeybindingsDraft {
    pub fn assign(
        &mut self,
        action: &ShortcutAction,
        index: Option<usize>,
        stroke: Stroke,
    ) -> io::Result<()> {
        let mut overrides = self.overrides.clone();
        overrides
            .replace(action, index, stroke)
            .map_err(io::Error::other)?;
        self.overrides = overrides;
        Ok(())
    }
    pub fn remove(&mut self, action: &ShortcutAction, index: usize) -> io::Result<()> {
        let mut overrides = self.overrides.clone();
        overrides.remove(action, index).map_err(io::Error::other)?;
        self.overrides = overrides;
        Ok(())
    }
    pub fn clear(&mut self, id: &str) -> io::Result<()> {
        self.overrides.actions.insert(id.into(), Vec::new());
        Ok(())
    }
    pub fn restore(&mut self, id: &str) -> io::Result<()> {
        self.overrides.actions.remove(id);
        Ok(())
    }
}
