//! Host-agnostic geometry and interaction state for in-window floating panels.
//!
//! This module is independent of any UI toolkit. The host renderer (e.g. the
//! `OpenCADStudio` iced frontend) creates a [`FloatingPanel`] for each open
//! panel and calls its methods in response to drag/resize/window events.

use super::{DockStyle, DockZone, PanelDef, PanelError};

/// Distance from a window edge at which a floating panel snaps to that edge.
pub const SNAP_DISTANCE: f32 = 16.0;
/// Width of a panel docked to the left or right edge.
pub const DOCKED_PANEL_WIDTH: f32 = 260.0;

/// Result of finishing a panel drag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragResult {
    /// The panel stayed floating; its new position is `(x, y)`.
    Moved { x: f32, y: f32 },
    /// The panel was docked to `zone`.
    Docked { zone: DockZone },
    /// A previously docked panel was undocked and is now floating at `(x, y)`.
    Undocked { x: f32, y: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DragState {
    start_cursor: Option<Point>,
    start_pos: Point,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ResizeState {
    start_cursor: Option<Point>,
    start_size: Point,
}

/// A 2-D point with `f32` coordinates. Toolkit-agnostic copy of the usual
/// `(x, y)` pair so this module does not depend on a rendering library.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Geometry and interaction state for one floating/docked panel.
#[derive(Debug, Clone)]
pub struct FloatingPanel {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub dock: DockZone,
    pub snap: Option<DockZone>,
    pub dock_style: DockStyle,
    min_width: f32,
    min_height: f32,
    dockable_zones: Vec<DockZone>,
    allow_undock: bool,
    resizable: bool,
    draggable: bool,
    drag: Option<DragState>,
    resize: Option<ResizeState>,
}

impl FloatingPanel {
    /// Create panel geometry from a `PanelDef` and the current window size.
    pub fn new(def: &PanelDef, window_width: f32, window_height: f32) -> Self {
        let dock = Self::clamp_dock_zone(&def.dockable_zones, def.dock);
        let mut panel = Self {
            x: 0.0,
            y: 0.0,
            width: def.initial_width,
            height: def.initial_height,
            dock,
            snap: None,
            dock_style: def.dock_style,
            min_width: def.min_width.max(32.0),
            min_height: def.min_height.max(32.0),
            dockable_zones: def.dockable_zones.clone(),
            allow_undock: def.allow_undock,
            resizable: def.resizable,
            draggable: def.draggable,
            drag: None,
            resize: None,
        };
        panel.apply_def_geometry(def, window_width, window_height);
        panel.clamp_geometry(window_width, window_height);
        panel
    }

    /// Refresh geometry and docking policy from an updated `PanelDef`.
    pub fn refresh_def(&mut self, def: &PanelDef, window_width: f32, window_height: f32) {
        self.set_policy(def);
        self.min_width = def.min_width.max(32.0);
        self.min_height = def.min_height.max(32.0);
        self.dock = Self::clamp_dock_zone(&self.dockable_zones, def.dock);
        self.apply_def_geometry(def, window_width, window_height);
        self.clamp_geometry(window_width, window_height);
    }

    /// Copy the docking policy from a `PanelDef`.
    pub fn set_policy(&mut self, def: &PanelDef) {
        self.dockable_zones = def.dockable_zones.clone();
        self.allow_undock = def.allow_undock;
        self.resizable = def.resizable;
        self.draggable = def.draggable;
        self.dock_style = def.dock_style;
    }

    /// Clamp a desired dock zone to the set of allowed zones. An empty
    /// `dockable_zones` means no docking changes are allowed, so the preferred
    /// zone is kept.
    fn clamp_dock_zone(dockable_zones: &[DockZone], preferred: DockZone) -> DockZone {
        if dockable_zones.is_empty() || dockable_zones.contains(&preferred) {
            preferred
        } else if dockable_zones.contains(&DockZone::Floating) {
            DockZone::Floating
        } else {
            dockable_zones
                .first()
                .copied()
                .unwrap_or(DockZone::Floating)
        }
    }

    /// Returns `true` if the panel is allowed to dock to `zone`.
    pub fn can_dock(&self, zone: DockZone) -> bool {
        self.dockable_zones.contains(&zone)
    }

    /// Dock the panel to `zone` and apply the corresponding geometry.
    /// Returns an error if `zone` is not in `dockable_zones`.
    pub fn set_dock(
        &mut self,
        zone: DockZone,
        window_width: f32,
        window_height: f32,
    ) -> Result<(), PanelError> {
        if !self.can_dock(zone) {
            return Err(PanelError::Unsupported);
        }
        self.dock = zone;
        match zone {
            DockZone::Left => {
                self.x = 0.0;
                self.y = 0.0;
                self.width = DOCKED_PANEL_WIDTH;
                self.height = window_height;
            }
            DockZone::Right => {
                self.x = (window_width - DOCKED_PANEL_WIDTH).max(0.0);
                self.y = 0.0;
                self.width = DOCKED_PANEL_WIDTH;
                self.height = window_height;
            }
            DockZone::Floating => {
                self.width = self.width.max(self.min_width).min(window_width);
                self.height = self.height.max(self.min_height).min(window_height);
                self.clamp_geometry(window_width, window_height);
            }
        }
        Ok(())
    }

    /// Call when the main window is resized. Docked panels are re-docked;
    /// floating panels are clamped to the new bounds.
    pub fn set_window_size(&mut self, def: &PanelDef, window_width: f32, window_height: f32) {
        if self.dock != DockZone::Floating {
            self.apply_def_geometry(def, window_width, window_height);
        }
        self.clamp_geometry(window_width, window_height);
    }

    fn apply_def_geometry(&mut self, def: &PanelDef, window_width: f32, window_height: f32) {
        match self.dock {
            DockZone::Left => {
                self.x = 0.0;
                self.y = 0.0;
                self.width = DOCKED_PANEL_WIDTH;
                self.height = window_height;
            }
            DockZone::Right => {
                self.x = (window_width - DOCKED_PANEL_WIDTH).max(0.0);
                self.y = 0.0;
                self.width = DOCKED_PANEL_WIDTH;
                self.height = window_height;
            }
            DockZone::Floating => {
                self.width = def.initial_width;
                self.height = def.initial_height;
                self.x = def
                    .initial_x
                    .unwrap_or_else(|| 32.0 + (self.x as u64 as f32 * 16.0) % 200.0);
                self.y = def.initial_y.unwrap_or(32.0);
            }
        }
    }

    fn clamp_geometry(&mut self, window_width: f32, window_height: f32) {
        self.width = self.width.max(self.min_width).min(window_width);
        self.height = self.height.max(self.min_height).min(window_height);
        self.x = self.x.max(0.0).min((window_width - self.width).max(0.0));
        self.y = self.y.max(0.0).min((window_height - self.height).max(0.0));
    }

    /// Move the panel to `(x, y)`, clamp to bounds, and float it.
    /// Returns the clamped position.
    pub fn move_to(&mut self, x: f32, y: f32, window_width: f32, window_height: f32) -> (f32, f32) {
        self.dock = DockZone::Floating;
        self.x = x;
        self.y = y;
        self.clamp_geometry(window_width, window_height);
        (self.x, self.y)
    }

    /// Resize the panel, clamping to minimum and window size.
    /// Returns the clamped size.
    pub fn resize_to(
        &mut self,
        width: f32,
        height: f32,
        window_width: f32,
        window_height: f32,
    ) -> (f32, f32) {
        self.width = width;
        self.height = height;
        self.clamp_geometry(window_width, window_height);
        (self.width, self.height)
    }

    /// Start a drag operation.
    pub fn start_drag(&mut self) {
        self.drag = Some(DragState {
            start_cursor: None,
            start_pos: Point::new(self.x, self.y),
        });
    }

    /// Update geometry while dragging. The first call records the cursor anchor;
    /// subsequent calls move the panel by delta.
    pub fn drag_to(&mut self, cursor: Point, window_width: f32, window_height: f32) {
        if !self.draggable {
            return;
        }
        if let Some(drag) = self.drag.as_mut() {
            let was_docked = self.dock != DockZone::Floating;
            if was_docked && !self.allow_undock {
                return;
            }
            self.dock = DockZone::Floating;
            if let Some(start) = drag.start_cursor {
                self.x = drag.start_pos.x + (cursor.x - start.x);
                self.y = drag.start_pos.y + (cursor.y - start.y);
                self.snap = Self::compute_snap(self.x, self.width, window_width)
                    .filter(|z| self.can_dock(*z));
                self.clamp_geometry(window_width, window_height);
            } else {
                drag.start_cursor = Some(cursor);
            }
        }
    }

    /// Finish dragging. Returns the logical outcome so the host can emit the
    /// appropriate `PanelEvent`.
    pub fn end_drag(&mut self, window_width: f32, window_height: f32) -> DragResult {
        self.drag = None;
        if let Some(zone) = self.snap.take() {
            if !self.can_dock(zone) {
                self.dock = DockZone::Floating;
                return DragResult::Moved {
                    x: self.x,
                    y: self.y,
                };
            }
            let was_docked = self.dock != DockZone::Floating;
            self.dock = zone;
            match zone {
                DockZone::Left => {
                    self.x = 0.0;
                    self.y = 0.0;
                    self.width = DOCKED_PANEL_WIDTH;
                    self.height = window_height;
                }
                DockZone::Right => {
                    self.x = (window_width - DOCKED_PANEL_WIDTH).max(0.0);
                    self.y = 0.0;
                    self.width = DOCKED_PANEL_WIDTH;
                    self.height = window_height;
                }
                DockZone::Floating => {}
            }
            if was_docked {
                DragResult::Moved {
                    x: self.x,
                    y: self.y,
                }
            } else {
                DragResult::Docked { zone }
            }
        } else {
            let was_docked = self.dock != DockZone::Floating;
            if was_docked && !self.allow_undock {
                return DragResult::Moved {
                    x: self.x,
                    y: self.y,
                };
            }
            self.dock = DockZone::Floating;
            if was_docked {
                DragResult::Undocked {
                    x: self.x,
                    y: self.y,
                }
            } else {
                DragResult::Moved {
                    x: self.x,
                    y: self.y,
                }
            }
        }
    }

    /// Start a resize operation.
    pub fn start_resize(&mut self) {
        self.resize = Some(ResizeState {
            start_cursor: None,
            start_size: Point::new(self.width, self.height),
        });
    }

    /// Update size while resizing.
    pub fn resize_move(&mut self, cursor: Point, window_width: f32, window_height: f32) {
        if !self.resizable {
            return;
        }
        if let Some(resize) = self.resize.as_mut() {
            if let Some(start) = resize.start_cursor {
                self.width = resize.start_size.x + (cursor.x - start.x);
                self.height = resize.start_size.y + (cursor.y - start.y);
                self.clamp_geometry(window_width, window_height);
            } else {
                resize.start_cursor = Some(cursor);
            }
        }
    }

    /// Finish resizing. Returns the final size.
    pub fn end_resize(&mut self) -> (f32, f32) {
        self.resize = None;
        (self.width, self.height)
    }

    /// Returns `true` if a drag or resize is currently active.
    pub fn is_interacting(&self) -> bool {
        self.drag.is_some() || self.resize.is_some()
    }

    fn compute_snap(x: f32, width: f32, window_width: f32) -> Option<DockZone> {
        let center_x = x + width / 2.0;
        if center_x <= SNAP_DISTANCE {
            Some(DockZone::Left)
        } else if center_x >= window_width - SNAP_DISTANCE {
            Some(DockZone::Right)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> PanelDef {
        PanelDef {
            id: "test".to_string(),
            title: "Test".to_string(),
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
            dock_style: DockStyle::Tabs,
        }
    }

    #[test]
    fn new_floating_uses_def_geometry() {
        let panel = FloatingPanel::new(&def(), 800.0, 600.0);
        assert_eq!(panel.x, 100.0);
        assert_eq!(panel.y, 80.0);
        assert_eq!(panel.width, 260.0);
        assert_eq!(panel.height, 400.0);
        assert_eq!(panel.dock, DockZone::Floating);
    }

    #[test]
    fn move_clamps_to_window() {
        let mut panel = FloatingPanel::new(&def(), 800.0, 600.0);
        let (x, y) = panel.move_to(900.0, 500.0, 800.0, 600.0);
        assert!(x <= 800.0 - panel.width);
        assert!(y <= 600.0 - panel.height);
        assert_eq!(panel.dock, DockZone::Floating);
    }

    #[test]
    fn drag_snap_to_left_edge() {
        let mut panel = FloatingPanel::new(&def(), 800.0, 600.0);
        panel.start_drag();
        // First move records the anchor.
        panel.drag_to(Point::new(0.0, 0.0), 800.0, 600.0);
        // Second move: center = 100 + 130 = 230, need delta = 16 - 230 = -214
        // from anchor 0 -> cursor -214.
        panel.drag_to(Point::new(-214.0, 50.0), 800.0, 600.0);
        assert_eq!(panel.snap, Some(DockZone::Left));
    }

    #[test]
    fn drag_end_docks() {
        let mut panel = FloatingPanel::new(&def(), 800.0, 600.0);
        panel.start_drag();
        panel.drag_to(Point::new(0.0, 0.0), 800.0, 600.0);
        panel.drag_to(Point::new(-214.0, 50.0), 800.0, 600.0);
        let result = panel.end_drag(800.0, 600.0);
        assert_eq!(
            result,
            DragResult::Docked {
                zone: DockZone::Left
            }
        );
        assert_eq!(panel.dock, DockZone::Left);
        assert_eq!(panel.x, 0.0);
        assert_eq!(panel.width, DOCKED_PANEL_WIDTH);
    }

    #[test]
    fn resize_honours_minimum() {
        let mut panel = FloatingPanel::new(&def(), 800.0, 600.0);
        panel.start_resize();
        panel.resize_move(Point::new(0.0, 0.0), 800.0, 600.0);
        panel.resize_move(Point::new(1.0, 1.0), 800.0, 600.0);
        let (w, h) = panel.end_resize();
        assert!(w >= panel.min_width);
        assert!(h >= panel.min_height);
    }

    #[test]
    fn panel_with_empty_dockable_zones_cannot_dock() {
        let mut d = def();
        d.dockable_zones = vec![];
        let panel = FloatingPanel::new(&d, 800.0, 600.0);
        assert!(!panel.can_dock(DockZone::Left));
        assert!(!panel.can_dock(DockZone::Floating));
    }

    #[test]
    fn dock_panel_applies_dock_geometry() {
        let mut panel = FloatingPanel::new(&def(), 800.0, 600.0);
        panel.set_dock(DockZone::Right, 800.0, 600.0).unwrap();
        assert_eq!(panel.dock, DockZone::Right);
        assert_eq!(panel.width, DOCKED_PANEL_WIDTH);
        assert_eq!(panel.height, 600.0);
    }

    #[test]
    fn undock_panel_keeps_position_and_sets_floating() {
        let mut d = def();
        d.dock = DockZone::Left;
        let mut panel = FloatingPanel::new(&d, 800.0, 600.0);
        panel.set_dock(DockZone::Floating, 800.0, 600.0).unwrap();
        assert_eq!(panel.dock, DockZone::Floating);
    }

    #[test]
    fn disallowed_zone_returns_error() {
        let mut d = def();
        d.dockable_zones = vec![DockZone::Left];
        let mut panel = FloatingPanel::new(&d, 800.0, 600.0);
        assert!(panel.set_dock(DockZone::Right, 800.0, 600.0).is_err());
    }

    #[test]
    fn resize_is_noop_when_not_resizable() {
        let mut d = def();
        d.resizable = false;
        let mut panel = FloatingPanel::new(&d, 800.0, 600.0);
        let original_width = panel.width;
        panel.start_resize();
        panel.resize_move(Point::new(0.0, 0.0), 800.0, 600.0);
        panel.resize_move(Point::new(500.0, 500.0), 800.0, 600.0);
        assert_eq!(panel.width, original_width);
    }

    #[test]
    fn drag_does_not_undock_when_not_allowed() {
        let mut d = def();
        d.dock = DockZone::Left;
        d.allow_undock = false;
        d.draggable = true;
        let mut panel = FloatingPanel::new(&d, 800.0, 600.0);
        panel.start_drag();
        panel.drag_to(Point::new(0.0, 0.0), 800.0, 600.0);
        panel.drag_to(Point::new(400.0, 300.0), 800.0, 600.0);
        assert_eq!(panel.dock, DockZone::Left);
        let result = panel.end_drag(800.0, 600.0);
        assert_eq!(
            result,
            DragResult::Moved {
                x: panel.x,
                y: panel.y
            }
        );
        assert_eq!(panel.dock, DockZone::Left);
    }

    #[test]
    fn new_clamps_disallowed_initial_dock_zone() {
        let mut d = def();
        d.dock = DockZone::Right;
        d.dockable_zones = vec![DockZone::Left];
        let panel = FloatingPanel::new(&d, 800.0, 600.0);
        assert_eq!(panel.dock, DockZone::Left);
    }

    #[test]
    fn refresh_def_clamps_disallowed_dock_zone() {
        let mut d = def();
        d.dock = DockZone::Right;
        d.dockable_zones = vec![DockZone::Left];
        let mut panel = FloatingPanel::new(&def(), 800.0, 600.0);
        panel.refresh_def(&d, 800.0, 600.0);
        assert_eq!(panel.dock, DockZone::Left);
    }

    #[test]
    fn empty_dockable_zones_keeps_initial_dock() {
        let mut d = def();
        d.dock = DockZone::Left;
        d.dockable_zones = vec![];
        let panel = FloatingPanel::new(&d, 800.0, 600.0);
        assert_eq!(panel.dock, DockZone::Left);
    }
}
