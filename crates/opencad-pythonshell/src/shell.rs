//! egui/eframe UI controller for the Python Shell plugin.
//!
//! The controller runs on the plugin main thread. It owns the crossbeam
//! receiver for [`UiRequest`]s and manages one deferred egui viewport per
//! active Python Shell session.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crossbeam_channel::Receiver;
use egui::{Context, Key, TextEdit, Vec2, ViewportCommand, ViewportId};

use crate::host_proxy::HostProxy;
use crate::interpreter::{create_interpreter, Interpreter};
use crate::UiRequest;

/// Marker emitted by the Python wrapper when it catches `SystemExit`.
const EXIT_MARKER: &str = "<<<PYSHELL_EXIT>>>";

/// One active Python Shell window.
struct ViewportState {
    tab: usize,
    session_id: String,
    proxy: HostProxy,
    interpreter: Arc<dyn Interpreter>,
    output: String,
    input: String,
    /// Set by the viewport UI when the user clicks the window close button.
    close_requested: bool,
}

type SharedViewportState = Arc<Mutex<ViewportState>>;

/// Main egui application.
pub struct ShellApp {
    rx: Receiver<UiRequest>,
    viewports: HashMap<ViewportId, SharedViewportState>,
}

impl ShellApp {
    /// Create the app from a receiver of UI requests.
    pub fn new(rx: Receiver<UiRequest>) -> Self {
        Self {
            rx,
            viewports: HashMap::new(),
        }
    }

    fn handle_request(&mut self, ctx: &Context, req: UiRequest) {
        match req {
            UiRequest::Open {
                tab,
                session_id,
                proxy,
            } => {
                let (interpreter, _output_rx) = create_interpreter();
                let vp_id = viewport_id(tab, &session_id);
                self.viewports.insert(
                    vp_id,
                    Arc::new(Mutex::new(ViewportState {
                        tab,
                        session_id,
                        proxy,
                        interpreter,
                        output: String::new(),
                        input: String::new(),
                        close_requested: false,
                    })),
                );
            }
            UiRequest::Raise { tab } => {
                if let Some(vp_id) = self
                    .viewports
                    .iter()
                    .find(|(_, s)| s.lock().unwrap().tab == tab)
                    .map(|(id, _)| *id)
                {
                    ctx.send_viewport_cmd_to(vp_id, ViewportCommand::Focus);
                }
            }
            UiRequest::Close { session_id } => {
                if let Some(vp_id) = self
                    .viewports
                    .iter()
                    .find(|(_, s)| s.lock().unwrap().session_id == session_id)
                    .map(|(id, _)| *id)
                {
                    ctx.send_viewport_cmd_to(vp_id, ViewportCommand::Close);
                    self.viewports.remove(&vp_id);
                }
            }
            UiRequest::Shutdown => {
                for vp_id in self.viewports.keys().copied().collect::<Vec<_>>() {
                    ctx.send_viewport_cmd_to(vp_id, ViewportCommand::Close);
                }
                self.viewports.clear();
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }
    }

    /// Close a single viewport, notify the host, and clean up the session map.
    fn close_viewport(&mut self, ctx: &Context, vp_id: ViewportId) {
        let Some(state) = self.viewports.remove(&vp_id) else {
            return;
        };
        let state = state.lock().unwrap();
        if let Err(e) = state.proxy.end_session() {
            eprintln!("[pyshell] failed to end async session {}: {e:?}", state.session_id);
        }
        crate::remove_session_by_id(&state.session_id);
        ctx.send_viewport_cmd_to(vp_id, ViewportCommand::Close);
    }

    /// Drain interpreter output and return viewports that have exited.
    fn drain_output_and_find_dead(&self) -> Vec<ViewportId> {
        let mut dead_viewports = Vec::new();
        for (vp_id, state) in &self.viewports {
            let mut state = state.lock().unwrap();
            let mut saw_exit_marker = false;
            for line in state.interpreter.drain_output() {
                if line.contains(EXIT_MARKER) {
                    saw_exit_marker = true;
                }
                state.output.push_str(&line);
                state.output.push('\n');
            }
            if saw_exit_marker || !state.interpreter.is_alive() {
                dead_viewports.push(*vp_id);
            }
        }
        dead_viewports
    }
}

impl eframe::App for ShellApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Drain incoming requests from the dispatch thread.
        while let Ok(req) = self.rx.try_recv() {
            self.handle_request(ctx, req);
        }

        // Pull interpreter output and close viewports whose interpreter exited.
        let dead_viewports = self.drain_output_and_find_dead();
        for vp_id in dead_viewports {
            eprintln!("[pyshell] interpreter exit detected, closing viewport {vp_id:?}");
            self.close_viewport(ctx, vp_id);
        }

        // Close viewports whose close button was pressed.
        let to_remove: Vec<ViewportId> = self
            .viewports
            .iter()
            .filter(|(_, s)| s.lock().unwrap().close_requested)
            .map(|(id, _)| *id)
            .collect();
        for vp_id in to_remove {
            self.close_viewport(ctx, vp_id);
        }

        // Draw the small controller/root window.
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(format!("Python Shell controller — {} session(s)", self.viewports.len()));
            if ui.button("Shut down").clicked() {
                // Notify the host via the first available proxy.
                if let Some(state) = self.viewports.values().next() {
                    let state = state.lock().unwrap();
                    if let Err(e) = state.proxy.push_info("Python Shell controller shut down") {
                        eprintln!("[pyshell] failed to push shutdown info: {e:?}");
                    }
                }
                for vp_id in self.viewports.keys().copied().collect::<Vec<_>>() {
                    self.close_viewport(ctx, vp_id);
                }
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        });

        // Schedule each deferred viewport for this frame.
        for (vp_id, state) in &self.viewports {
            let vp_id = *vp_id;
            let state = Arc::clone(state);
            let title = {
                let s = state.lock().unwrap();
                format!("Python Shell {} (tab {})", s.session_id, s.tab)
            };
            let builder = egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([640.0, 480.0]);

            ctx.show_viewport_deferred(vp_id, builder, move |ctx, _class| {
                draw_viewport(ctx, vp_id, &state);
            });
        }
    }
}

fn draw_viewport(ctx: &Context, _vp_id: ViewportId, state: &SharedViewportState) {
    let mut state = state.lock().unwrap();

    // Detect the user closing the window.
    ctx.input(|i| {
        if i.viewport().close_requested() {
            state.close_requested = true;
        }
    });

    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical(|ui| {
            // Output area takes all available space except the input line.
            let output_height = ui.available_height() - 32.0;
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), output_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add(
                            TextEdit::multiline(&mut state.output)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
                },
            );

            // One-line input at the bottom.
            ui.horizontal(|ui| {
                ui.label(">>>");
                let response = ui.add(
                    TextEdit::singleline(&mut state.input)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
                if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    let line = state.input.clone();
                    state.output.push_str(">>> ");
                    state.output.push_str(&line);
                    state.output.push('\n');
                    state.interpreter.eval(&line);
                    state.input.clear();
                    response.request_focus();
                }
            });
        });
    });
}

fn viewport_id(tab: usize, session_id: &str) -> ViewportId {
    ViewportId::from_hash_of(&(tab, session_id))
}

/// Run the egui event loop. Blocks until the controller is shut down.
pub fn run_controller(rx: Receiver<UiRequest>) -> Result<(), Box<dyn std::error::Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Python Shell Controller")
            .with_inner_size([320.0, 120.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Python Shell Controller",
        options,
        Box::new(|_cc| Ok(Box::new(ShellApp::new(rx)))),
    )
    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ocs_plugin_api::host::{
        AsyncSessionError, AsyncSessionHandle, DocumentReader, ReaderEntity,
    };
    use ocs_plugin_api::ipc::protocol::{PluginRequest, PluginResponse};
    use ocs_plugin_api::shm::DocumentViewInfo;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct EmptyReader;
    impl DocumentReader for EmptyReader {
        fn entity_count(&self) -> usize {
            0
        }
        fn for_each_entity(&self, _f: &mut dyn FnMut(ReaderEntity<'_>)) {}
        fn layer_name(&self, _handle: acadrust::Handle) -> Option<&str> {
            None
        }
        fn app_id_name(&self, _handle: acadrust::Handle) -> Option<&str> {
            None
        }
    }

    struct MockHandle {
        requests: Arc<Mutex<Vec<PluginRequest>>>,
    }

    impl MockHandle {
        fn new() -> (Self, Arc<Mutex<Vec<PluginRequest>>>) {
            let requests = Arc::new(Mutex::new(Vec::new()));
            (Self { requests: Arc::clone(&requests) }, requests)
        }
    }

    impl AsyncSessionHandle for MockHandle {
        fn tab_index(&self) -> usize {
            0
        }
        fn request(
            &self,
            req: PluginRequest,
        ) -> Result<PluginResponse, AsyncSessionError> {
            let response = match &req {
                PluginRequest::EndAsyncSession { .. } | PluginRequest::PushInfo { .. } => {
                    Ok(PluginResponse::Ok)
                }
                _ => Ok(PluginResponse::Error(format!("unmocked: {req:?}"))),
            };
            self.requests.lock().unwrap().push(req);
            response
        }
        fn document_reader(&self) -> Box<dyn DocumentReader + 'static> {
            Box::new(EmptyReader)
        }
        fn document_view(&self) -> Option<DocumentViewInfo> {
            None
        }
    }

    struct MockInterpreter {
        pending: Mutex<Vec<String>>,
        alive: AtomicBool,
    }

    impl MockInterpreter {
        fn new() -> Self {
            Self {
                pending: Mutex::new(Vec::new()),
                alive: AtomicBool::new(true),
            }
        }

        fn push_output(&self, line: &str) {
            self.pending.lock().unwrap().push(line.to_string());
        }

        fn set_alive(&self, alive: bool) {
            self.alive.store(alive, Ordering::SeqCst);
        }
    }

    impl Interpreter for MockInterpreter {
        fn eval(&self, _line: &str) {}

        fn drain_output(&self) -> Vec<String> {
            std::mem::take(&mut *self.pending.lock().unwrap())
        }

        fn is_alive(&self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
    }

    fn insert_test_viewport(
        app: &mut ShellApp,
        tab: usize,
        session_id: &str,
    ) -> (ViewportId, Arc<Mutex<Vec<PluginRequest>>>, Arc<MockInterpreter>) {
        let (handle, requests) = MockHandle::new();
        let interpreter = Arc::new(MockInterpreter::new());
        let proxy = HostProxy::new(Box::new(handle));
        let vp_id = viewport_id(tab, session_id);
        app.viewports.insert(
            vp_id,
            Arc::new(Mutex::new(ViewportState {
                tab,
                session_id: session_id.to_string(),
                proxy,
                interpreter: interpreter.clone(),
                output: String::new(),
                input: String::new(),
                close_requested: false,
            })),
        );
        (vp_id, requests, interpreter)
    }

    #[test]
    fn viewport_id_is_stable() {
        let a = viewport_id(1, "s-1");
        let b = viewport_id(1, "s-1");
        let c = viewport_id(2, "s-1");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn exit_marker_removes_viewport_and_ends_session() {
        let (_tx, rx) = crossbeam_channel::unbounded();
        let mut app = ShellApp::new(rx);
        let (vp_id, requests, interpreter) = insert_test_viewport(&mut app, 3, "s-3");
        crate::SESSIONS.lock().unwrap().insert(3, "s-3".to_string());

        interpreter.push_output(EXIT_MARKER);
        let ctx = egui::Context::default();
        let dead = app.drain_output_and_find_dead();
        assert_eq!(dead, vec![vp_id]);
        for id in dead {
            app.close_viewport(&ctx, id);
        }

        assert!(!app.viewports.contains_key(&vp_id));
        assert!(requests.lock().unwrap().iter().any(|r| matches!(r, PluginRequest::EndAsyncSession { .. })));
        assert!(!crate::has_session_for_tab(3));
    }

    #[test]
    fn interpreter_death_removes_viewport_and_ends_session() {
        let (_tx, rx) = crossbeam_channel::unbounded();
        let mut app = ShellApp::new(rx);
        let (vp_id, requests, interpreter) = insert_test_viewport(&mut app, 4, "s-4");
        crate::SESSIONS.lock().unwrap().insert(4, "s-4".to_string());

        interpreter.set_alive(false);
        let ctx = egui::Context::default();
        let dead = app.drain_output_and_find_dead();
        assert_eq!(dead, vec![vp_id]);
        for id in dead {
            app.close_viewport(&ctx, id);
        }

        assert!(!app.viewports.contains_key(&vp_id));
        assert!(requests.lock().unwrap().iter().any(|r| matches!(r, PluginRequest::EndAsyncSession { .. })));
        assert!(!crate::has_session_for_tab(4));
    }

    #[test]
    fn close_requested_removes_viewport_and_ends_session() {
        let (_tx, rx) = crossbeam_channel::unbounded();
        let mut app = ShellApp::new(rx);
        let (vp_id, requests, _interpreter) = insert_test_viewport(&mut app, 5, "s-5");
        crate::SESSIONS.lock().unwrap().insert(5, "s-5".to_string());

        {
            let mut state = app.viewports[&vp_id].lock().unwrap();
            state.close_requested = true;
        }

        let ctx = egui::Context::default();
        app.close_viewport(&ctx, vp_id);

        assert!(!app.viewports.contains_key(&vp_id));
        assert!(requests.lock().unwrap().iter().any(|r| matches!(r, PluginRequest::EndAsyncSession { .. })));
        assert!(!crate::has_session_for_tab(5));
    }
}
