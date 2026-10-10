//! Exercise the registered Git module in the native host fixture.
use super::Workbench;
use std::io;

impl Workbench {
    pub fn git_smoke_setup(&mut self, path: &str) -> io::Result<()> {
        self.with_module_services(|modules, services| {
            let module = modules
                .instances
                .iter_mut()
                .find_map(|module| {
                    module
                        .as_any_mut()
                        .downcast_mut::<bed_module_git::GitModule>()
                })
                .ok_or_else(|| io::Error::other("Git module is unavailable"))?;
            module.smoke_setup(path, services, &mut modules.requests)
        })?;
        self.process_plugin_requests()?;
        // Keep the Changes tab selected in its sidebar dock while the working
        // comparison remains the active central tab.
        self.dispatch_command(bed_module_git::SHOW_COMMAND)?;
        Ok(())
    }

    pub fn git_smoke_ready(&self) -> io::Result<bool> {
        self.modules
            .instances
            .iter()
            .find_map(|module| module.as_any().downcast_ref::<bed_module_git::GitModule>())
            .ok_or_else(|| io::Error::other("Git module is unavailable"))?
            .smoke_ready()
    }

    pub fn git_smoke_focus_comparison(&mut self) {
        if let Some(index) = self.tabs.iter().position(|tab| {
            tab.panel.kind == bed_module_git::DIFF_PANEL_ID
                && tab.panel.instance.save_state()["side"] == "unstaged"
        }) {
            self.switch_to_tab(index);
        }
    }
}
