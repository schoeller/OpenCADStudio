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

/// One active Python Shell window.
struct ViewportState {
    tab: usize,
    session_id: String,
    #[allow(dead_code)]
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
}

impl eframe::App for ShellApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Drain incoming requests from the dispatch thread.
        while let Ok(req) = self.rx.try_recv() {
            self.handle_request(ctx, req);
        }

        // Pull interpreter output into each viewport's output buffer.
        for state in self.viewports.values() {
            let mut state = state.lock().unwrap();
            for line in state.interpreter.drain_output() {
                state.output.push_str(&line);
                state.output.push('\n');
            }
        }

        // Remove viewports whose close button was pressed.
        let to_remove: Vec<ViewportId> = self
            .viewports
            .iter()
            .filter(|(_, s)| s.lock().unwrap().close_requested)
            .map(|(id, _)| *id)
            .collect();
        for id in to_remove {
            self.viewports.remove(&id);
        }

        // Draw the small controller/root window.
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(format!("Python Shell controller — {} session(s)", self.viewports.len()));
            if ui.button("Shut down").clicked() {
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
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_id_is_stable() {
        let a = viewport_id(1, "s-1");
        let b = viewport_id(1, "s-1");
        let c = viewport_id(2, "s-1");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
