//! The one colour table, and the gamma correction that makes it true.
//!
//! **A token module only.** No `apply`, no `Visuals`, no spacing — those land
//! in Stage 3. The table exists now so that the inspector, which is the
//! largest new surface in the rework, is written against named tokens on its
//! first line rather than against `Color32::from_rgb` literals that Stage 3
//! would then have to find and replace by hand.
//!
//! # Why every value goes through [`tok`]
//!
//! egui's colour does not arrive on screen as authored: the vertex shader
//! raises it to 2.2 and the sRGB swapchain encodes the result, and those two
//! are not inverses. A hex authored naively lands about 36% darker —
//! `#16191E` arrives as `#0E1218`. [`tok`] is the exact inverse of that
//! composition, and it lives in `loom_render::ui` beside the flag it
//! compensates for; see there for the derivation and for why it cannot live
//! in this crate.
//!
//! **Doc 11 states that arrival as `#0E0E10`, and that value is wrong** — it
//! applies one channel's correction to all three, and the transfer function
//! is per-channel and non-linear. `without_tok_the_panel_is_a_third_too_dark`
//! in this module is the recomputation. Its *magnitude* claim, ~35%, survives
//! and nothing downstream of it moves.
//!
//! **No gate can see any of this.** `cargo xtask image` drives the offscreen
//! `Renderer`, which never constructs a `Ui`. The check is a human running
//! the swatch probe and sampling three squares.

use loom_render::egui::Color32;

/// Pre-warp an authored hex into the byte egui must be given.
///
/// **Re-exported, not defined here.** The correction belongs beside the
/// `srgb_framebuffer` flag it compensates for, in `loom_render::ui`, because
/// the runtime links that module for the HUD and a shipped game must not get
/// only the half of the fix that darkens it. See [`loom_render::ui::tok`] for
/// the three-stage pipeline this inverts.
pub use loom_render::ui::tok;

/// Every colour the editor is allowed to use.
///
/// Adding a field here is a design decision; reaching for `Color32::from_rgb`
/// at a call site is a bug. The whole editor runs on six hues, and a seventh
/// is the token creep that makes a design system stop being followed.
#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    // --- Ground and surfaces ---
    /// Behind the dock, the gutters between panels, viewport letterbox.
    pub ground: Color32,
    /// Every panel body — the application's default background.
    pub surface: Color32,
    /// Anything sitting on top of a panel: tab bar, toolbar, menu bar.
    pub raised: Color32,
    /// Text fields, the console body, a slider's value track.
    pub sunken: Color32,
    /// A hovered row or button. A fill with no thread: hover is not a state
    /// you can act on later, so it earns no accent.
    pub hover: Color32,
    /// A pressed button, an open menu's button.
    pub press: Color32,
    /// Every 1 px separator and panel edge. 1.43:1 against `surface` —
    /// **decorative**, and legitimately so: a hairline is detected by edge
    /// contrast, not area contrast.
    pub line: Color32,
    /// Control borders, the splitter while dragged. 1.97:1 — also decorative.
    /// **Never the focus ring**, which must clear WCAG's 3:1 for a meaningful
    /// non-text indicator and therefore uses `accent` (6.47:1).
    pub line_strong: Color32,

    // --- Text ---
    /// Headings, tab labels, the selected row, a value being edited. 14.60:1.
    pub text_strong: Color32,
    /// Body — every control label and list row. 10.67:1.
    pub text: Color32,
    /// Field labels, units, counts, placeholder text. **4.83:1, which clears
    /// 4.5:1 by a hair — this may not be darkened without recomputing.**
    /// `contrast_floor_holds` in this module is that recomputation.
    pub text_weak: Color32,
    /// `add_enabled(false)`. 2.29:1 and **exempt**, therefore never the only
    /// carrier of meaning — a disabled command also states its reason.
    pub text_disabled: Color32,

    // --- Accent: warp violet ---
    /// Every thread, the focus ring, the active tool, the selection outline.
    /// 6.47:1.
    ///
    /// **Violet at ~260° because it is the furthest unclaimed hue**, and the
    /// argument is spatial rather than aesthetic: red, green and blue are the
    /// three gizmo axes, cyan is the agent, amber is warning. The default
    /// choice for a dark tool is blue — and blue is the Z axis, so a blue
    /// selection highlight in a 3D viewport can be misread as a depth handle.
    pub accent: Color32,
    /// The selection *fill* behind a row, at α90. Composited over `surface`
    /// that is `#353058`, on which `text_strong` reads 10.14:1.
    pub accent_deep: Color32,

    // --- Semantic. Each means one thing, everywhere, and nothing else. ---
    /// Validation passed, an assertion held, a brush that adds.
    pub ok: Color32,
    /// A physical-sanity finding, "this is two undo steps".
    pub warn: Color32,
    /// A parse failure, a rejected transaction, a brush that subtracts.
    pub error: Color32,
    /// **A recent write, never an owner.** The scene format records no
    /// authorship — `Transaction` carries `label`, `ops`, `dry_run` and
    /// `expect_version` and no author — so this marks the session-local
    /// decayed recency `run.rs` infers, and nothing more. Unchanged from the
    /// `(120, 200, 255)` the editor has used since M12, because it is a
    /// meaning a user has already learned.
    pub agent: Color32,
    /// X, everywhere, forever. 4.71:1 — the other row that only just clears,
    /// with the same prohibition as `text_weak`.
    pub axis_x: Color32,
    pub axis_y: Color32,
    pub axis_z: Color32,
    /// The dark under-stroke every viewport mark is drawn on, so a mark stays
    /// legible against both a snow field and a night sky.
    pub chrome_casing: Color32,
    /// The bright over-stroke for viewport marks carrying no semantic.
    pub chrome_core: Color32,
}

/// The palette, authored as screen-space hexes.
///
/// Read this table as "what a colour picker on a screenshot should report",
/// not as bytes handed to egui. [`tokens`] applies [`tok`].
const DARK: Tokens = Tokens {
    ground: Color32::from_rgb(0x0E, 0x10, 0x13),
    surface: Color32::from_rgb(0x16, 0x19, 0x1E),
    raised: Color32::from_rgb(0x1E, 0x23, 0x2A),
    sunken: Color32::from_rgb(0x0F, 0x12, 0x16),
    hover: Color32::from_rgb(0x26, 0x2C, 0x35),
    press: Color32::from_rgb(0x2F, 0x37, 0x42),
    line: Color32::from_rgb(0x2E, 0x35, 0x40),
    line_strong: Color32::from_rgb(0x41, 0x4A, 0x58),
    text_strong: Color32::from_rgb(0xE6, 0xEA, 0xF0),
    text: Color32::from_rgb(0xC3, 0xCA, 0xD4),
    text_weak: Color32::from_rgb(0x7C, 0x87, 0x94),
    text_disabled: Color32::from_rgb(0x4C, 0x55, 0x61),
    accent: Color32::from_rgb(0xA7, 0x8B, 0xFA),
    accent_deep: Color32::from_rgb(0x6E, 0x5B, 0xC4),
    ok: Color32::from_rgb(0x6F, 0xCF, 0x97),
    warn: Color32::from_rgb(0xE8, 0xB8, 0x4B),
    error: Color32::from_rgb(0xF0, 0x73, 0x6D),
    agent: Color32::from_rgb(0x78, 0xC8, 0xFF),
    axis_x: Color32::from_rgb(0xE2, 0x54, 0x4F),
    axis_y: Color32::from_rgb(0x7C, 0xC8, 0x60),
    axis_z: Color32::from_rgb(0x54, 0x94, 0xE8),
    chrome_casing: Color32::from_rgb(0x0A, 0x0C, 0x0F),
    chrome_core: Color32::from_rgb(0xF2, 0xF5, 0xFA),
};

/// The five tokens high contrast swaps, and nothing else.
///
/// The genuine accessibility need underneath "we should have a light theme"
/// is *contrast*, not polarity — the editor's main content is a lit 3D scene
/// whose average luminance the chrome does not control, and light chrome
/// around `cave` makes the viewport read as a hole. So there is no second
/// palette, only this patch and zoom.
const HIGH_CONTRAST: [(u32, u32); 5] = [
    (0x16_19_1E, 0x00_00_00),
    (0x1E_23_2A, 0x0C_0C_0C),
    (0x2E_35_40, 0x6B_74_84),
    (0x7C_87_94, 0xC3_CA_D4),
    (0x4C_55_61, 0x6B_74_84),
];

/// The palette, warped for the screen.
///
/// **Call this once and thread the result.** Calling [`tok`] at a use site is
/// how half the editor ends up double-corrected.
#[must_use]
pub fn tokens(high_contrast: bool) -> Tokens {
    let warp = |c: Color32| tok(u32::from_be_bytes([0, c.r(), c.g(), c.b()]));
    let hex = |h: u32| {
        #[allow(clippy::cast_possible_truncation)]
        Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
    };
    let mut t = DARK;
    if high_contrast {
        // Read out of the table rather than repeated beside it — the five
        // rows below are the *only* difference between the two themes, and a
        // second copy of them is a second thing to keep in step.
        t.surface = hex(HIGH_CONTRAST[0].1);
        t.raised = hex(HIGH_CONTRAST[1].1);
        t.line = hex(HIGH_CONTRAST[2].1);
        t.text_weak = hex(HIGH_CONTRAST[3].1);
        t.text_disabled = hex(HIGH_CONTRAST[4].1);
    }
    Tokens {
        ground: warp(t.ground),
        surface: warp(t.surface),
        raised: warp(t.raised),
        sunken: warp(t.sunken),
        hover: warp(t.hover),
        press: warp(t.press),
        line: warp(t.line),
        line_strong: warp(t.line_strong),
        text_strong: warp(t.text_strong),
        text: warp(t.text),
        text_weak: warp(t.text_weak),
        text_disabled: warp(t.text_disabled),
        accent: warp(t.accent),
        accent_deep: warp(t.accent_deep),
        ok: warp(t.ok),
        warn: warp(t.warn),
        error: warp(t.error),
        agent: warp(t.agent),
        axis_x: warp(t.axis_x),
        axis_y: warp(t.axis_y),
        axis_z: warp(t.axis_z),
        chrome_casing: warp(t.chrome_casing),
        chrome_core: warp(t.chrome_core),
    }
}

#[cfg(test)]
mod tests {
    use super::{DARK, HIGH_CONTRAST, Tokens, tok, tokens};
    use loom_render::egui::Color32;

    /// WCAG relative luminance of an **authored** (un-warped) colour.
    fn luminance(c: Color32) -> f64 {
        let ch = |b: u8| {
            let s = f64::from(b) / 255.0;
            if s <= 0.040_45 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        let (hi, lo) = if x > y { (x, y) } else { (y, x) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// **The doc comments state contrast ratios; this recomputes them.**
    ///
    /// `text_weak` and `axis_x` clear 4.5:1 by a hair, and the table says in
    /// two places that they may not be darkened without recomputing. A
    /// prohibition nobody can check is a comment, not a rule — so this is the
    /// check, and darkening either constant fails it.
    #[test]
    fn contrast_floor_holds() {
        let rows: [(&str, Color32, f64); 6] = [
            ("text_strong", DARK.text_strong, 14.60),
            ("text", DARK.text, 10.67),
            ("text_weak", DARK.text_weak, 4.83),
            ("accent", DARK.accent, 6.47),
            ("ok", DARK.ok, 9.3),
            ("error", DARK.error, 6.2),
        ];
        for (name, colour, stated) in rows {
            let actual = contrast(colour, DARK.surface);
            assert!(
                (actual - stated).abs() < 0.06,
                "{name}: doc says {stated:.2}:1, recomputed {actual:.2}:1"
            );
        }
        // The two that only just clear, asserted against the threshold
        // itself rather than against the stated number.
        for (name, colour) in [("text_weak", DARK.text_weak), ("axis_x", DARK.axis_x)] {
            assert!(
                contrast(colour, DARK.surface) >= 4.5,
                "{name} fell below WCAG 4.5:1"
            );
        }
    }

    /// `tok` must be the inverse of the shader-plus-hardware pipeline: a
    /// token authored as `h` has to *arrive* as `h`.
    ///
    /// This replays the three stages the module docs name, in order, and
    /// checks the byte that lands in the framebuffer.
    #[test]
    fn tok_round_trips_through_the_real_pipeline() {
        let srgb_encode = |linear: f64| {
            if linear <= 0.003_130_8 {
                linear * 12.92
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            }
        };
        for hex in [0x0E_10_13u32, 0x16_19_1E, 0xE6_EA_F0, 0xA7_8B_FA, 0x00_00_00] {
            let warped = tok(hex);
            for (shift, got) in [(16, warped.r()), (8, warped.g()), (0, warped.b())] {
                let authored = (hex >> shift) & 0xFF;
                // Stage 1: the vertex shader. Stage 2 is a pass-through
                // under `srgb_framebuffer: true`. Stage 3: the hardware.
                let linear = (f64::from(got) / 255.0).powf(2.2);
                let on_screen = (srgb_encode(linear) * 255.0).round();
                assert!(
                    (on_screen - f64::from(authored)).abs() <= 1.0,
                    "authored {authored}, arrived {on_screen}"
                );
            }
        }
    }

    /// The naive path — no `tok` — must be visibly wrong, or `tok` is
    /// correcting nothing and should be deleted.
    ///
    /// Doc 11 states `#16191E` arrives as `#0E0E10` without it. **That value
    /// is wrong, and this test is how it became known to be wrong** — the
    /// true arrival is `#0E1218`. The doc collapses three channels onto one
    /// correction, but the transfer function is per-channel and non-linear,
    /// so `0x19` and `0x1E` do not land where `0x16` does.
    ///
    /// Doc 11's *magnitude* claim survives and nothing downstream moves:
    /// `0x16` → `0x0E` is 36% darker, which is the "about 35%" it argues
    /// from.
    #[test]
    fn without_tok_the_panel_is_a_third_too_dark() {
        let srgb_encode = |linear: f64| 1.055 * linear.powf(1.0 / 2.4) - 0.055;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let arrived = |b: u32| {
            let linear = (f64::from(b) / 255.0).powf(2.2);
            (srgb_encode(linear) * 255.0).round() as u32
        };
        assert_eq!([arrived(0x16), arrived(0x19), arrived(0x1E)], [0x0E, 0x12, 0x18]);
        // The claim the design actually rests on.
        assert!(1.0 - f64::from(arrived(0x16)) / 22.0 > 0.30);
    }

    /// High contrast swaps five tokens and leaves the rest alone.
    #[test]
    fn high_contrast_touches_exactly_five() {
        let a = tokens(false);
        let b = tokens(true);
        let pairs: [(Color32, Color32); 23] = [
            (a.ground, b.ground),
            (a.surface, b.surface),
            (a.raised, b.raised),
            (a.sunken, b.sunken),
            (a.hover, b.hover),
            (a.press, b.press),
            (a.line, b.line),
            (a.line_strong, b.line_strong),
            (a.text_strong, b.text_strong),
            (a.text, b.text),
            (a.text_weak, b.text_weak),
            (a.text_disabled, b.text_disabled),
            (a.accent, b.accent),
            (a.accent_deep, b.accent_deep),
            (a.ok, b.ok),
            (a.warn, b.warn),
            (a.error, b.error),
            (a.agent, b.agent),
            (a.axis_x, b.axis_x),
            (a.axis_y, b.axis_y),
            (a.axis_z, b.axis_z),
            (a.chrome_casing, b.chrome_casing),
            (a.chrome_core, b.chrome_core),
        ];
        let changed = pairs.iter().filter(|(x, y)| x != y).count();
        assert_eq!(changed, HIGH_CONTRAST.len(), "high contrast changed {changed} tokens");

        // **The table's left column must still name the colour it replaces.**
        // Without this, retuning `surface` in `DARK` leaves the high-contrast
        // row pointing at a hex that is no longer in the palette, and the
        // table silently stops documenting anything.
        let from = |c: Color32| (u32::from(c.r()) << 16) | (u32::from(c.g()) << 8) | u32::from(c.b());
        for (row, authored) in HIGH_CONTRAST.iter().zip([
            DARK.surface,
            DARK.raised,
            DARK.line,
            DARK.text_weak,
            DARK.text_disabled,
        ]) {
            assert_eq!(row.0, from(authored), "high-contrast row names a stale colour");
        }
    }

    /// A guard against the `Tokens` struct growing a field that `tokens()`
    /// forgets to warp — which would ship one raw hex among 23 corrected
    /// ones and look like a subtly wrong shade rather than a bug.
    #[test]
    fn every_token_is_warped() {
        let raw = DARK;
        let t: Tokens = tokens(false);
        let pairs: [(Color32, Color32); 23] = [
            (raw.ground, t.ground),
            (raw.surface, t.surface),
            (raw.raised, t.raised),
            (raw.sunken, t.sunken),
            (raw.hover, t.hover),
            (raw.press, t.press),
            (raw.line, t.line),
            (raw.line_strong, t.line_strong),
            (raw.text_strong, t.text_strong),
            (raw.text, t.text),
            (raw.text_weak, t.text_weak),
            (raw.text_disabled, t.text_disabled),
            (raw.accent, t.accent),
            (raw.accent_deep, t.accent_deep),
            (raw.ok, t.ok),
            (raw.warn, t.warn),
            (raw.error, t.error),
            (raw.agent, t.agent),
            (raw.axis_x, t.axis_x),
            (raw.axis_y, t.axis_y),
            (raw.axis_z, t.axis_z),
            (raw.chrome_casing, t.chrome_casing),
            (raw.chrome_core, t.chrome_core),
        ];
        for (before, after) in pairs {
            assert_eq!(
                after,
                tok(u32::from_be_bytes([0, before.r(), before.g(), before.b()])),
                "a token in `Tokens` is not the warp of its `DARK` entry"
            );
        }
        // **The warp has fixed points at both ends of the range**, so the
        // check above cannot catch a forgotten field there — `#F2F5FA` warps
        // to itself, and so does black. It catches every mid-tone, which is
        // where the whole palette lives. Asserted rather than left implicit,
        // because "every token is warped" is a stronger sentence than this
        // test can actually write.
        let raw_core = u32::from_be_bytes([0, raw.chrome_core.r(), raw.chrome_core.g(), raw.chrome_core.b()]);
        assert_eq!(tok(raw_core), raw.chrome_core, "chrome_core stopped being a fixed point");
        assert_ne!(
            tok(u32::from_be_bytes([0, raw.surface.r(), raw.surface.g(), raw.surface.b()])),
            raw.surface,
            "surface must move, or `tok` is doing nothing"
        );
    }
}
