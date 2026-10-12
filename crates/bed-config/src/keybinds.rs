//! Application-owned overrides; defaults belong to action declarations.
use bed_keybindings::{Action, Overrides, Stroke};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub struct KeybindsManager {
    pub overrides: Overrides,
    path: PathBuf,
    disk_contents: Option<Vec<u8>>,
    pub reload_error: Option<String>,
}
impl KeybindsManager {
    pub fn new(config_dir: &Path) -> Self {
        Self {
            overrides: Overrides::default(),
            path: config_dir.join("keybindings.json"),
            disk_contents: None,
            reload_error: None,
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn load_keybinds(&mut self) -> io::Result<bool> {
        if !self.path.exists() {
            crate::persistence::write_atomic_new(
                &self.path,
                &serde_json::to_value(Overrides::default())?,
            )?;
        }
        // A malformed user file must not prevent startup or overwrite their edits.
        match self.check_keybinds_file() {
            Ok(changed) => Ok(changed),
            Err(error) => {
                self.reload_error = Some(error.to_string());
                Ok(false)
            }
        }
    }
    pub fn check_keybinds_file(&mut self) -> io::Result<bool> {
        let result: io::Result<bool> = (|| {
            let bytes = fs::read(&self.path)?;
            if self.disk_contents.as_ref() == Some(&bytes) {
                return Ok(false);
            }
            let overrides = Overrides::parse(&bytes).map_err(io::Error::other)?;
            self.overrides = overrides;
            self.disk_contents = Some(bytes);
            Ok(true)
        })();
        match &result {
            Ok(_) => self.reload_error = None,
            Err(error) => {
                self.reload_error = Some(format!(
                    "{}: {error}. Repair the file to resume saving.",
                    self.path.display()
                ))
            }
        }
        result.map_err(|e| {
            io::Error::new(
                e.kind(),
                self.reload_error.clone().unwrap_or_else(|| e.to_string()),
            )
        })
    }
    fn commit(&mut self, overrides: Overrides) -> io::Result<()> {
        self.check_keybinds_file()?;
        crate::persistence::write_atomic(&self.path, &serde_json::to_value(&overrides)?)?;
        self.disk_contents = Some(fs::read(&self.path)?);
        self.overrides = overrides;
        Ok(())
    }
    pub fn assign(
        &mut self,
        action: &Action,
        index: Option<usize>,
        stroke: Stroke,
    ) -> io::Result<()> {
        self.check_keybinds_file()?;
        let mut overrides = self.overrides.clone();
        overrides
            .replace(action, index, stroke)
            .map_err(io::Error::other)?;
        self.commit(overrides)
    }
    pub fn remove(&mut self, action: &Action, index: usize) -> io::Result<()> {
        self.check_keybinds_file()?;
        let mut overrides = self.overrides.clone();
        overrides.remove(action, index).map_err(io::Error::other)?;
        self.commit(overrides)
    }
    pub fn clear(&mut self, id: &str) -> io::Result<()> {
        self.check_keybinds_file()?;
        let mut overrides = self.overrides.clone();
        overrides.actions.insert(id.into(), Vec::new());
        self.commit(overrides)
    }
    pub fn restore(&mut self, id: &str) -> io::Result<()> {
        self.check_keybinds_file()?;
        let mut overrides = self.overrides.clone();
        overrides.actions.remove(id);
        self.commit(overrides)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_keybindings::{Bindings, Platform, Scope};
    #[test]
    fn saved_overrides_reload_and_malformed_edits_preserve_last_good_bindings() {
        let dir = crate::test_support::TempDir::new();
        let mut manager = KeybindsManager::new(&dir.path("config"));
        manager.load_keybinds().unwrap();
        let action = Action::new(
            "fixture.save",
            "Save",
            "fixture",
            Scope::Global,
            &["Primary+S"],
        );
        manager
            .assign(&action, None, "Ctrl+Alt+F12".parse().unwrap())
            .unwrap();
        let mut reloaded = KeybindsManager::new(&dir.path("config"));
        reloaded.load_keybinds().unwrap();
        assert_eq!(reloaded.overrides, manager.overrides);
        assert_eq!(
            Bindings::new(vec![action.clone()], &reloaded.overrides, Platform::Other)
                .for_action(&action.id)
                .len(),
            2
        );
        fs::write(manager.path(), "{ invalid").unwrap();
        assert!(manager.check_keybinds_file().is_err());
        assert_eq!(manager.overrides, reloaded.overrides);
        assert!(manager.clear(&action.id).is_err());
        assert_eq!(fs::read(manager.path()).unwrap(), b"{ invalid");
    }
}
