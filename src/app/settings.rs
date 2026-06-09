//! Persisted user preferences — DYN, ORTHO, POLAR, the polar increment, the
//! grid toggle, and the object-snap configuration (OSNAP on/off, which snap
//! modes are active, OTRACK). These are UI choices, not drawing data, so they
//! live in a per-user config file and survive across sessions.
//!
//! Plain `key=value` text, matching the recent-files / status-bar stores so we
//! don't pull in a serialization crate just for a handful of flags. Drawing
//! header settings such as LWT (lineweight display) are intentionally NOT here
//! — those belong to the file.

use crate::snap::SnapType;
use std::path::PathBuf;

/// Canonical order for serializing snap modes, so the written list is stable
/// (a `HashSet` iterates in arbitrary order).
const SNAP_ORDER: &[SnapType] = &[
    SnapType::Endpoint,
    SnapType::Midpoint,
    SnapType::Center,
    SnapType::Node,
    SnapType::Quadrant,
    SnapType::Intersection,
    SnapType::Extension,
    SnapType::Insertion,
    SnapType::Perpendicular,
    SnapType::Tangent,
    SnapType::Nearest,
    SnapType::ApparentIntersection,
    SnapType::Parallel,
    SnapType::Grid,
];

fn snap_id(s: SnapType) -> &'static str {
    match s {
        SnapType::Endpoint => "endpoint",
        SnapType::Midpoint => "midpoint",
        SnapType::Center => "center",
        SnapType::Node => "node",
        SnapType::Quadrant => "quadrant",
        SnapType::Intersection => "intersection",
        SnapType::Extension => "extension",
        SnapType::Insertion => "insertion",
        SnapType::Perpendicular => "perpendicular",
        SnapType::Tangent => "tangent",
        SnapType::Nearest => "nearest",
        SnapType::ApparentIntersection => "apparentintersection",
        SnapType::Parallel => "parallel",
        SnapType::Grid => "grid",
        // Internal plugin object-pick snap — not a user-toggleable OSNAP mode,
        // so it never enters SNAP_ORDER / the persisted set.
        SnapType::ObjectPick => "objectpick",
    }
}

fn snap_from_id(s: &str) -> Option<SnapType> {
    SNAP_ORDER.iter().copied().find(|t| snap_id(*t) == s)
}

/// A snapshot of the persisted preferences. Field defaults mirror the app's
/// in-code defaults so a missing key restores the same value the app boots
/// with.
#[derive(Clone, PartialEq)]
pub struct UserSettings {
    pub dyn_input: bool,
    pub ortho: bool,
    pub polar: bool,
    pub polar_increment_deg: f32,
    pub show_grid: bool,
    pub snap_enabled: bool,
    pub otrack: bool,
    /// Active snap modes, in `SNAP_ORDER`.
    pub snap_modes: Vec<SnapType>,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            dyn_input: true,
            ortho: false,
            polar: false,
            polar_increment_deg: 45.0,
            show_grid: false,
            snap_enabled: false,
            otrack: false,
            snap_modes: vec![
                SnapType::Endpoint,
                SnapType::Midpoint,
                SnapType::Center,
                SnapType::Node,
                SnapType::Quadrant,
                SnapType::Intersection,
                SnapType::Nearest,
            ],
        }
    }
}

impl UserSettings {
    /// Build the active-mode set in canonical order from any iterator of modes.
    pub fn modes_from<'a>(modes: impl IntoIterator<Item = &'a SnapType>) -> Vec<SnapType> {
        let set: std::collections::HashSet<SnapType> = modes.into_iter().copied().collect();
        SNAP_ORDER.iter().copied().filter(|t| set.contains(t)).collect()
    }

    /// Read the saved preferences, or `None` when no settings file exists yet.
    /// Unknown / missing keys fall back to [`UserSettings::default`].
    pub fn load() -> Option<Self> {
        let path = config_path()?;
        let body = std::fs::read_to_string(path).ok()?;
        let mut s = UserSettings::default();
        for line in body.lines() {
            let line = line.trim();
            let Some((key, val)) = line.split_once('=') else { continue };
            let (key, val) = (key.trim(), val.trim());
            match key {
                "dyn" => s.dyn_input = val == "1",
                "ortho" => s.ortho = val == "1",
                "polar" => s.polar = val == "1",
                "polar_increment_deg" => {
                    if let Ok(v) = val.parse::<f32>() {
                        s.polar_increment_deg = v;
                    }
                }
                "grid" => s.show_grid = val == "1",
                "osnap" => s.snap_enabled = val == "1",
                "otrack" => s.otrack = val == "1",
                "snap_modes" => {
                    let modes: Vec<SnapType> =
                        val.split(',').filter_map(|t| snap_from_id(t.trim())).collect();
                    s.snap_modes = UserSettings::modes_from(modes.iter());
                }
                _ => {}
            }
        }
        Some(s)
    }

    /// Best-effort persist; silent on failure (read-only home, full disk).
    pub fn save(&self) {
        let Some(path) = config_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let b = |v: bool| if v { "1" } else { "0" };
        let modes = self
            .snap_modes
            .iter()
            .map(|t| snap_id(*t))
            .collect::<Vec<_>>()
            .join(",");
        let body = format!(
            "dyn={}\northo={}\npolar={}\npolar_increment_deg={}\ngrid={}\nosnap={}\notrack={}\nsnap_modes={}\n",
            b(self.dyn_input),
            b(self.ortho),
            b(self.polar),
            self.polar_increment_deg,
            b(self.show_grid),
            b(self.snap_enabled),
            b(self.otrack),
            modes,
        );
        let _ = std::fs::write(path, body);
    }
}

/// `<config-dir>/OpenCADStudio/settings.txt`, matching the recent-files store.
fn config_path() -> Option<PathBuf> {
    let base: PathBuf = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)?
    } else if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME")?;
        let mut p = PathBuf::from(home);
        p.push("Library");
        p.push("Application Support");
        p
    } else if let Some(d) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(d)
    } else {
        let home = std::env::var_os("HOME")?;
        let mut p = PathBuf::from(home);
        p.push(".config");
        p
    };
    let mut p = base;
    p.push("OpenCADStudio");
    Some(p.join("settings.txt"))
}
