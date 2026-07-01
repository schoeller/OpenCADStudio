//! Process manager for out-of-process plugins.

use std::path::Path;
use std::sync::Arc;

use crate::host::HostApi;
use crate::ipc::server::handle_plugin_request;
use crate::process::{AsyncInbound, PluginError, PluginProcess};
use crate::ribbon::owned::{to_shared_module, SharedCadModule};

/// Owner of every spawned plugin process.
pub struct PluginManager {
    plugins: Vec<LoadedPlugin>,
}

struct LoadedPlugin {
    process: Arc<PluginProcess>,
    module: SharedCadModule,
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of [`PluginManager::dispatch`].
#[derive(Default)]
pub struct DispatchResult {
    /// A plugin handled the command.
    pub handled: bool,
    /// An interactive command was started by a plugin.
    pub started: Option<(Arc<PluginProcess>, u64)>,
    /// Plugins whose process died before or during dispatch.
    pub dead_plugins: Vec<String>,
    /// Plugins that returned an error while trying to handle the command.
    pub errors: Vec<(String, String)>,
}

impl PluginManager {
    /// Create an empty manager.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Spawn `cdylib_path` as a separate plugin process, build its ribbon
    /// module, and store it. Returns the plugin id on success.
    pub fn load(
        &mut self,
        cdylib_path: &Path,
        host: &mut dyn HostApi,
    ) -> Result<String, PluginError> {
        let process = PluginProcess::spawn(cdylib_path, host)?;
        let id = process.id().to_string();
        let name = process.manifest().name.clone();
        let module = to_shared_module(id.clone(), name, process.ribbon().to_vec());
        self.plugins.push(LoadedPlugin {
            process: Arc::new(process),
            module,
        });
        Ok(id)
    }

    /// Ribbon modules for alive, non-disabled plugins, sorted by `ribbon_order`.
    pub fn ribbon_modules<F: Fn(&str) -> bool>(
        &self,
        is_disabled: F,
    ) -> Vec<(i32, SharedCadModule)> {
        let mut out: Vec<(i32, SharedCadModule)> = self
            .plugins
            .iter()
            .filter(|p| !is_disabled(p.process.id()) && p.process.is_alive())
            .map(|p| (p.process.manifest().ribbon_order, p.module.clone()))
            .collect();
        out.sort_by_key(|(order, _)| *order);
        out
    }

    /// Dispatch `cmd` to each plugin until one handles it.
    ///
    /// `is_disabled` is called for each plugin id so the host can filter
    /// disabled plugins without exposing its set type to the crate.
    ///
    /// The list of processes is cloned before iterating so that a nested
    /// `HostApi` call (e.g. `open_panel`) can re-borrow the manager without
    /// tripping the thread-local `RefCell`.
    pub fn dispatch<F: Fn(&str) -> bool>(
        &self,
        host: &mut dyn HostApi,
        cmd: &str,
        is_disabled: F,
    ) -> DispatchResult {
        let mut result = DispatchResult::default();
        // Snapshot the alive, non-disabled processes so we can release the
        // manager borrow while calling into plugin code. This prevents a panic
        // when a plugin's dispatch handler calls back into the manager (e.g.
        // `HostApi::open_panel`).
        let processes: Vec<(String, Arc<PluginProcess>)> = self
            .plugins
            .iter()
            .filter(|p| !is_disabled(p.process.id()) && p.process.is_alive())
            .map(|p| (p.process.id().to_string(), Arc::clone(&p.process)))
            .collect();

        for (id, process) in processes {
            if is_disabled(&id) {
                continue;
            }
            if !process.is_alive() {
                result.dead_plugins.push(id);
                continue;
            }
            host.set_current_process(Some(Arc::clone(&process)));
            let mut on_start = |command_id: u64| {
                result.started = Some((Arc::clone(&process), command_id));
            };
            match process.dispatch(host, cmd, &mut on_start) {
                Ok(true) => {
                    result.handled = true;
                    host.set_current_process(None);
                    return result;
                }
                Ok(false) => {}
                Err(e) => result.errors.push((id, e.to_string())),
            }
        }
        host.set_current_process(None);
        result
    }

    /// Plugin ids currently loaded.
    pub fn ids(&self) -> Vec<String> {
        self.plugins
            .iter()
            .map(|p| p.process.id().to_string())
            .collect()
    }

    /// Command names advertised by every alive, non-disabled plugin: each
    /// ribbon tool's command id plus the manifest's `command_prefixes`. The
    /// host merges these into command-line autocomplete so plugin commands are
    /// discoverable by typing (#272).
    pub fn command_names<F: Fn(&str) -> bool>(&self, is_disabled: F) -> Vec<String> {
        let mut out = Vec::new();
        for p in &self.plugins {
            if is_disabled(p.process.id()) || !p.process.is_alive() {
                continue;
            }
            for group in p.process.ribbon() {
                out.append(&mut group.command_ids());
            }
            out.extend(p.process.manifest().command_prefixes.iter().cloned());
        }
        out
    }

    /// Look up a loaded process by plugin id.
    pub fn process(&self, id: &str) -> Option<Arc<PluginProcess>> {
        self.plugins
            .iter()
            .find(|p| p.process.id() == id)
            .map(|p| Arc::clone(&p.process))
    }

    /// Drain all queued async messages from every alive plugin process,
    /// applying `PluginRequest` items to `host` and returning any `PluginAsync`
    /// UI events to the caller.  Request responses are sent back to the plugin
    /// so that async event handlers can perform synchronous host API calls.
    pub fn drain_async_events(
        &self,
        host: &mut dyn HostApi,
    ) -> Vec<crate::ipc::protocol::PluginAsync> {
        let mut events = Vec::new();
        for plugin in &self.plugins {
            if !plugin.process.is_alive() {
                continue;
            }
            host.set_current_process(Some(Arc::clone(&plugin.process)));
            let mut on_start = |_command_id: u64| {};
            for msg in plugin.process.drain_async() {
                match msg {
                    AsyncInbound::Event(event) => events.push(event),
                    AsyncInbound::Request(req) => {
                        let resp = handle_plugin_request(host, req, &mut on_start);
                        let _ = plugin.process.send_async_response(resp);
                    }
                }
            }
        }
        host.set_current_process(None);
        events
    }

    /// Begin asynchronous shutdown of every plugin process.
    ///
    /// Kills every child synchronously on the calling thread and moves the
    /// blocking `wait()` calls into a single detached reaper thread, so host
    /// shutdown is fast regardless of how many plugins are loaded.
    pub fn shutdown_all(&mut self) {
        let plugins = std::mem::take(&mut self.plugins);
        for p in plugins {
            p.process.shutdown();
        }
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

#[cfg(all(test, feature = "host"))]
mod tests {
    use super::*;

    #[test]
    fn empty_manager_has_no_ribbon_modules() {
        let manager = PluginManager::new();
        assert!(manager.ribbon_modules(|_| false).is_empty());
        assert!(manager.ids().is_empty());
    }

    #[test]
    fn dispatch_with_no_plugins_is_not_handled() {
        struct EmptyReader;
        impl crate::host::DocumentReader for EmptyReader {
            fn entity_count(&self) -> usize {
                0
            }
            fn for_each_entity(&self, _f: &mut dyn FnMut(crate::host::ReaderEntity<'_>)) {}
            fn layer_name(&self, _handle: acadrust::Handle) -> Option<&str> {
                None
            }
            fn app_id_name(&self, _handle: acadrust::Handle) -> Option<&str> {
                None
            }
        }

        struct DummyHost;
        impl HostApi for DummyHost {
            fn tab_index(&self) -> usize {
                0
            }
            fn document(&self) -> &acadrust::CadDocument {
                panic!("not used")
            }
            fn document_mut(&mut self) -> &mut acadrust::CadDocument {
                panic!("not used")
            }
            fn document_reader(&self) -> Box<dyn crate::host::DocumentReader + '_> {
                Box::new(EmptyReader)
            }
            fn add_entity(&mut self, _entity: acadrust::EntityType) -> acadrust::Handle {
                panic!("not used")
            }
            fn bump_geometry(&mut self) {}
            fn read_record(
                &self,
                _handle: acadrust::Handle,
                _app_name: &str,
            ) -> Option<&acadrust::xdata::ExtendedDataRecord> {
                None
            }
            fn write_record(
                &mut self,
                _handle: acadrust::Handle,
                _record: acadrust::xdata::ExtendedDataRecord,
            ) -> bool {
                false
            }
            fn remove_record(&mut self, _handle: acadrust::Handle, _app_name: &str) -> bool {
                false
            }
            fn push_undo(&mut self, _label: &str) {}
            fn set_dirty(&mut self) {}
            fn push_info(&mut self, _msg: &str) {}
            fn push_output(&mut self, _msg: &str) {}
            fn push_error(&mut self, _msg: &str) {}
            fn start_interactive(&mut self, _command: Box<dyn crate::host::InteractiveCommand>) {}
            fn plugin_state_any(
                &self,
                _plugin_id: &str,
            ) -> Option<&(dyn std::any::Any + Send + Sync)> {
                None
            }
            fn plugin_state_any_mut(
                &mut self,
                _plugin_id: &str,
            ) -> Option<&mut (dyn std::any::Any + Send + Sync)> {
                None
            }
            fn ensure_plugin_state_any(
                &mut self,
                _plugin_id: &'static str,
                _init: &mut dyn FnMut() -> Box<dyn std::any::Any + Send + Sync>,
            ) -> &mut (dyn std::any::Any + Send + Sync) {
                panic!("not used")
            }
        }

        let manager = PluginManager::new();
        let mut host = DummyHost;
        let result = manager.dispatch(&mut host, "FOO", |_| false);
        assert!(!result.handled);
        assert!(result.started.is_none());
    }
}
