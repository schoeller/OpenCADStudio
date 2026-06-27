use iced::time::Instant;
use iced::Point;

/// Mouse / selection interaction state for the viewport.
#[derive(Clone, Default)]
pub struct SelectionState {
    pub vp_size: (f32, f32),
    pub box_anchor: Option<Point>,
    pub box_current: Option<Point>,
    pub box_last: Option<(Point, Point)>,
    pub box_crossing: bool,
    pub box_last_crossing: bool,
    pub poly_active: bool,
    pub poly_points: Vec<Point>,
    pub poly_crossing: bool,
    pub poly_last_crossing: bool,
    pub context_menu: Option<Point>,
    /// True while the context menu's Draw Order sub-items are expanded.
    pub draworder_submenu: bool,
    pub last_move_pos: Option<Point>,
    pub left_down: bool,
    pub left_press_pos: Option<Point>,
    pub left_press_time: Option<Instant>,
    pub left_dragging: bool,
    pub right_down: bool,
    pub right_press_pos: Option<Point>,
    pub right_press_time: Option<Instant>,
    pub right_dragging: bool,
    pub right_last_pos: Option<Point>,
    pub middle_down: bool,
    pub middle_last_pos: Option<Point>,
    pub middle_last_press_time: Option<Instant>,
}
