//! `LOOM_ABLATE` — switch one effect off, so that its contribution can be measured.
//!
//! **This exists because no gate in this project can detect an absent feature.**
//! `Renderer::set_ripples` shipped with no caller and drew dead-flat water through a
//! commit, an ADR and a review; the W9 crown was registered, closed-form-correct and
//! drawn nowhere; grass ran two slices outside `GOLDEN`. In each case a reference image
//! recorded the absence and passed for ever. An ablation asks the opposite question —
//! *what changes if this is removed* — and a feature that is not drawing answers zero.
//!
//! **An environment variable rather than a scene field**, for the reason `ao_rays` gives
//! for the same choice: this is a property of the measurement being taken, not of the
//! scene, and a scene-authored value would make the golden images photograph whatever
//! each `.loom` happened to say.
//!
//! **An unknown name is a hard error and not a silent no-op.** A typo that quietly
//! ablates nothing reports "no difference", which is indistinguishable from the failure
//! this harness exists to catch — it would turn the instrument into a machine for
//! producing false negatives.

/// Every ablatable effect, and its bit.
///
/// **Adding an effect means adding a row here and a row to `xtask`'s own table.** The two
/// are deliberately not shared: `xtask` links nothing from the engine, because a gate that
/// shares code with the thing it checks can be fooled by the same bug twice.
///
/// Bits are assigned explicitly rather than by position, so reordering this table cannot
/// silently repoint an existing name at a different effect.
pub const ABLATIONS: &[(&str, u32)] = &[("water_foam", 1 << 0)];

/// The bit for whitecap and swash foam on the water surface.
pub const WATER_FOAM: u32 = 1 << 0;

/// Parse a `LOOM_ABLATE` value into a mask.
///
/// Comma-separated, whitespace around each name ignored, repeats idempotent. `None` and
/// the empty string are both "ablate nothing", which is every ordinary run.
pub fn parse_ablations(spec: Option<&str>) -> Result<u32, String> {
    let mut mask = 0;
    for name in spec.unwrap_or("").split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let Some((_, bit)) = ABLATIONS.iter().find(|(known, _)| *known == name) else {
            let known: Vec<&str> = ABLATIONS.iter().map(|(n, _)| *n).collect();
            return Err(format!(
                "LOOM_ABLATE: unknown effect {name:?}; known effects are {}",
                known.join(", ")
            ));
        };
        mask |= bit;
    }
    Ok(mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_asked_for_is_nothing_ablated() {
        assert_eq!(parse_ablations(None), Ok(0));
        assert_eq!(parse_ablations(Some("")), Ok(0));
        assert_eq!(parse_ablations(Some("   ")), Ok(0));
    }

    #[test]
    fn a_name_is_its_bit() {
        assert_eq!(parse_ablations(Some("water_foam")), Ok(WATER_FOAM));
    }

    #[test]
    fn whitespace_and_repeats_do_not_change_the_mask() {
        assert_eq!(parse_ablations(Some("  water_foam  ")), Ok(WATER_FOAM));
        assert_eq!(parse_ablations(Some("water_foam,water_foam")), Ok(WATER_FOAM));
    }

    /// **The test this module exists for.** A misspelling that parsed to zero would
    /// render an unablated frame, compare it against another unablated frame, and report
    /// "no difference" — which reads exactly like a feature that is not drawing.
    #[test]
    fn an_unknown_name_is_refused_and_says_what_is_valid() {
        let err = parse_ablations(Some("water_form")).unwrap_err();
        assert!(err.contains("water_form"), "the bad name is not in: {err}");
        assert!(err.contains("water_foam"), "the valid names are not in: {err}");
    }

    #[test]
    fn one_bad_name_refuses_the_whole_list() {
        assert!(parse_ablations(Some("water_foam,nonsense")).is_err());
    }

    /// The mask travels in an `f32` (`viewport.w`), whose mantissa is 24 bits, so every
    /// bit up to `1 << 23` survives the round trip exactly. This asserts the table has
    /// not outgrown its carrier.
    #[test]
    fn every_bit_survives_an_f32() {
        for (name, bit) in ABLATIONS {
            assert!(*bit <= 1 << 23, "{name}'s bit {bit} does not fit an f32 mantissa");
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let round_tripped = *bit as f32 as u32;
            assert_eq!(round_tripped, *bit, "{name} does not survive an f32");
        }
    }

    #[test]
    fn no_two_effects_share_a_bit() {
        let mut seen = 0_u32;
        for (name, bit) in ABLATIONS {
            assert_eq!(seen & bit, 0, "{name}'s bit is already taken");
            seen |= bit;
        }
    }
}
