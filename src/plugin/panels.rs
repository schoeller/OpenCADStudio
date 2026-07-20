//! Host-side manager for plugin-rendered panels (API v3).
//!
//! `PanelManager` owns the open panels, stores their widgets, renders them as
//! in-window floating containers with drag/resize/snap support, and forwards
//! user actions to the owning plugin process as [`HostAsync::PanelEvent`] async
//! messages.
//!
//! All geometry, docking and drag/resize state lives in
//! [`ocs_plugin_api::panel::floating::FloatingPanel`] so that the host renderer
//! stays a thin, toolkit-specific layer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

fn panel_log(_msg: &str) {
    // Disabled in release builds to avoid synchronous file I/O on the hot
    // panel-event path. Re-enable locally when debugging async IPC.
}

use iced::widget::{button, container, mouse_area, scrollable, text, text_input, Column, Row, Space, Stack};
use iced::{Element, Length, Padding, Point, Task};

use ocs_plugin_api::ipc::protocol::HostAsync;
use ocs_plugin_api::panel::floating::{FloatingPanel, Point as FloatingPoint};
use ocs_plugin_api::panel::{DockZone, PanelDef, PanelError, PanelEvent, PanelHandle, Widget};
use ocs_plugin_api::process::{PluginError, PluginProcess};

use crate::app::Message;

/// Abstraction over the plugin process that owns a panel. In production this
/// is [`PluginProcess`]; tests provide a mock implementation.
pub trait PanelProcess: Send + Sync {
    /// Plugin identifier used for logging and routing.
    fn id(&self) -> &str;
    /// Send an asynchronous host event to the plugin process.
    fn send_async(&self, event: HostAsync) -> Result<(), PluginError>;
}

impl PanelProcess for PluginProcess {
    fn id(&self) -> &str {
        self.id()
    }

    fn send_async(&self, event: HostAsync) -> Result<(), PluginError> {
        self.send_async(event)
    }
}

/// Lifecycle event the host broadcasts to every panel-owning plugin process.
#[derive(Debug, Clone)]
pub enum DocumentEvent {
    /// The active document tab changed to this tab.
    Activated,
    /// The active document tab's content changed.
    Changed { version: u64 },
    /// A document tab was closed.
    Closed,
}

/// One open plugin panel and its current widget tree.
struct OpenPanel {
    handle: PanelHandle,
    panel_id: String,
    process_id: String,
    process: Arc<dyn PanelProcess>,
    def: PanelDef,
    widgets: Vec<Widget>,
    geometry: FloatingPanel,
}

/// Owns every open plugin panel and renders them as floating overlays.
pub struct PanelManager {
    next_handle: u64,
    panels: HashMap<PanelHandle, OpenPanel>,
    /// Render order / z-order: later handles are drawn on top.
    order: Vec<PanelHandle>,
    by_panel_id: HashMap<String, PanelHandle>,
    /// Panel definitions declared by loaded plugins but not yet opened.
    registered_defs: HashMap<String, PanelDef>,
    /// Last `geometry_epoch` broadcast as `DocumentEvent::Changed` per tab,
    /// so unchanged documents do not spam panel-owning plugins every frame.
    last_changed_version: RefCell<HashMap<usize, u64>>,
    /// Current text values for `TextInput` widgets, keyed by `(panel_id, widget_id)`.
    /// This lets the user keep typing while async `PanelUpdate`s arrive, without
    /// the UI being reset to a stale plugin-side value.
    edit_values: Rc<RefCell<HashMap<(String, String), String>>>,
    /// Last text value supplied by the plugin for each `TextInput`, used to
    /// detect intentional value changes (e.g. clear-after-submit) versus stale
    /// echoes.
    plugin_text_values: RefCell<HashMap<(String, String), String>>,
    /// Logical size of the main window, used for clamping and snap geometry.
    window_size: (f32, f32),
}

impl Default for PanelManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PanelManager {
    pub fn new() -> Self {
        Self {
            next_handle: 1,
            panels: HashMap::new(),
            order: Vec::new(),
            by_panel_id: HashMap::new(),
            registered_defs: HashMap::new(),
            last_changed_version: RefCell::new(HashMap::new()),
            edit_values: Rc::new(RefCell::new(HashMap::new())),
            plugin_text_values: RefCell::new(HashMap::new()),
            window_size: (1024.0, 768.0),
        }
    }

    /// Update the logical window size used for clamping and edge snapping.
    pub fn set_window_size(&mut self, width: f32, height: f32) {
        self.window_size = (width.max(1.0), height.max(1.0));
        let (ww, wh) = self.window_size;
        for panel in self.panels.values_mut() {
            panel.geometry.set_window_size(&panel.def, ww, wh);
        }
    }

    /// Returns whether at least one plugin panel is currently open.
    pub fn has_panels(&self) -> bool {
        !self.panels.is_empty()
    }

    /// Returns whether the user is currently dragging or resizing a panel,
    /// so the host can subscribe to global pointer events for the operation.
    pub fn is_dragging_or_resizing(&self) -> bool {
        self.panels
            .values()
            .any(|p| p.geometry.is_interacting())
    }

    /// Returns whether `cursor` lies inside any open panel's bounds. Used to
    /// suppress the model-space crosshair while the pointer is over a panel.
    pub fn cursor_over_panel(&self, cursor: Point) -> bool {
        self.panels.values().any(|p| {
            let g = &p.geometry;
            cursor.x >= g.x
                && cursor.x <= g.x + g.width
                && cursor.y >= g.y
                && cursor.y <= g.y + g.height
        })
    }

    /// Returns the bounding rectangles of all open panels in window coordinates.
    pub fn panel_rects(&self) -> Vec<iced::Rectangle> {
        self.panels
            .values()
            .map(|p| {
                let g = &p.geometry;
                iced::Rectangle {
                    x: g.x,
                    y: g.y,
                    width: g.width,
                    height: g.height,
                }
            })
            .collect()
    }

    /// Register a panel declaration from a loaded plugin. The panel is not
    /// opened until the plugin (or user) requests it.
    pub fn register_def(&mut self, def: &PanelDef) {
        self.registered_defs.insert(def.id.clone(), def.clone());
    }

    /// Open a panel owned by `process` and return a host-allocated handle.
    pub fn open(
        &mut self,
        process: Arc<dyn PanelProcess>,
        def: &PanelDef,
    ) -> Result<PanelHandle, PanelError> {
        if let Some(&handle) = self.by_panel_id.get(&def.id) {
            // Refresh an already-open panel: keep the handle but reset widgets.
            if let Some(panel) = self.panels.get_mut(&handle) {
                panel.def = def.clone();
                panel.widgets.clear();
                let (ww, wh) = self.window_size;
                panel.geometry.refresh_def(&panel.def, ww, wh);
            }
            return Ok(handle);
        }
        let handle = PanelHandle(self.next_handle);
        self.next_handle += 1;
        let (ww, wh) = self.window_size;
        let panel = OpenPanel {
            handle,
            panel_id: def.id.clone(),
            process_id: process.id().to_string(),
            process,
            def: def.clone(),
            widgets: Vec::new(),
            geometry: FloatingPanel::new(def, ww, wh),
        };
        self.panels.insert(handle, panel);
        self.by_panel_id.insert(def.id.clone(), handle);
        self.order.push(handle);
        Ok(handle)
    }

    /// Close an open panel.
    pub fn close(&mut self, handle: PanelHandle) -> Result<(), PanelError> {
        let panel = self
            .panels
            .remove(&handle)
            .ok_or(PanelError::UnknownHandle)?;
        self.by_panel_id.remove(&panel.panel_id);
        self.order.retain(|h| h != &handle);
        // Clean up per-widget text state so closed panels do not leak memory.
        let panel_id = panel.panel_id.clone();
        self.edit_values.borrow_mut().retain(|(pid, _), _| pid != &panel_id);
        self.plugin_text_values
            .borrow_mut()
            .retain(|(pid, _), _| pid != &panel_id);
        let _ = panel
            .process
            .send_async(HostAsync::PanelEvent {
                panel_id: panel.panel_id,
                event: PanelEvent::Closed,
            });
        Ok(())
    }

    /// Move an open panel to `(x, y)` and notify the plugin.
    pub fn move_panel(&mut self, handle: PanelHandle, x: f32, y: f32) -> Result<(), PanelError> {
        let panel = self.panels.get_mut(&handle).ok_or(PanelError::UnknownHandle)?;
        let (ww, wh) = self.window_size;
        let (x, y) = panel.geometry.move_to(x, y, ww, wh);
        Self::send_panel_event_by_panel(panel, PanelEvent::Moved { x, y });
        Ok(())
    }

    /// Resize an open panel and notify the plugin.
    pub fn resize_panel(
        &mut self,
        handle: PanelHandle,
        width: f32,
        height: f32,
    ) -> Result<(), PanelError> {
        let panel = self.panels.get_mut(&handle).ok_or(PanelError::UnknownHandle)?;
        let (ww, wh) = self.window_size;
        let (width, height) = panel.geometry.resize_to(width, height, ww, wh);
        Self::send_panel_event_by_panel(panel, PanelEvent::Resized { width, height });
        Ok(())
    }

    /// Dock an open panel to `zone` and notify the plugin.
    pub fn dock_panel(&mut self, handle: PanelHandle, zone: DockZone) -> Result<(), PanelError> {
        let panel = self.panels.get_mut(&handle).ok_or(PanelError::UnknownHandle)?;
        let (ww, wh) = self.window_size;
        panel.geometry.set_dock(zone, ww, wh)?;
        Self::send_panel_event_by_panel(panel, PanelEvent::Docked { zone });
        Ok(())
    }

    /// Undock an open panel and place it at `(x, y)`.
    pub fn undock_panel(
        &mut self,
        handle: PanelHandle,
        x: f32,
        y: f32,
    ) -> Result<(), PanelError> {
        let panel = self.panels.get_mut(&handle).ok_or(PanelError::UnknownHandle)?;
        let (ww, wh) = self.window_size;
        panel.geometry.width = panel.def.initial_width;
        panel.geometry.height = panel.def.initial_height;
        panel.geometry.move_to(x, y, ww, wh);
        Self::send_panel_event_by_panel(panel, PanelEvent::Undocked);
        Ok(())
    }

    /// Bring a panel to the front of the z-order.
    pub fn focus(&mut self, handle: PanelHandle) -> Result<(), PanelError> {
        if !self.panels.contains_key(&handle) {
            return Err(PanelError::UnknownHandle);
        }
        self.order.retain(|h| h != &handle);
        self.order.push(handle);
        if let Some(panel) = self.panels.get(&handle) {
            Self::send_panel_event_by_panel(panel, PanelEvent::Focused);
        }
        Ok(())
    }

    /// Replace the widgets of the panel identified by `panel_id`.
    pub fn update(&mut self, panel_id: &str, widgets: Vec<Widget>) {
        if let Some(&handle) = self.by_panel_id.get(panel_id) {
            if let Some(panel) = self.panels.get_mut(&handle) {
                // For `TextInput` widgets, only adopt a plugin-provided value
                // when it differs from the last plugin-provided value. This
                // prevents async `PanelUpdate`s from resetting the user's
                // in-progress typing, while still allowing the plugin to
                // intentionally clear or preset the field.
                let mut plugin_text = self.plugin_text_values.borrow_mut();
                let mut edit_values = self.edit_values.borrow_mut();
                for widget in &widgets {
                    if let Widget::TextInput { id, value } = widget {
                        let key = (panel_id.to_string(), id.clone());
                        let changed = plugin_text.get(&key) != Some(value);
                        if changed {
                            plugin_text.insert(key.clone(), value.clone());
                            edit_values.insert(key, value.clone());
                        }
                    }
                }
                panel.widgets = widgets;
            }
        }
    }

    /// Broadcast a document lifecycle event to every plugin process that owns
    /// at least one open panel. `Changed` events are suppressed when the
    /// geometry epoch has not changed since the last broadcast for that tab.
    pub fn broadcast_document_event(&self, tab: usize, event: DocumentEvent) {
        let host_event = match event {
            DocumentEvent::Activated => HostAsync::DocumentActivated { tab },
            DocumentEvent::Changed { version } => {
                let should_send = self
                    .last_changed_version
                    .borrow()
                    .get(&tab)
                    .copied()
                    .unwrap_or(0)
                    != version;
                if !should_send {
                    return;
                }
                self.last_changed_version.borrow_mut().insert(tab, version);
                HostAsync::DocumentChanged { tab, version }
            }
            DocumentEvent::Closed => HostAsync::TabClosed { tab },
        };
        let mut seen = std::collections::HashSet::new();
        for panel in self.panels.values() {
            if seen.insert(panel.process_id.clone()) {
                if let Err(e) = panel.process.send_async(host_event.clone()) {
                    eprintln!(
                        "[panel] failed to send document event to {}: {e}",
                        panel.process_id
                    );
                }
            }
        }
    }

    /// Handle a host UI message directed at a plugin panel.
    pub fn handle_message(&mut self, msg: &Message) -> Option<Task<Message>> {
        let (ww, wh) = self.window_size;
        match msg {
            Message::PluginPanelEvent {
                process_id: _,
                panel_id,
                event: PanelEvent::Closed,
            } => {
                if let Some(handle) = self.handle_by_panel_id(panel_id) {
                    let _ = self.close(handle);
                }
                None
            }
            Message::PluginPanelEvent {
                process_id,
                panel_id,
                event,
            } => {
                self.send_panel_event(process_id, panel_id, event.clone());
                None
            }
            Message::PanelDragStart(handle) => {
                let _ = self.focus(*handle);
                if let Some(panel) = self.panels.get_mut(handle) {
                    panel.geometry.start_drag();
                }
                None
            }
            Message::PanelResizeStart(handle) => {
                let _ = self.focus(*handle);
                if let Some(panel) = self.panels.get_mut(handle) {
                    panel.geometry.start_resize();
                }
                None
            }
            Message::PanelPointerMove { point } => {
                for panel in self.panels.values_mut() {
                    panel
                        .geometry
                        .drag_to(to_floating_point(*point), ww, wh);
                    panel
                        .geometry
                        .resize_move(to_floating_point(*point), ww, wh);
                }
                None
            }
            Message::PanelPointerRelease => {
                for panel in self.panels.values_mut() {
                    if panel.geometry.is_interacting() {
                        let drag_result = panel.geometry.end_drag(ww, wh);
                        Self::emit_drag_result(panel, drag_result);
                        let (width, height) = panel.geometry.end_resize();
                        Self::send_panel_event_by_panel(
                            panel,
                            PanelEvent::Resized { width, height },
                        );
                    }
                }
                None
            }
            Message::PanelFocus(handle) => {
                let _ = self.focus(*handle);
                None
            }
            _ => None,
        }
    }

    fn emit_drag_result(panel: &OpenPanel, result: ocs_plugin_api::panel::floating::DragResult) {
        use ocs_plugin_api::panel::floating::DragResult;
        match result {
            DragResult::Moved { x, y } => {
                Self::send_panel_event_by_panel(panel, PanelEvent::Moved { x, y });
            }
            DragResult::Docked { zone } => {
                Self::send_panel_event_by_panel(panel, PanelEvent::Docked { zone });
            }
            DragResult::Undocked { x, y } => {
                Self::send_panel_event_by_panel(panel, PanelEvent::Undocked);
                Self::send_panel_event_by_panel(panel, PanelEvent::Moved { x, y });
            }
        }
    }

    /// Send a user-generated panel event to the owning plugin process.
    fn send_panel_event(&self, process_id: &str, panel_id: &str, event: PanelEvent) {
        panel_log(&format!(
            "send_panel_event process={process_id} panel={panel_id} event={event:?}"
        ));
        let target = self
            .panels
            .values()
            .find(|p| p.process_id == process_id && p.panel_id == panel_id)
            .map(|p| Arc::clone(&p.process));
        if let Some(process) = target {
            if let Err(e) = process.send_async(HostAsync::PanelEvent {
                panel_id: panel_id.to_string(),
                event,
            }) {
                eprintln!("[panel] failed to send panel event to {process_id}: {e}");
            }
        }
    }

    fn send_panel_event_by_panel(panel: &OpenPanel, event: PanelEvent) {
        panel_log(&format!(
            "send_panel_event panel={} event={event:?}",
            panel.panel_id
        ));
        if let Err(e) = panel.process.send_async(HostAsync::PanelEvent {
            panel_id: panel.panel_id.clone(),
            event,
        }) {
            eprintln!("[panel] failed to send panel event to {}: {e}", panel.process_id);
        }
    }

    /// Look up the (process_id, panel_id) for an open panel handle.
    pub fn panel_by_handle(&self, handle: PanelHandle) -> Option<(&str, &str)> {
        self.panels
            .get(&handle)
            .map(|p| (p.process_id.as_str(), p.panel_id.as_str()))
    }

    /// Look up the handle for an open panel by its plugin id.
    pub fn handle_by_panel_id(&self, panel_id: &str) -> Option<PanelHandle> {
        self.by_panel_id.get(panel_id).copied()
    }

    /// Send a panel event given explicit owner ids. Used by the host manager
    /// after resolving a handle.
    pub fn send_panel_event_by_ids(&self, process_id: &str, panel_id: &str, event: PanelEvent) {
        self.send_panel_event(process_id, panel_id, event);
    }

    /// Render the open panels as floating overlays. All data is cloned so the
    /// returned element owns its contents and can outlive the manager borrow.
    pub fn view(&self) -> Element<'static, Message> {
        if self.panels.is_empty() {
            return Space::new().into();
        }
        let mut stack = Stack::new();
        for handle in &self.order {
            if let Some(panel) = self.panels.get(handle) {
                let positioned = container(self.render_floating_panel(panel))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(Padding {
                        top: panel.geometry.y,
                        left: panel.geometry.x,
                        right: 0.0,
                        bottom: 0.0,
                    });
                stack = stack.push(positioned);
                if let Some(zone) = panel.geometry.snap {
                    stack = stack.push(self.render_snap_ghost(panel, zone));
                }
            }
        }
        stack.width(Length::Fill).height(Length::Fill).into()
    }

    fn render_floating_panel(&self, panel: &OpenPanel) -> Element<'static, Message> {
        // Header stays fixed at the top; only the widget body scrolls.
        let header_drag = mouse_area(
            Row::new()
                .push(text(panel.def.title.clone()).size(14))
                .push(Space::new().width(Length::Fill)),
        )
        .on_press(Message::PanelDragStart(panel.handle))
        .interaction(iced::mouse::Interaction::Grab);

        let header = Row::new()
            .spacing(4)
            .push(
                container(header_drag)
                    .padding(2)
                    .style(|_| container::Style {
                        background: Some(iced::Background::Color(iced::Color {
                            r: 0.18,
                            g: 0.18,
                            b: 0.18,
                            a: 1.0,
                        })),
                        border: iced::Border {
                            color: iced::Color {
                                r: 0.3,
                                g: 0.3,
                                b: 0.3,
                                a: 1.0,
                            },
                            width: 1.0,
                            radius: 2.0.into(),
                        },
                        ..Default::default()
                    })
                    .width(Length::Fill),
            )
            .push(
                button(text("✕").size(12))
                    .on_press(Message::PluginPanelEvent {
                        process_id: panel.process_id.clone(),
                        panel_id: panel.panel_id.clone(),
                        event: PanelEvent::Closed,
                    })
                    .padding(2),
            )
            .width(Length::Fill);

        let widget_col = {
            let mut col = Column::new().spacing(4);
            for widget in &panel.widgets {
                col = col.push(self.render_widget(panel, widget));
            }
            col
        };

        let body = container(
            Column::new()
                .spacing(4)
                .push(header)
                .push(scrollable(widget_col).height(Length::Fill)),
        )
        .padding(6)
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::Color {
                r: 0.12,
                g: 0.12,
                b: 0.12,
                a: 1.0,
            })),
            border: iced::Border {
                color: iced::Color {
                    r: 0.25,
                    g: 0.25,
                    b: 0.25,
                    a: 1.0,
                },
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        })
        .width(Length::Fill)
        .height(Length::Fill);

        let resize_handle = container(
            mouse_area(
                container(Space::new())
                    .width(Length::Fixed(12.0))
                    .height(Length::Fixed(12.0))
                    .style(|_| container::Style {
                        background: Some(iced::Background::Color(iced::Color {
                            r: 0.3,
                            g: 0.3,
                            b: 0.3,
                            a: 1.0,
                        })),
                        border: iced::Border {
                            color: iced::Color {
                                r: 0.45,
                                g: 0.45,
                                b: 0.45,
                                a: 1.0,
                            },
                            width: 1.0,
                            radius: 2.0.into(),
                        },
                        ..Default::default()
                    }),
            )
            .on_press(Message::PanelResizeStart(panel.handle))
            .interaction(iced::mouse::Interaction::ResizingHorizontally),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Bottom)
        .padding(4.0);

        let panel_stack = iced::widget::Stack::with_capacity(2)
            .push(body)
            .push(resize_handle)
            .width(Length::Fixed(panel.geometry.width))
            .height(Length::Fixed(panel.geometry.height));

        mouse_area(panel_stack)
            .on_press(Message::PanelFocus(panel.handle))
            .into()
    }

    fn render_snap_ghost(&self, panel: &OpenPanel, zone: DockZone) -> Element<'static, Message> {
        use ocs_plugin_api::panel::floating::DOCKED_PANEL_WIDTH;
        let (x, y, w, h) = match zone {
            DockZone::Left => (0.0, 0.0, DOCKED_PANEL_WIDTH, self.window_size.1),
            DockZone::Right => (
                (self.window_size.0 - DOCKED_PANEL_WIDTH).max(0.0),
                0.0,
                DOCKED_PANEL_WIDTH,
                self.window_size.1,
            ),
            DockZone::Floating => (panel.geometry.x, panel.geometry.y, panel.geometry.width, panel.geometry.height),
        };
        container(Space::new())
            .width(Length::Fixed(w))
            .height(Length::Fixed(h))
            .padding(Padding {
                top: y,
                left: x,
                right: 0.0,
                bottom: 0.0,
            })
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color {
                    r: 0.4,
                    g: 0.4,
                    b: 0.4,
                    a: 0.2,
                })),
                border: iced::Border {
                    color: iced::Color {
                        r: 0.5,
                        g: 0.5,
                        b: 0.5,
                        a: 0.5,
                    },
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn render_widget(&self, panel: &OpenPanel, widget: &Widget) -> Element<'static, Message> {
        let process_id = panel.process_id.clone();
        let panel_id = panel.panel_id.clone();
        match widget.clone() {
            Widget::Label(value) => text(value).into(),
            Widget::Button { id, label } => button(text(label))
                .on_press(Message::PluginPanelEvent {
                    process_id,
                    panel_id,
                    event: PanelEvent::Clicked(id),
                })
                .width(Length::Fill)
                .into(),
            Widget::TextInput { id, value } => {
                let widget_id = id.clone();
                let key = (panel_id.clone(), widget_id.clone());
                let display_value = self
                    .edit_values
                    .borrow()
                    .get(&key)
                    .cloned()
                    .unwrap_or(value);
                let edit_values = Rc::clone(&self.edit_values);
                text_input("", &display_value)
                    .on_input(move |s| {
                        edit_values.borrow_mut().insert(key.clone(), s.clone());
                        Message::PluginPanelEvent {
                            process_id: process_id.clone(),
                            panel_id: panel_id.clone(),
                            event: PanelEvent::TextChanged {
                                id: widget_id.clone(),
                                value: s,
                            },
                        }
                    })
                    .width(Length::Fill)
                    .into()
            }
            Widget::MultilineOutput { id: _, lines } => {
                let mut inner = Column::new().spacing(2);
                for line in lines {
                    inner = inner.push(text(line).size(12));
                }
                container(inner).padding(2).into()
            }
            Widget::List { id, items } => {
                let mut inner = Column::new().spacing(2);
                for (idx, item) in items.into_iter().enumerate() {
                    let widget_id = id.clone();
                    let btn = button(text(item).size(12))
                        .on_press(Message::PluginPanelEvent {
                            process_id: process_id.clone(),
                            panel_id: panel_id.clone(),
                            event: PanelEvent::ItemSelected {
                                id: widget_id,
                                index: idx,
                            },
                        })
                        .width(Length::Fill);
                    inner = inner.push(btn);
                }
                container(inner).padding(2).into()
            }
        }
    }
}

fn to_floating_point(p: Point) -> FloatingPoint {
    FloatingPoint::new(p.x, p.y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockProcess {
        id: String,
        sent: Mutex<Vec<HostAsync>>,
    }

    impl PanelProcess for MockProcess {
        fn id(&self) -> &str {
            &self.id
        }

        fn send_async(&self, event: HostAsync) -> Result<(), PluginError> {
            self.sent.lock().unwrap().push(event);
            Ok(())
        }
    }

    fn mock(id: &str) -> Arc<dyn PanelProcess> {
        Arc::new(MockProcess {
            id: id.to_string(),
            ..Default::default()
        })
    }

    fn floating_def(id: &str) -> PanelDef {
        PanelDef {
            id: id.to_string(),
            title: id.to_string(),
            icon: None,
            dock: DockZone::Floating,
            initial_x: Some(100.0),
            initial_y: Some(80.0),
            initial_width: 260.0,
            initial_height: 400.0,
            min_width: 160.0,
            min_height: 120.0,
            dockable_zones: vec![DockZone::Floating, DockZone::Left, DockZone::Right],
            allow_undock: true,
            resizable: true,
            draggable: true,
            dock_style: ocs_plugin_api::panel::DockStyle::Tabs,
        }
    }

    #[test]
    fn panel_open_update_close() {
        let mut mgr = PanelManager::new();
        let process = mock("p1");
        let def = floating_def("test.panel");

        let handle = mgr.open(process, &def).expect("open panel");
        assert_eq!(mgr.panels.len(), 1);
        let panel = mgr.panels.get(&handle).unwrap();
        assert_eq!(panel.geometry.x, 100.0);
        assert_eq!(panel.geometry.y, 80.0);

        mgr.update(
            "test.panel",
            vec![
                Widget::Label("hello".to_string()),
                Widget::Button {
                    id: "btn1".to_string(),
                    label: "Click me".to_string(),
                },
            ],
        );
        let panel = mgr.panels.get(&handle).unwrap();
        assert_eq!(panel.widgets.len(), 2);
        assert!(matches!(panel.widgets[0], Widget::Label(ref s) if s == "hello"));

        mgr.close(handle).expect("close panel");
        assert!(mgr.panels.is_empty());
        assert!(mgr.by_panel_id.is_empty());
    }

    #[test]
    fn document_lifecycle_to_panel() {
        let mut mgr = PanelManager::new();
        let mock_proc = Arc::new(MockProcess {
            id: "p1".to_string(),
            ..Default::default()
        });
        let process: Arc<dyn PanelProcess> = mock_proc.clone();
        let def = floating_def("lifecycle.panel");
        mgr.open(process, &def).unwrap();

        mgr.broadcast_document_event(2, DocumentEvent::Activated);
        mgr.broadcast_document_event(2, DocumentEvent::Changed { version: 42 });
        mgr.broadcast_document_event(2, DocumentEvent::Closed);

        let sent = mock_proc.sent.lock().unwrap();
        assert_eq!(sent.len(), 3);
        assert!(matches!(&sent[0], HostAsync::DocumentActivated { tab: 2 }));
        assert!(matches!(&sent[1], HostAsync::DocumentChanged { tab: 2, version: 42 }));
        assert!(matches!(&sent[2], HostAsync::TabClosed { tab: 2 }));
    }

    #[test]
    fn button_click_reaches_plugin() {
        let mut mgr = PanelManager::new();
        let mock_proc = Arc::new(MockProcess {
            id: "p1".to_string(),
            ..Default::default()
        });
        let process: Arc<dyn PanelProcess> = mock_proc.clone();
        let def = floating_def("click.panel");
        mgr.open(process, &def).unwrap();

        let msg = Message::PluginPanelEvent {
            process_id: "p1".to_string(),
            panel_id: "click.panel".to_string(),
            event: PanelEvent::Clicked("btn1".to_string()),
        };
        mgr.handle_message(&msg);

        let sent = mock_proc.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(matches!(
            &sent[0],
            HostAsync::PanelEvent { panel_id, event: PanelEvent::Clicked(id) }
            if panel_id == "click.panel" && id == "btn1"
        ));
    }

    #[test]
    fn panel_move_clamps_and_notifies() {
        let mut mgr = PanelManager::new();
        mgr.set_window_size(800.0, 600.0);
        let mock_proc = Arc::new(MockProcess {
            id: "p1".to_string(),
            ..Default::default()
        });
        let process: Arc<dyn PanelProcess> = mock_proc.clone();
        let def = floating_def("move.panel");
        let handle = mgr.open(process, &def).unwrap();

        mgr.move_panel(handle, 900.0, 500.0).unwrap();
        let panel = mgr.panels.get(&handle).unwrap();
        assert!(panel.geometry.x <= 800.0 - panel.geometry.width);
        assert!(panel.geometry.y <= 600.0 - panel.geometry.height);

        let sent = mock_proc.sent.lock().unwrap();
        assert!(sent.iter().any(|e| matches!(
            e,
            HostAsync::PanelEvent { panel_id, event: PanelEvent::Moved { .. } }
            if panel_id == "move.panel"
        )));
    }

    #[test]
    fn focus_brings_panel_to_front() {
        let mut mgr = PanelManager::new();
        let p1 = mock("p1");
        let p2 = mock("p2");
        let h1 = mgr.open(p1, &floating_def("a")).unwrap();
        let h2 = mgr.open(p2, &floating_def("b")).unwrap();
        assert_eq!(mgr.order.last(), Some(&h2));

        mgr.focus(h1).unwrap();
        assert_eq!(mgr.order.last(), Some(&h1));
    }

    #[test]
    fn snap_to_left_edge() {
        let mut mgr = PanelManager::new();
        mgr.set_window_size(800.0, 600.0);
        let process = mock("p1");
        let def = floating_def("snap.panel");
        let handle = mgr.open(process, &def).unwrap();

        mgr.handle_message(&Message::PanelDragStart(handle));
        // First move records the anchor.
        mgr.handle_message(&Message::PanelPointerMove {
            point: Point::new(0.0, 0.0),
        });
        // Second move: center = 100 + 130 = 230, need delta = 16 - 230 = -214.
        mgr.handle_message(&Message::PanelPointerMove {
            point: Point::new(-214.0, 50.0),
        });
        let panel = mgr.panels.get(&handle).unwrap();
        assert_eq!(panel.geometry.snap, Some(DockZone::Left));
    }

    #[test]
    fn dock_panel_applies_geometry_and_notifies() {
        let mut mgr = PanelManager::new();
        mgr.set_window_size(800.0, 600.0);
        let mock_proc = Arc::new(MockProcess {
            id: "p1".to_string(),
            ..Default::default()
        });
        let process: Arc<dyn PanelProcess> = mock_proc.clone();
        let def = floating_def("dock.panel");
        let handle = mgr.open(process, &def).unwrap();

        mgr.dock_panel(handle, DockZone::Left).unwrap();
        let panel = mgr.panels.get(&handle).unwrap();
        assert_eq!(panel.geometry.dock, DockZone::Left);
        assert_eq!(panel.geometry.x, 0.0);
        assert_eq!(panel.geometry.width, 260.0);

        let sent = mock_proc.sent.lock().unwrap();
        assert!(sent.iter().any(|e| matches!(
            e,
            HostAsync::PanelEvent { panel_id, event: PanelEvent::Docked { zone: DockZone::Left } }
            if panel_id == "dock.panel"
        )));
    }

    #[test]
    fn undock_panel_applies_geometry_and_notifies() {
        let mut mgr = PanelManager::new();
        mgr.set_window_size(800.0, 600.0);
        let mock_proc = Arc::new(MockProcess {
            id: "p1".to_string(),
            ..Default::default()
        });
        let process: Arc<dyn PanelProcess> = mock_proc.clone();
        let mut def = floating_def("undock.panel");
        def.dock = DockZone::Left;
        let handle = mgr.open(process, &def).unwrap();

        mgr.undock_panel(handle, 120.0, 80.0).unwrap();
        let panel = mgr.panels.get(&handle).unwrap();
        assert_eq!(panel.geometry.dock, DockZone::Floating);
        assert_eq!(panel.geometry.x, 120.0);
        assert_eq!(panel.geometry.y, 80.0);

        let sent = mock_proc.sent.lock().unwrap();
        assert!(sent.iter().any(|e| matches!(
            e,
            HostAsync::PanelEvent { panel_id, event: PanelEvent::Undocked }
            if panel_id == "undock.panel"
        )));
    }
}
