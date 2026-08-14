//! Rain, synthesised rather than sampled.
//!
//! # Why there is no rain loop in `assets/audio/`
//!
//! Property 1 of this project is that everything authored is diffable text. A
//! rain loop is an opaque binary blob: nobody can review a change to it, and
//! "the rain got louder" is not a line in a commit. Rain is also one of the few
//! sounds synthesis genuinely suits — it *is* broadband noise, and its character
//! is spectral shape plus the density of individual impacts, both of which are
//! parameters rather than recordings.
//!
//! Two things fall out of that which a clip could not give:
//!
//! - **Intensity is continuous.** Drizzle to downpour is one number moving, not
//!   a crossfade between two recordings that were made in different places.
//! - **It never repeats.** The noise is a function of a monotonically rising
//!   sample counter, so there is no loop point. A short looping buffer is
//!   audibly periodic, and "you can see the texture moving back and forth" is
//!   the exact complaint that was made about the *visual* rain lattice. The
//!   same mistake is easier to make here and harder to notice.
//!
//! # What makes it rain rather than static
//!
//! White noise alone is a television between channels. Rain is three things at
//! once, and the mix between them is what the ear reads as *how hard it is
//! raining*:
//!
//! - **Hiss** — the high band, the sound of many small impacts far away. It
//!   dominates in drizzle.
//! - **Roar** — the low band, which rises with rate. A downpour has a body to
//!   it that light rain does not.
//! - **Patter** — sparse, sharp transients: individual drops landing near you.
//!   This is the part static has none of, and leaving it out is the classic way
//!   synthesised rain fails.
//!
//! # Determinism
//!
//! The noise comes from [`loom_field::noise::hash`], the frozen lattice mixer,
//! indexed by sample number. So a render is reproducible byte for byte, which is
//! what makes an offline audio gate possible at all. Audio is an output and is
//! not in the simulation hash; the hash is reused because there is no reason to
//! introduce a second random source with its own failure modes.

/// One white-noise sample in `[-1, 1)` from a counter and a stream id.
///
/// **A stream id rather than one sequence sliced up.** The hiss, the roar and
/// the patter trigger all need noise at the same instant; drawing them from one
/// counter would correlate them, and correlated "independent" noise sources
/// comb-filter into a metallic ring.
fn white(cursor: u64, stream: u32) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    let h = loom_field::noise::hash((cursor as u32).wrapping_add(stream.wrapping_mul(0x9E37_79B9)));
    // Top 24 bits, exactly representable in an f32 — the same slice `lattice`
    // takes, for the same reason.
    f32::from_bits((h >> 8) | 0x3F80_0000).mul_add(2.0, -3.0)
}

/// A one-pole low-pass, as a coefficient for a given cutoff.
fn one_pole(cutoff_hz: f32, sample_rate: f32) -> f32 {
    let x = (-std::f32::consts::TAU * cutoff_hz / sample_rate.max(1.0)).exp();
    1.0 - x
}

/// How hard it is raining, and how much of the sky the listener can see.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RainAudio {
    /// Rate in mm/h, unsheltered — the same number `loom_rain` works in.
    pub intensity: f32,
    /// `0.0` fully sheltered, `1.0` open sky.
    ///
    /// **This is [`crate::Acoustics::openness`], not S3's sky exposure**, and
    /// the distinction matters. S3 marches the voxel volume only, so it reads
    /// any non-voxel scene as wide open; `openness` casts against the collision
    /// world. Since ADR 0017 the *visible* rain is occluded by the collision
    /// world too, so these finally measure the same geometry — and using S3
    /// here would make `rain_gantry`'s mesh deck look sheltered and sound
    /// exposed.
    pub openness: f32,
    /// Overall trim, so a scene can be quieter without lying about the rate.
    pub volume: f32,
}

impl Default for RainAudio {
    fn default() -> Self {
        Self { intensity: 0.0, openness: 1.0, volume: 1.0 }
    }
}

/// The rain bed: a continuous, non-repeating ambient source.
///
/// Not a [`crate::Voice`]. A voice is positional — it has a distance, a range
/// and a pan — and rain has none of those: it surrounds the listener and does
/// not get quieter as you walk toward it. Bolting an ambient bed onto the
/// positional path would mean a distance that must be held at zero and a range
/// that must be held at infinity, both of which are lies that some later change
/// would trip over.
#[derive(Debug, Clone)]
pub struct RainBed {
    sample_rate: f32,
    /// Monotonic sample counter. **Never wraps in practice and never resets**:
    /// this is what guarantees the texture has no loop point.
    cursor: u64,
    /// One-pole states for the two bands and the shelter filter.
    roar: f32,
    hiss: f32,
    muffle: f32,
    /// The decaying envelope of recent drop impacts.
    patter: f32,
    /// Smoothed parameters. Rain does not start and stop between one buffer and
    /// the next, and a level that jumps by a buffer boundary clicks — the same
    /// reason `Voice` carries its low-pass state across buffers.
    level: f32,
    tilt: f32,
}

/// Seconds for the level to follow a change in the weather.
///
/// Long enough that walking under a roof is a fade rather than a switch, short
/// enough that it is not lagging behind what the eye can already see.
const SMOOTHING_SECONDS: f32 = 0.35;

/// How much of the top end a full shelter removes.
///
/// Under a roof rain is not merely quieter — it is *darker*, because the direct
/// high-frequency impacts near you are gone and what is left arrives around a
/// corner. Level alone reads as someone turning a volume knob.
const SHELTERED_CUTOFF_HZ: f32 = 700.0;
const OPEN_CUTOFF_HZ: f32 = 16_000.0;

/// What a fully sheltered listener still hears, as a fraction.
///
/// **Not zero.** Standing under a roof in a downpour is not silent; the rain is
/// still audible all around and on the roof itself. Zero here is the audio
/// equivalent of the bug where walking under an overhang stopped every drop in
/// the world.
/// **0.55 rather than 0.35, and the reason is that the filter also attenuates.**
/// At 0.35 a sheltered listener measured 21 dB below an open one, because the
/// 700 Hz low-pass removes most of a broadband signal's energy on its own. A
/// 21 dB drop does not read as *sheltered*, it reads as *the rain stopped* —
/// which is the exact immersion complaint the visible layer already had once.
const SHELTERED_LEVEL: f32 = 0.55;

/// The rate, in mm/h, at which the bed reaches full loudness.
const FULL_RATE: f32 = 12.0;

/// Master trim, so the loudest case stays inside `[-1, 1]`.
///
/// **Measured, not chosen.** Without it a 40 mm/h downpour peaks at 1.23 — the
/// patter transients ride on top of a full-level bed — and the test below
/// caught it on the first run. Headroom rather than a clamp: clamping a
/// transient is distortion, and distortion on a noise bed sounds like a blown
/// speaker rather than like heavy rain.
const HEADROOM: f32 = 0.6;

impl RainBed {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1) as f32,
            cursor: 0,
            roar: 0.0,
            hiss: 0.0,
            muffle: 0.0,
            patter: 0.0,
            level: 0.0,
            tilt: 1.0,
        }
    }

    /// Add `seconds` of rain into `out`, which is interleaved stereo.
    ///
    /// **Adds rather than overwrites**, so the bed sits under the positional
    /// voices the mixer has already rendered.
    pub fn render(&mut self, params: RainAudio, out: &mut [f32]) {
        let target_level = if params.intensity <= 0.0 {
            0.0
        } else {
            let rate = (params.intensity / FULL_RATE).clamp(0.0, 1.0);
            // Loudness rises fast at first and then flattens, which is how rate
            // actually maps to perceived level — the difference between 1 and
            // 4 mm/h is far more audible than between 20 and 24.
            let shelter = SHELTERED_LEVEL + (1.0 - SHELTERED_LEVEL) * params.openness.clamp(0.0, 1.0);
            rate.sqrt() * shelter * params.volume.max(0.0)
        };
        let target_tilt = params.openness.clamp(0.0, 1.0);

        let smoothing = one_pole(1.0 / SMOOTHING_SECONDS, self.sample_rate);
        // The two bands. Fixed cutoffs: these describe rain, not a scene.
        let roar_k = one_pole(420.0, self.sample_rate);
        let hiss_k = one_pole(3_000.0, self.sample_rate);

        for frame in out.chunks_mut(2) {
            self.level += (target_level - self.level) * smoothing;
            self.tilt += (target_tilt - self.tilt) * smoothing;

            // Two decorrelated noise streams so the left and right ears are not
            // handed the same signal — rain arriving identically at both ears
            // collapses to a point in the middle of your head.
            let n_l = white(self.cursor, 1);
            let n_r = white(self.cursor, 2);

            // Roar: the low band, present in proportion to how hard it rains.
            self.roar += (n_l - self.roar) * roar_k;
            // Hiss: the high band, taken as what the low-pass did not keep.
            self.hiss += (n_r - self.hiss) * hiss_k;
            let hiss = n_r - self.hiss;

            // Patter: sparse impacts. The trigger rate rises with the level, so
            // a downpour is dense enough to read as texture and drizzle is
            // individual drops.
            let density = 0.0006 + 0.02 * self.level;
            let roll = white(self.cursor, 3).mul_add(0.5, 0.5);
            if roll < density {
                self.patter = 1.0;
            }
            // ~8 ms decay: a drop impact is a click, not a note.
            self.patter *= 1.0 - one_pole(120.0, self.sample_rate);
            let patter = self.patter * white(self.cursor, 4) * 0.8;

            // Heavier rain is weighted toward the roar; light rain is mostly
            // hiss. This is the mix the ear reads as rate, independently of
            // level, and it is why turning the volume up does not make drizzle
            // sound like a storm.
            let body = self.level.mul_add(0.55, 0.15);
            let dry = (self.roar * 2.2 * body) + hiss * (1.0 - body) + patter;

            // Shelter darkens as well as quietens. One filter, driven by the
            // same `openness`, for the reason `Acoustics::underwater` gives:
            // a second filter beside it is a second thing to disagree.
            let cutoff = SHELTERED_CUTOFF_HZ + (OPEN_CUTOFF_HZ - SHELTERED_CUTOFF_HZ) * self.tilt;
            let muffle_k = one_pole(cutoff, self.sample_rate);
            self.muffle += (dry - self.muffle) * muffle_k;

            let value = self.muffle * self.level * HEADROOM;
            // Decorrelate the ears slightly rather than duplicating: the same
            // sample in both is a point source between your eyes.
            frame[0] += value;
            if frame.len() > 1 {
                frame[1] += value * 0.92 + patter * self.level * 0.08;
            }

            self.cursor = self.cursor.wrapping_add(1);
        }
    }
}

/// What a rendered buffer measures, for tests and for the CLI.
///
/// **Measured quantities rather than a byte hash**, for the same reason the
/// image gate has a calibrated tolerance: a hash pins the arithmetic, and every
/// harmless change to the arithmetic then reads as a failure. What is worth
/// asserting is that heavier rain is louder and that shelter is darker — claims
/// about the sound, which survive a refactor and fail when the sound is wrong.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measurement {
    /// Root-mean-square level of the whole buffer.
    pub rms: f32,
    /// Peak absolute sample. Over 1.0 is clipping.
    pub peak: f32,
    /// High-band energy over low-band energy, split at 1 kHz.
    ///
    /// **The number that says "darker".** Level alone cannot tell a quiet
    /// downpour from a sheltered one, and the whole point of the shelter filter
    /// is that those are different sounds.
    pub tilt: f32,
}

/// Measure a rendered buffer. Interleaving does not matter: this is energy.
#[must_use]
pub fn measure(samples: &[f32], sample_rate: u32) -> Measurement {
    if samples.is_empty() {
        return Measurement { rms: 0.0, peak: 0.0, tilt: 0.0 };
    }
    let k = one_pole(1_000.0, sample_rate.max(1) as f32);
    let mut low_state = 0.0_f32;
    let (mut sum, mut peak, mut low, mut high) = (0.0_f64, 0.0_f32, 0.0_f64, 0.0_f64);
    for &s in samples {
        sum += f64::from(s) * f64::from(s);
        peak = peak.max(s.abs());
        low_state += (s - low_state) * k;
        low += f64::from(low_state) * f64::from(low_state);
        let h = s - low_state;
        high += f64::from(h) * f64::from(h);
    }
    #[allow(clippy::cast_possible_truncation)]
    Measurement {
        rms: (sum / samples.len() as f64).sqrt() as f32,
        peak,
        tilt: (high.sqrt() / (low.sqrt() + 1e-9)) as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn render(params: RainAudio, seconds: f32) -> Vec<f32> {
        let mut bed = RainBed::new(RATE);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = (RATE as f32 * seconds) as usize;
        let mut out = vec![0.0; frames * 2];
        bed.render(params, &mut out);
        out
    }

    /// The guard every other test here rests on. If the bed rendered silence,
    /// every comparison below would be zero against zero and would pass.
    #[test]
    fn dry_is_silent_and_wet_is_not() {
        let dry = measure(&render(RainAudio::default(), 0.5), RATE);
        assert_eq!(dry.rms, 0.0, "a scene with no rain must render exact silence");

        let wet = measure(
            &render(RainAudio { intensity: 8.0, ..RainAudio::default() }, 0.5),
            RATE,
        );
        assert!(wet.rms > 0.01, "8 mm/h rendered essentially nothing: {}", wet.rms);
    }

    #[test]
    fn heavier_rain_is_louder() {
        let mut last = 0.0;
        for intensity in [1.0, 4.0, 8.0, 16.0] {
            let m = measure(
                &render(RainAudio { intensity, ..RainAudio::default() }, 1.0),
                RATE,
            );
            assert!(
                m.rms > last,
                "{intensity} mm/h is not louder than the rate below it: {} vs {last}",
                m.rms
            );
            last = m.rms;
        }
    }

    /// **Shelter is quieter *and* darker, and the second half is the point.**
    /// A version that only changed the level would pass a loudness test and
    /// still sound like someone turning a knob.
    #[test]
    fn shelter_is_quieter_and_darker() {
        let open = measure(
            &render(RainAudio { intensity: 8.0, openness: 1.0, volume: 1.0 }, 1.0),
            RATE,
        );
        let under = measure(
            &render(RainAudio { intensity: 8.0, openness: 0.0, volume: 1.0 }, 1.0),
            RATE,
        );
        assert!(under.rms < open.rms, "shelter did not quieten: {} vs {}", under.rms, open.rms);
        assert!(under.rms > 0.0, "shelter silenced the rain entirely — a roof is not a mute");
        assert!(
            under.tilt < open.tilt * 0.7,
            "shelter did not darken the sound: tilt {} against {} open",
            under.tilt,
            open.tilt
        );
    }

    /// Rate changes the *character*, not only the level. Drizzle is hissier
    /// than a downpour whatever the volume knob says.
    #[test]
    fn drizzle_is_brighter_than_a_downpour() {
        let light = measure(
            &render(RainAudio { intensity: 1.0, ..RainAudio::default() }, 1.0),
            RATE,
        );
        let heavy = measure(
            &render(RainAudio { intensity: 24.0, ..RainAudio::default() }, 1.0),
            RATE,
        );
        assert!(
            light.tilt > heavy.tilt,
            "drizzle is not brighter than a downpour: {} against {}",
            light.tilt,
            heavy.tilt
        );
    }

    #[test]
    fn it_does_not_clip() {
        let m = measure(
            &render(RainAudio { intensity: 40.0, volume: 1.0, openness: 1.0 }, 2.0),
            RATE,
        );
        assert!(m.peak <= 1.0, "the bed clipped at {} — a downpour must stay in range", m.peak);
    }

    /// **No loop point.** A short looping buffer is audibly periodic, and the
    /// visual rain shipped exactly that bug: a lattice whose period the human
    /// could see sliding. Compare the first second against the fifth; if the
    /// generator repeated on any period dividing four seconds they would match.
    #[test]
    fn the_texture_never_repeats() {
        let params = RainAudio { intensity: 8.0, ..RainAudio::default() };
        let long = render(params, 5.0);
        let frame = RATE as usize * 2;
        let first = &long[..frame];
        let fifth = &long[frame * 4..frame * 5];
        let identical = first.iter().zip(fifth).filter(|(a, b)| a == b).count();
        assert!(
            identical * 100 < frame,
            "{identical} of {frame} samples repeat after four seconds — the bed has a loop point"
        );
    }

    /// Two renders of the same parameters must agree exactly, or the offline
    /// gate cannot exist.
    #[test]
    fn it_is_deterministic() {
        let params = RainAudio { intensity: 8.0, openness: 0.4, volume: 1.0 };
        assert_eq!(render(params, 0.25), render(params, 0.25));
    }
}
