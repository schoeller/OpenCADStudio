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
    // Grid snap (SNAPMODE) is a per-drawing view setting stored on the VPort,
    // not a global OSNAP preference, so it is deliberately excluded from the
    // persisted set. (#121)
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

/// Parse a persisted `r,g,b` background triplet (each 0–255). Returns `None`
/// for an empty or malformed value so a missing/garbage key falls back to the
/// app default rather than a wrong colour.
fn parse_rgb(val: &str) -> Option<[u8; 3]> {
    let mut it = val.split(',').map(|t| t.trim().parse::<u8>());
    let r = it.next()?.ok()?;
    let g = it.next()?.ok()?;
    let b = it.next()?.ok()?;
    if it.next().is_some() {
        return None;
    }
    Some([r, g, b])
}

/// Serialize an optional background triplet back to `r,g,b`, or empty when unset.
fn rgb_to_str(c: Option<[u8; 3]>) -> String {
    c.map(|[r, g, b]| format!("{r},{g},{b}")).unwrap_or_default()
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
    pub snap_enabled: bool,
    pub otrack: bool,
    /// Active snap modes, in `SNAP_ORDER`.
    pub snap_modes: Vec<SnapType>,
    /// Whether the one-time "make Open CAD Studio the default for .dwg/.dxf?"
    /// prompt has already been shown. Set once the user answers (either way),
    /// so we never nag again on subsequent launches.
    pub default_assoc_prompted: bool,
    /// Ids of plugins the user turned off in the Plugin Manager. Disabled
    /// plugins keep their manifest listed but drop their ribbon tab and command
    /// dispatch.
    pub disabled_plugins: Vec<String>,
    /// Linked plugin source repositories (`owner/repo`) the marketplace installs
    /// from.
    pub plugin_repos: Vec<String>,
    /// Controls whether the TEXTEDIT command repeats automatically (0 = Multiple, 1 = Single).
    pub texteditmode: bool,
    /// When true, a right-click in the viewport acts as Enter (commit / close)
    /// while a command is active; when idle it still opens the context menu.
    /// Toggled by the `RMBENTER` command. Right-drag always orbits.
    pub rmb_enter: bool,
    /// Persisted viewport background colours (0–255 RGB); `None` = app default
    /// (dark grey model / off-white paper). Applied to every drawing tab on
    /// launch and to tabs opened later, so a chosen background survives restarts
    /// (#188).
    pub bg_color: Option<[u8; 3]>,
    pub paper_bg_color: Option<[u8; 3]>,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            dyn_input: true,
            ortho: false,
            polar: false,
            polar_increment_deg: 45.0,
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
            default_assoc_prompted: false,
            disabled_plugins: Vec::new(),
            plugin_repos: Vec::new(),
            texteditmode: false,
            rmb_enter: false,
            bg_color: None,
            paper_bg_color: None,
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
                "osnap" => s.snap_enabled = val == "1",
                "otrack" => s.otrack = val == "1",
                "bg_color" => s.bg_color = parse_rgb(val),
                "paper_bg_color" => s.paper_bg_color = parse_rgb(val),
                "default_assoc_prompted" => s.default_assoc_prompted = val == "1",
                "rmb_enter" => s.rmb_enter = val == "1",
                "texteditmode" => {
                    if let Some(v) =
                        crate::modules::annotate::textedit::parse_texteditmode(val)
                    {
                        s.texteditmode = v;
                    }
                }
                "disabled_plugins" => {
                    s.disabled_plugins = val
                        .split(',')
                        .map(|t| t.trim())
                        .filter(|t| !t.is_empty())
                        .map(|t| t.to_string())
                        .collect();
                }
                "plugin_repos" => {
                    s.plugin_repos = val
                        .split(',')
                        .map(|t| t.trim())
                        .filter(|t| !t.is_empty())
                        .map(|t| t.to_string())
                        .collect();
                }
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
            "dyn={}\northo={}\npolar={}\npolar_increment_deg={}\nosnap={}\notrack={}\ndefault_assoc_prompted={}\nsnap_modes={}\ndisabled_plugins={}\nplugin_repos={}\ntexteditmode={}\nrmb_enter={}\nbg_color={}\npaper_bg_color={}\n",
            b(self.dyn_input),
            b(self.ortho),
            b(self.polar),
            self.polar_increment_deg,
            b(self.snap_enabled),
            b(self.otrack),
            b(self.default_assoc_prompted),
            modes,
            self.disabled_plugins.join(","),
            self.plugin_repos.join(","),
            self.texteditmode,
            b(self.rmb_enter),
            rgb_to_str(self.bg_color),
            rgb_to_str(self.paper_bg_color),
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
