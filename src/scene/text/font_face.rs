// Unified font face over the two glyph sources: embedded LFF stroke fonts and
// the user's installed TrueType/OpenType fonts. Both deliver glyphs in the same
// 9-unit cap-height space (see `ttf_glyph`), so the text layout code resolves a
// `Face` once per run and asks it for glyphs without caring which kind it is.
//
// Dispatch rule: a name that matches an embedded LFF font wins (so DXF stroke
// fonts like `txt`/`romans` keep rendering as before); otherwise, if it matches
// an installed system family, the TrueType path is used; otherwise we fall back
// to LFF's own `STANDARD` resolution.

use std::ops::Deref;
use std::sync::Arc;

use crate::scene::text::lff::{self, Glyph};
use crate::scene::text::{sysfont, ttf_glyph};

/// A glyph borrowed from a static LFF font or owned (cached `Arc`) from a TTF
/// face. Derefs to [`Glyph`] either way.
pub enum GlyphRef<'a> {
    Borrowed(&'a Glyph),
    Owned(Arc<Glyph>),
}

impl Deref for GlyphRef<'_> {
    type Target = Glyph;
    fn deref(&self) -> &Glyph {
        match self {
            GlyphRef::Borrowed(g) => g,
            GlyphRef::Owned(g) => g,
        }
    }
}

/// Resolved font for a text run.
pub enum Face {
    Lff(&'static lff::Font),
    Ttf {
        family: String,
        /// Cached space-glyph advance (9-unit), used for word spacing.
        word: f32,
    },
}

impl Face {
    /// Resolve a style's font name to a concrete face. Embedded stroke fonts
    /// take priority; only otherwise-unknown names try the system fonts.
    pub fn resolve(font_name: &str) -> Face {
        if !lff::is_builtin(font_name) && sysfont::has_family(font_name) {
            let word = ttf_glyph::glyph(font_name, ' ')
                .map(|g| g.advance)
                // Fall back to a sensible blank-width if the font has no space.
                .filter(|w| *w > 0.0)
                .unwrap_or(4.5);
            return Face::Ttf {
                family: font_name.to_string(),
                word,
            };
        }
        Face::Lff(lff::get_font(font_name))
    }

    /// Look up a glyph. A stroke (LFF) font uses its own glyphs; anything it
    /// lacks falls back to a system TrueType font chosen by cosmic-text (filled
    /// outline, so covers scripts no stroke font provides). The TTF path mirrors
    /// this for its single-glyph lookups.
    pub fn glyph(&self, ch: char) -> Option<GlyphRef<'_>> {
        match self {
            Face::Lff(f) => match f.glyph(ch) {
                Some(g) => Some(GlyphRef::Borrowed(g)),
                None => ttf_glyph::fallback_glyph(ch).map(GlyphRef::Owned),
            },
            Face::Ttf { family, .. } => ttf_glyph::glyph(family, ch)
                .or_else(|| ttf_glyph::fallback_glyph(ch))
                .map(GlyphRef::Owned),
        }
    }

    /// Extra spacing added after every glyph (9-unit). TTF advances already
    /// include side bearings, so no extra tracking is added there.
    pub fn letter_spacing(&self) -> f32 {
        match self {
            Face::Lff(f) => f.letter_spacing,
            Face::Ttf { .. } => 0.0,
        }
    }

    /// Width of a space (9-unit).
    pub fn word_spacing(&self) -> f32 {
        match self {
            Face::Lff(f) => f.word_spacing,
            Face::Ttf { word, .. } => *word,
        }
    }

    /// The system family name when this is a TrueType face, else `None`. Lets
    /// the tessellator shape TTF runs while keeping the per-glyph stroke path
    /// for LFF.
    pub fn ttf_family(&self) -> Option<&str> {
        match self {
            Face::Ttf { family, .. } => Some(family),
            Face::Lff(_) => None,
        }
    }

    /// Line-spacing factor (1.0 = one cap height between baselines as scaled by
    /// the caller). TTF uses the neutral 1.0; LFF carries its font's value.
    pub fn line_spacing(&self) -> f32 {
        match self {
            Face::Lff(f) => f.line_spacing,
            Face::Ttf { .. } => 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttf_dispatches_through_tessellation() {
        let fams = sysfont::families();
        if fams.is_empty() {
            eprintln!("no system fonts; skipping");
            return;
        }
        // Pick a family that actually yields an 'A' outline, then render a run
        // through the real LFF tessellation entry to prove the Face dispatch.
        let fam = fams
            .iter()
            .find(|f| ttf_glyph::glyph(f, 'A').is_some())
            .expect("a system family with an 'A'");
        assert!(matches!(Face::resolve(fam), Face::Ttf { .. }));
        let strokes =
            lff::tessellate_text_ex([0.0, 0.0], 10.0, 0.0, 1.0, 0.0, fam, "ABC");
        assert!(!strokes.is_empty(), "TTF run produced no strokes");
    }

    #[test]
    fn shx_name_stays_lff() {
        assert!(matches!(Face::resolve("txt"), Face::Lff(_)));
        assert!(matches!(Face::resolve("romans.shx"), Face::Lff(_)));
    }

    #[test]
    fn lff_falls_back_to_ttf_for_uncovered_glyph() {
        // A CJK ideograph is in no stroke font; with a stroke style selected it
        // must now resolve through the system-font (cosmic-text) fallback rather
        // than coming back empty. Tolerated if the machine has no CJK font.
        let txt = Face::resolve("txt");
        assert!(matches!(txt, Face::Lff(_)));
        assert!(txt.glyph('A').is_some(), "primary stroke glyph still works");
        match txt.glyph('中') {
            Some(g) => assert!(!g.strokes.is_empty(), "fallback glyph has contours"),
            None => eprintln!("no CJK system font; fallback skipped"),
        }
    }

    #[test]
    fn ttf_run_keeps_decorations() {
        let fams = sysfont::families();
        if fams.is_empty() {
            eprintln!("no system fonts; skipping");
            return;
        }
        let fam = fams
            .iter()
            .find(|f| ttf_glyph::glyph(f, 'A').is_some())
            .expect("a system family with an 'A'");
        // Underlined shaped text: glyph contours plus exactly one underline
        // segment (a 2-point polyline) emitted on the \l toggle.
        let plain = lff::tessellate_text_ex([0.0, 0.0], 10.0, 0.0, 1.0, 0.0, fam, "AB");
        let deco = lff::tessellate_text_ex([0.0, 0.0], 10.0, 0.0, 1.0, 0.0, fam, "\\LAB\\l");
        assert!(!plain.is_empty());
        assert_eq!(
            deco.len(),
            plain.len() + 1,
            "underline should add exactly one segment on the TTF path"
        );
    }
}
