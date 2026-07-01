//! Integration tests for `ocs_python_lsp`.
//!
//! Python-dependent tests that spawn a worker process are removed because the
//! environment's Python interpreter opens an editor (Zed) when invoked, causing
//! hangs and spurious failures.

use acadrust::xdata::ExtendedDataRecord;
use acadrust::{CadDocument, EntityType, Handle};
use ocs_plugin_api::host::{BuiltinPlugin, CadDocumentReader, DocumentReader, HostApi, InteractiveCommand};
use ocs_plugin_api::ipc::protocol::PluginAsync;
use std::any::Any;
use std::sync::Mutex;

struct MockHost {
    doc: CadDocument,
    tab: usize,
    infos: Mutex<Vec<String>>,
    errors: Mutex<Vec<String>>,
    async_events: Mutex<Vec<PluginAsync>>,
    set_active_tab_calls: Mutex<Vec<(usize, usize)>>,
}

impl MockHost {
    fn new() -> Self {
        Self {
            doc: CadDocument::default(),
            tab: 0,
            infos: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
            async_events: Mutex::new(Vec::new()),
            set_active_tab_calls: Mutex::new(Vec::new()),
        }
    }
}

impl HostApi for MockHost {
    fn tab_index(&self) -> usize {
        self.tab
    }
    fn document(&self) -> &CadDocument {
        &self.doc
    }
    fn document_mut(&mut self) -> &mut CadDocument {
        &mut self.doc
    }
    fn document_reader(&self) -> Box<dyn DocumentReader + '_> {
        Box::new(CadDocumentReader(&self.doc))
    }
    fn add_entity(&mut self, entity: EntityType) -> Handle {
        self.doc.add_entity(entity).unwrap_or_default()
    }
    fn remove_entity(&mut self, handle: Handle) -> Option<EntityType> {
        self.doc.remove_entity(handle)
    }
    fn bump_geometry(&mut self) {}
    fn read_record(&self, handle: Handle, app_name: &str) -> Option<&ExtendedDataRecord> {
        self.doc
            .get_entity(handle)?
            .common()
            .extended_data
            .get_record(app_name)
    }
    fn write_record(&mut self, handle: Handle, record: ExtendedDataRecord) -> bool {
        let Some(entity) = self.doc.get_entity_mut(handle) else {
            return false;
        };
        let app = record.application_name.clone();
        let xd = &mut entity.common_mut().extended_data;
        let kept: Vec<_> = xd
            .records()
            .iter()
            .filter(|r| r.application_name != app)
            .cloned()
            .collect();
        xd.clear();
        for r in kept {
            xd.add_record(r);
        }
        xd.add_record(record);
        true
    }
    fn remove_record(&mut self, handle: Handle, app_name: &str) -> bool {
        let Some(entity) = self.doc.get_entity_mut(handle) else {
            return false;
        };
        let xd = &mut entity.common_mut().extended_data;
        let kept: Vec<_> = xd
            .records()
            .iter()
            .filter(|r| r.application_name != app_name)
            .cloned()
            .collect();
        if kept.len() == xd.records().len() {
            return false;
        }
        xd.clear();
        for r in kept {
            xd.add_record(r);
        }
        true
    }
    fn push_undo(&mut self, _label: &str) {}
    fn set_dirty(&mut self) {}
    fn push_info(&mut self, msg: &str) {
        self.infos.lock().unwrap().push(msg.to_string());
    }
    fn push_output(&mut self, _msg: &str) {}
    fn push_error(&mut self, msg: &str) {
        self.errors.lock().unwrap().push(msg.to_string());
    }
    fn start_interactive(&mut self, _command: Box<dyn InteractiveCommand>) {}
    fn send_async(&mut self, event: PluginAsync) {
        self.async_events.lock().unwrap().push(event);
    }
    fn plugin_state_any(&self, _plugin_id: &str) -> Option<&(dyn Any + Send + Sync)> {
        None
    }
    fn plugin_state_any_mut(&mut self, _plugin_id: &str) -> Option<&mut (dyn Any + Send + Sync)> {
        None
    }
    fn ensure_plugin_state_any(
        &mut self,
        _plugin_id: &'static str,
        _init: &mut dyn FnMut() -> Box<dyn Any + Send + Sync>,
    ) -> &mut (dyn Any + Send + Sync) {
        panic!("not used")
    }
    fn set_active_tab(&mut self, tab: usize) -> Result<(), String> {
        self.set_active_tab_calls.lock().unwrap().push((self.tab, tab));
        self.tab = tab;
        Ok(())
    }
}

#[test]
fn ribbon_exposes_python_tab_and_pythonedit() {
    let plugin = ocs_python_lsp::PythonLspPlugin::new();
    assert_eq!(plugin.manifest().id, "ocs.python_lsp");
    let ribbon = plugin.ribbon();
    let groups = ribbon.ribbon_groups();
    assert!(!groups.is_empty());
    assert_eq!(groups[0].title, "Python");
    let tools = &groups[0].tools;
    assert_eq!(tools.len(), 1);
    match &tools[0] {
        ocs_plugin_api::ribbon::RibbonItem::Tool(t) => {
            assert_eq!(t.id, "PYTHONEDIT");
        }
        _ => panic!("expected tool"),
    }
}

#[test]
fn set_active_tab_round_trip() {
    let mut host = MockHost::new();
    host.tab = 3;
    assert!(host.set_active_tab(7).is_ok());
    assert_eq!(host.tab, 7);
    let calls = host.set_active_tab_calls.lock().unwrap();
    assert_eq!(calls.as_slice(), &[(3, 7)]);
}
