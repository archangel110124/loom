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
/// **Adding an effect means adding a row here, a row to `xtask`'s own table, AND a matching
/// `static const uint LOOM_ABLATE_<NAME>` in `assets/shaders/scene.slang`** (they sit
/// together next to `ablated()`). The three are deliberately not shared —
/// `xtask` links nothing from the engine, because a gate that shares code with the thing it
/// checks can be fooled by the same bug twice, and Slang cannot import a Rust constant at
/// all. **Nothing enforces that the Slang constant's value matches the bit assigned here.**
/// A mismatch fails open: `LOOM_ABLATE=<name>` would ablate whichever effect actually holds
/// that bit in Slang, the frame would still move past the floor, and `cargo xtask ablate`
/// would report `ok` having never measured the intended effect.
///
/// Bits are assigned explicitly rather than by position, so reordering this table cannot
/// silently repoint an existing name at a different effect.
pub const ABLATIONS: &[(&str, u32)] = &[
    ("water_foam", 1 << 0),
    ("ocean_spectrum", 1 << 1),
    ("water_glow", 1 << 2),
    ("spray_haze", 1 << 3),
    ("spray_droplets", 1 << 4),
];

/// The bit for whitecap and swash foam on the water surface.
///
/// Must equal `LOOM_ABLATE_WATER_FOAM` in `assets/shaders/scene.slang` — see the warning on
/// [`ABLATIONS`] above; nothing checks that these two agree.
pub const WATER_FOAM: u32 = 1 << 0;

/// The bit for the FFT cascade that displaces a `wave_model = "spectrum"` surface — ADR
/// 0076.
///
/// Ablating it makes `waterVertexMain` read `sea.count == 0`, which is the same value a
/// Gerstner body carries, so the shader takes its `loom_sample_water` branch. A spectrum
/// body authors no wave list — that is refused at load — so that branch sums nothing and
/// draws the flat plane. Removing the cascade therefore removes the whole sea, which is
/// what makes it worth measuring: the failure this gate exists to catch is a cascade that
/// is uploaded, sampled and contributing nothing.
///
/// Must equal `LOOM_ABLATE_OCEAN_SPECTRUM` in `assets/shaders/scene.slang` — see the
/// warning on [`ABLATIONS`] above; nothing checks that these two agree.
pub const OCEAN_SPECTRUM: u32 = 1 << 1;

/// The bit for the backlit subsurface term on the water surface.
///
/// Ablating it zeroes `sss` at its single point of use, so what is measured is the light
/// that came up through the water and nothing else. **The registered scene is
/// `ocean_tropical`**: its Beaufort 4 sea never reaches `WATER_FOLD_REF`, so before `lift`
/// replaced `crest` this term drew nothing at all there — the exact failure this harness
/// exists to catch, and a blessed reference of it would have recorded the absence for ever.
///
/// Must equal `LOOM_ABLATE_WATER_GLOW` in `assets/shaders/scene.slang` — see the warning on
/// [`ABLATIONS`] above; nothing checks that these two agree.
pub const WATER_GLOW: u32 = 1 << 2;

/// The bit for the wind-blown sea-spray haze — the second exponential layer in
/// `fogAmount`.
///
/// Ablating it skips that layer's optical depth and leaves the atmosphere term untouched,
/// so what is measured is the haze alone. **The registered scene is `ocean_fft_storm`**:
/// Monahan's `U10^3.41` spans four orders across this repository's seas, so the gate has to
/// sit on a storm or it would pass on an effect that had stopped drawing everywhere below
/// a gale — `whitecaps`, at U10 16.4, moves 0.5% of its frame.
///
/// Must equal `LOOM_ABLATE_SPRAY_HAZE` in `assets/shaders/scene.slang` — see the warning on
/// [`ABLATIONS`] above; nothing checks that these two agree.
pub const SPRAY_HAZE: u32 = 1 << 3;

/// The bit for the thrown spray droplets — the particles a breaking crest launches.
///
/// **The one ablation with no `LOOM_ABLATE_` constant in `scene.slang`, and that is not an
/// oversight.** Every other effect here is a term in a shader; this one is a CPU particle
/// population built in `loom_cli::particles::spray`, so the switch is an early return
/// there rather than a branch the GPU takes. The warning on [`ABLATIONS`] about keeping
/// the Slang constant in step does not apply — there is nothing to keep it in step with,
/// and adding one would create the mismatch it warns about.
///
/// It is read through [`mask`] rather than [`ablation_mask`], off the same `OnceLock`, so
/// the CPU and the shader cannot disagree about what this process is ablating.
///
/// Ablating it removes the droplets and leaves the haze alone — the two halves of spray
/// are separately measurable, which is the point of splitting them: a medium and a
/// particle population fail in different ways, and one covering for the other is exactly
/// the false negative this harness exists to catch.
pub const SPRAY_DROPLETS: u32 = 1 << 4;

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

/// The mask this process is running with.
///
/// Read once, like [`crate::renderer::ao_rays`] and for the same reason: nothing polls it,
/// and re-reading the environment per frame would be a syscall in the hot path for a value
/// that cannot change.
///
/// **Public, because not every ablatable effect is a shader term.** [`SPRAY_DROPLETS`] is a
/// CPU particle population, and its switch is an early return in `loom_cli`. Sharing this
/// `OnceLock` rather than parsing `LOOM_ABLATE` a second time over there is what stops the
/// two halves of the frame disagreeing about what is being measured.
///
/// **Panics** on an unknown name, at first use, with the list of valid ones. A measurement
/// tool that carried on after being misconfigured would report a number nobody could trust.
pub fn mask() -> u32 {
    static MASK: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MASK.get_or_init(
        || match parse_ablations(std::env::var("LOOM_ABLATE").ok().as_deref()) {
            Ok(mask) => mask,
            Err(message) => panic!("{message}"),
        },
    )
}

/// The same mask as [`mask`], carried to the shader in `viewport.w`.
pub(crate) fn ablation_mask() -> f32 {
    #[allow(clippy::cast_precision_loss)]
    let carried = mask() as f32;
    carried
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
        assert_eq!(parse_ablations(Some("ocean_spectrum")), Ok(OCEAN_SPECTRUM));
        assert_eq!(parse_ablations(Some("water_glow")), Ok(WATER_GLOW));
        assert_eq!(parse_ablations(Some("spray_haze")), Ok(SPRAY_HAZE));
        assert_eq!(parse_ablations(Some("spray_droplets")), Ok(SPRAY_DROPLETS));
        assert_eq!(
            parse_ablations(Some("water_foam,ocean_spectrum")),
            Ok(WATER_FOAM | OCEAN_SPECTRUM)
        );
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
