//! The sea's sound: a rumble, a hiss, and the ratio between them.
//!
//! # Two sounds, not one
//!
//! A sea is a **low rumble** from swell moving water and a **broadband hiss**
//! from breaking crests, and the ratio between them is what tells you, with
//! your eyes shut, whether you are hearing a swell or a gale. Those are the two
//! quantities the water already computes per tick: significant wave height, and
//! the fraction of the surface past the breaking threshold. So the bed is
//! *derived*, not authored, for the reason `loom_water::spectrum` gives for
//! refusing to let anybody author sixteen amplitudes — a sea has one honest
//! input and every other number about it follows.
//!
//! The consequence worth naming: **`breaking` is not a volume knob.** It feeds
//! the high band only, so raising it changes the *colour* of the sound while
//! `hs` sets the level. That is what `a_breaking_sea_is_brighter` asserts, and
//! the reason this file exists rather than a gain on one noise source.
//!
//! # Determinism
//!
//! Noise comes from [`loom_field::noise::hash`], the frozen integer lattice
//! mixer, indexed by sample number — not a crate, not `thread_rng`, not a chain
//! of `sin`. [`bed`] is a pure function with its filter state on the stack, so
//! the same state and seed give byte-identical samples, which is what makes an
//! offline render comparable at all. The two bands are summed in a fixed order
//! (swell, then hiss) because float addition is not associative and this
//! crate's output is compared byte for byte.
//!
//! **One call of [`bed`] is one continuous stretch.** It makes its own
//! [`SeaBed`], whose filter states and sample cursor start at zero, so calling
//! it repeatedly for successive buffers restarts the texture and clicks at the
//! joins. Successive buffers hold one [`SeaBed`] across them instead — the way
//! [`crate::rain::RainBed`] carries its own — and that is the path the mixer
//! takes.
//!
//! # Where the numbers come from
//!
//! Every constant below is either derived from the water's own numbers or
//! marked as not derived. **Nobody has listened to this yet** — it was built
//! and measured, not auditioned — so "chosen by ear" appears nowhere; the two
//! constants that want a human's ear say so at their definition.

/// What the sound is a function of, and nothing else.
///
/// These are the water's numbers, passed in rather than re-derived: a second
/// opinion about how rough the sea is would be free to disagree with the one
/// the boat floats on.
///
/// **[`Default`] is a dead flat calm, and that is the load-bearing default.** A
/// scene with no `WaterBody` hands this in, and [`bed`] answers it with exact
/// zeros — so every scene that has no sea sounds byte for byte as it did before
/// there was one.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SeaState {
    /// Significant wave height in metres — `loom_water::significant_height`.
    pub hs: f32,
    /// Fraction of the surface past the breaking threshold, `0.0` to `1.0`.
    ///
    /// A real sea reaches only the low end of that: `loom_water::spray`'s
    /// `SPRAY_BREAK` sits at 1.75σ of the fold distribution, which its own
    /// documentation calls "the steepest few percent of the surface, not every
    /// crest". [`BREAK_FULL`] is sized for that.
    pub breaking: f32,
    /// Wind speed at 10 m, in m/s. Colour only — see [`HISS_GALE_HZ`].
    pub wind: f32,
}

/// The Hs at which the swell rumble reaches full level, in metres.
///
/// **Derived.** It is the top rung of this engine's sea: `SEA-REBUILD.md` §3.6
/// targets Hs = 6.10 m at U10 = 18 m/s and 440 km of fetch, and
/// `loom_water::ocean`'s shipping stack realises 6.100 m against it. A larger
/// sea is one the spectrum does not build, so it clamps.
pub const HS_FULL: f32 = 6.1;

/// The wind speed at which the hiss is brightest, in m/s.
///
/// **Derived from the same pairing as [`HS_FULL`]:** U10 = 18 m/s is the wind
/// that raises the 6.10 m sea, so it is the top of this scale too.
pub const WIND_FULL: f32 = 18.0;

/// The breaking fraction at which the hiss reaches full level.
///
/// **Sized from the water, not chosen.** `loom_water::spray::SPRAY_BREAK` is
/// 1.75σ of the fold distribution — a one-tailed 4% of a Gaussian surface — so
/// a steep sea presents a few percent and a generous worst case is a few times
/// that. 0.25 puts every sea the engine builds on the steep part of the
/// square-root curve below, while leaving the top of the legal `[0, 1]` range
/// in range rather than clipped off.
pub const BREAK_FULL: f32 = 0.25;

/// Corner frequency of the swell band, in Hz.
///
/// **Not derived, and one of the two constants a human's ear should confirm.**
/// What fixes its order of magnitude is [`crate::rain::measure`]'s tilt split
/// at 1 kHz: the rumble has to sit far below that or "brighter" stops meaning
/// anything. Measured, two poles here leave the band at **tilt 0.077** against
/// the hiss band's 5.15, which is the separation the brightness test rests on.
const SWELL_HZ: f32 = 90.0;

/// Corner of the hiss band on a windless sea, in Hz.
///
/// Above the 1 kHz tilt split by design — this band *is* the bright half.
const HISS_CALM_HZ: f32 = 1_200.0;

/// Corner of the hiss band at [`WIND_FULL`], in Hz.
///
/// **Wind is a tone control here, never a level.** It moves this corner and
/// nothing else, which is why a scene authoring `Wind.speed = 0` over a still
/// sea renders exact silence rather than something merely quiet. Whether wind
/// should also contribute its own airflow noise is a different sound and a
/// separate decision; this file claims only what the water told it.
const HISS_GALE_HZ: f32 = 3_500.0;

/// Level of the swell band at [`HS_FULL`], before [`HEADROOM`].
///
/// **Measured, not chosen.** Two poles at [`SWELL_HZ`] on the ±1 uniform noise
/// below leave a band measuring **rms 0.0242, crest factor 4.1** over thirty
/// seconds at 48 kHz — a low-pass throws away most of a broadband signal's
/// energy, so the band needs a large multiplier merely to be heard. This one
/// puts the loudest sea at the rms in the report.
const SWELL_GAIN: f32 = 11.0;

/// Level of the hiss band at [`BREAK_FULL`], before [`HEADROOM`].
///
/// **Measured, not chosen.** The high band is near-white and keeps its noise's
/// own crest factor: **rms 0.512, peak 1.34** — past the noise's own ±1,
/// because a high-pass taken by subtraction adds the filter state to the sample
/// when the two have opposite sign. Two orders of magnitude below
/// [`SWELL_GAIN`] is therefore a *comparable* level and not a quiet one:
/// measured at Hs 6.1 m with `breaking` 0.2, the hiss contributes **rms 0.050**
/// beside the swell's 0.147.
const HISS_GAIN: f32 = 0.25;

/// Master trim, so the loudest legal state stays inside `[-1, 1]`.
///
/// **Measured, not chosen**, the same way `rain.rs` sizes its own. Untrimmed,
/// Hs 6.1 m with `breaking` at the top of its legal range peaks at **1.34 over
/// thirty seconds**, and `it_does_not_clip` caught it. Headroom rather than a
/// clamp: clamping a transient is distortion, and distortion on a noise bed
/// sounds like a blown speaker rather than like a heavy sea.
///
/// **The bound is statistical and this is the honest way to say so.** Both
/// bands are filtered noise, whose amplitude distribution is Gaussian and has
/// no hard ceiling — a long enough render will always find a larger peak. What
/// is true is that the peak grows like the tail, and the growth is measured:
/// the worst legal state peaks untrimmed at **0.93 / 1.34 / 1.51 over 2 / 30 /
/// 300 seconds**, which is 3.5σ / 4.5σ / 5.1σ of its own rms. This trim leaves
/// the 300-second worst case at **0.83**, a further sigma of margin — and that
/// state is one the FFT sea never reaches anyway, since [`BREAK_FULL`] records
/// that a real breaking fraction is a few percent.
const HEADROOM: f32 = 0.55;

/// How deep the swell surge modulates the rumble, as a fraction.
///
/// **Not derived, and the constant most worth a human's ear.** A rumble at a
/// constant level is a truck idling; a sea breathes as the wave groups pass
/// under the listener. The envelope runs `[1 - depth, 1)`, so the surge only
/// ever *subtracts* and cannot push the peak past what [`SWELL_GAIN`] and
/// [`HEADROOM`] were measured for.
const SURGE_DEPTH: f32 = 0.45;

/// Seconds per swell group, roughly.
///
/// **Not derived.** A big sea's groups pass on the order of ten seconds and
/// this is a round number in that range. It is deliberately *not* re-derived
/// from a textbook peak-period relation: `loom_water::ocean` warns in as many
/// words that a hand formula disagreeing with `spectrum.rs` is a fact about the
/// formula, and this crate cannot ask `spectrum.rs` — it does not depend on
/// `loom_water`, and passing the state in is what keeps it that way.
const SURGE_SECONDS: f32 = 12.0;

/// One white-noise sample in `[-1, 1)` from a counter, a seed and a stream id.
///
/// **A stream id rather than one sequence sliced up**, the reason `rain.rs`
/// gives: the two bands need noise at the same instant, and drawing them from
/// one counter correlates them into a metallic comb.
fn white(cursor: u32, seed: u32, stream: u32) -> f32 {
    let h = loom_field::noise::hash(
        cursor
            .wrapping_add(stream.wrapping_mul(0x9E37_79B9))
            .wrapping_add(seed.wrapping_mul(0x85EB_CA6B)),
    );
    // Top 24 bits into an f32 mantissa gives `[1, 2)` exactly; the affine step
    // takes it to `[-1, 1)`. The same slice `loom_field::noise::lattice` takes,
    // for the same reason — it is exactly representable.
    f32::from_bits((h >> 8) | 0x3F80_0000).mul_add(2.0, -3.0)
}

/// A one-pole low-pass, as a coefficient for a given cutoff.
fn one_pole(cutoff_hz: f32, sample_rate: f32) -> f32 {
    let x = (-std::f32::consts::TAU * cutoff_hz / sample_rate.max(1.0)).exp();
    1.0 - x
}

/// The seed the engine's own sea uses, so what plays and what `loom audio`
/// renders are the same texture.
///
/// One constant rather than a per-scene number: a sea's *identity* is its
/// state, and two scenes at the same state should sound the same.
pub const SEED: u32 = 0x5EA;

/// Seconds for the level to follow a change in the sea.
///
/// The same figure `rain.rs` smooths its own level over, for the same reason:
/// a gain that jumps between one buffer and the next clicks. A sea state moves
/// far slower than this, so it costs nothing when nothing is changing.
const SMOOTHING_SECONDS: f32 = 0.35;

/// What the gains are heading toward. One per buffer, not one per sample.
#[derive(Debug, Clone, Copy)]
struct Targets {
    swell: f32,
    hiss: f32,
    /// Corner of the hiss band, in Hz. Smoothed rather than the coefficient,
    /// because the coefficient is exponential in it.
    cutoff: f32,
}

impl Targets {
    fn of(state: SeaState) -> Self {
        // Loudness rises fast and then flattens, the curve `rain.rs` puts on
        // rate for the same reason: the step from a 0.5 m ripple to a 1.5 m
        // chop is far more audible than the step from 4.5 m to 6.1 m.
        let wind = (state.wind.max(0.0) / WIND_FULL).min(1.0);
        Self {
            swell: (state.hs.max(0.0) / HS_FULL).min(1.0).sqrt() * SWELL_GAIN * HEADROOM,
            hiss: (state.breaking.max(0.0) / BREAK_FULL).min(1.0).sqrt() * HISS_GAIN * HEADROOM,
            cutoff: HISS_CALM_HZ + (HISS_GALE_HZ - HISS_CALM_HZ) * wind,
        }
    }
}

/// The sea as a **stream**: [`bed`]'s arithmetic with its filter states and its
/// sample cursor carried across buffers.
///
/// This exists because [`bed`] is a pure function and restarts everything it
/// owns on every call, so feeding it one audio buffer at a time restarts the
/// texture and clicks at every join. [`crate::rain::RainBed`] is the shape this
/// follows, and for the same reason.
///
/// **There is one implementation of the sound and it is here.** [`bed`] is a
/// one-shot render through this, so an offline `.wav` and what the speakers
/// play cannot drift apart.
#[derive(Debug, Clone)]
pub struct SeaBed {
    rate: f32,
    seed: u32,
    /// The surge's noise coordinate, offset per seed so two seeds do not
    /// breathe in lockstep. An integer under 2^10 is exact in an f32.
    surge_offset: f32,
    /// Monotonic sample counter. See the note in the loop about 2^24.
    cursor: u32,
    /// Two poles for the swell — 12 dB/octave, because one pole leaves enough
    /// top end that the "rumble" still hisses — and one for the hiss.
    swell_state: [f32; 2],
    hiss_state: f32,
    swell_k: f32,
    /// The smoothed gains, or `None` until the first sample.
    ///
    /// **`None` rather than zero, and that is what keeps [`bed`] a pure
    /// function.** A bed that started at silence and ramped would make a
    /// one-shot render of a heavy sea quieter than the state it was asked for,
    /// which is exactly the measurement every constant above was chosen
    /// against. So the first sample *snaps* to its target and only later
    /// changes are smoothed.
    smoothed: Option<Targets>,
}

impl SeaBed {
    #[must_use]
    pub fn new(sample_rate: u32, seed: u32) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let rate = sample_rate.max(1) as f32;
        #[allow(clippy::cast_precision_loss)]
        let surge_offset = (loom_field::noise::hash(seed) >> 22) as f32;
        Self {
            rate,
            seed,
            surge_offset,
            cursor: 0,
            swell_state: [0.0; 2],
            hiss_state: 0.0,
            swell_k: one_pole(SWELL_HZ, rate),
            smoothed: None,
        }
    }

    /// One sample, advancing every piece of state this owns.
    fn next(&mut self, target: Targets, smoothing: f32) -> f32 {
        let now = match self.smoothed {
            None => target,
            Some(previous) => Targets {
                swell: smoothing.mul_add(target.swell - previous.swell, previous.swell),
                hiss: smoothing.mul_add(target.hiss - previous.hiss, previous.hiss),
                cutoff: smoothing.mul_add(target.cutoff - previous.cutoff, previous.cutoff),
            },
        };
        self.smoothed = Some(now);

        let cursor = self.cursor;
        self.cursor = cursor.wrapping_add(1);
        // Past 2^24 samples — 349 s at 48 kHz — this quantises, because that
        // is where an f32 stops holding consecutive integers. Harmless here:
        // the surge below wants seconds of resolution, not samples, and the
        // render stays a pure function of its arguments either way.
        #[allow(clippy::cast_precision_loss)]
        let t = cursor as f32 / self.rate;
        // `[1 - SURGE_DEPTH, 1)`: the surge only ever subtracts, so the peak
        // stays where SWELL_GAIN and HEADROOM were measured.
        let surge = SURGE_DEPTH.mul_add(
            loom_field::noise::value([t / SURGE_SECONDS, self.surge_offset, 0.0]),
            1.0 - SURGE_DEPTH,
        );

        let n = white(cursor, self.seed, 1);
        self.swell_state[0] += (n - self.swell_state[0]) * self.swell_k;
        self.swell_state[1] += (self.swell_state[0] - self.swell_state[1]) * self.swell_k;

        // The hiss is what the low-pass did *not* keep, from its own stream —
        // a high-pass by subtraction, the shape `rain.rs` uses.
        let hiss_k = one_pole(now.cutoff, self.rate);
        let m = white(cursor, self.seed, 2);
        self.hiss_state += (m - self.hiss_state) * hiss_k;
        let high = m - self.hiss_state;

        // **Fixed order: swell, then hiss.** Float addition is not associative
        // and this output is compared byte for byte.
        self.swell_state[1].mul_add(now.swell * surge, high * now.hiss)
    }

    /// Add `out.len()` mono samples of sea into `out`.
    ///
    /// **Adds rather than overwrites**, the same contract
    /// [`crate::rain::RainBed::render`] has, so a bed sits under whatever has
    /// already been rendered.
    pub fn render_mono(&mut self, state: SeaState, out: &mut [f32]) {
        let target = Targets::of(state);
        let smoothing = one_pole(1.0 / SMOOTHING_SECONDS, self.rate);
        for slot in out.iter_mut() {
            *slot += self.next(target, smoothing);
        }
    }

    /// Add the sea into `out`, which is interleaved stereo.
    ///
    /// **The same sample in both ears, deliberately.** The mono argument on
    /// [`bed`] is a measurement argument — [`crate::rain::measure`]'s tilt
    /// reads two decorrelated channels as Nyquist content — and duplicating is
    /// the one widening that keeps it honest, because two identical channels
    /// interleave to a signal at the same tilt as either.
    ///
    /// `ponytail:` so the sea sits in the middle of the head rather than
    /// around it. Placing it is the shore work — a shoreline louder than open
    /// water — which the plan excludes on purpose; that is where a real
    /// decorrelation belongs, together with the answer to what it does to the
    /// measurement.
    pub fn render(&mut self, state: SeaState, out: &mut [f32]) {
        let target = Targets::of(state);
        let smoothing = one_pole(1.0 / SMOOTHING_SECONDS, self.rate);
        for frame in out.chunks_mut(2) {
            let value = self.next(target, smoothing);
            frame[0] += value;
            if frame.len() > 1 {
                frame[1] += value;
            }
        }
    }
}

/// `seconds` of sea, as **mono** samples at `sample_rate`.
///
/// # Mono, and the trap that decided it
///
/// [`crate::rain::measure`] says interleaving does not matter because it is
/// measuring energy. **That is true of rms and peak and false of tilt.** An
/// interleaved buffer whose two channels are decorrelated alternates between
/// two independent signals sample by sample, which is broadband content at
/// Nyquist that no channel contains. Measured on this bed: the same 90 Hz
/// rumble reads **tilt 0.922 interleaved with a second seed against 0.077
/// as one channel** — an order of magnitude, and enough to swamp the brightness
/// the whole feature exists to produce. Rain's *synthesised* path does not hit
/// it because its right ear is `value * 0.92 + patter * level * 0.08` — the
/// left signal scaled, plus a trickle — so the two are near-identical. Its
/// *recording* path does hit it, and `a_stereo_recording_stays_stereo` is the
/// test that says so.
///
/// So the bed is one channel, and widening it is the caller's business.
/// [`SeaBed::render`] duplicates it, which is the one widening that leaves the
/// measurement meaning what it meant.
///
/// **One call is one continuous stretch**, because it makes its own
/// [`SeaBed`]. Successive buffers want one of those held across them; that is
/// what the struct is for.
///
/// A still sea returns exact zeros — see `a_still_sea_is_exactly_silent`.
///
/// **Bit-identical to the pure-function form this replaced**, checked sample by
/// sample at Hs 0.6 / 2.5 / 6.1 m. The one exception is the *exactly still* sea,
/// where a sample that was `-0.0` is now `+0.0`: [`SeaBed::render_mono`] adds
/// into its buffer rather than assigning, and `0.0 + -0.0` is `+0.0`. Both
/// encode to the same PCM zero and both compare equal; nothing downstream can
/// tell them apart.
#[must_use]
pub fn bed(state: SeaState, seconds: f32, sample_rate: u32, seed: u32) -> Vec<f32> {
    #[allow(clippy::cast_precision_loss)]
    let rate = sample_rate.max(1) as f32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames = (rate * seconds.max(0.0)) as usize;
    let mut out = vec![0.0_f32; frames];
    SeaBed::new(sample_rate, seed).render_mono(state, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rain::measure;

    const RATE: u32 = 48_000;

    fn sea(hs: f32, breaking: f32) -> SeaState {
        SeaState { hs, breaking, wind: 12.0 }
    }

    /// Byte-identical, not approximately equal. `loom audio` renders offline
    /// and its output is compared; a bed that drifts is not renderable.
    #[test]
    fn it_is_deterministic() {
        let state = sea(3.0, 0.05);
        let a = bed(state, 0.25, RATE, 7);
        let b = bed(state, 0.25, RATE, 7);
        assert_eq!(a.len(), b.len(), "two renders of the same request differ in length");
        assert!(!a.is_empty(), "the bed rendered nothing to compare");
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            assert_eq!(x.to_bits(), y.to_bits(), "sample {i} differs between two renders");
        }
    }

    /// **The reason [`SeaBed`] exists.** Two successive buffers off one bed
    /// must be byte-identical to one render of both, or the streaming path is
    /// a different sound from the one every constant above was measured on —
    /// and the join is where a restarted filter clicks.
    #[test]
    fn a_stream_joins_without_a_seam() {
        let state = sea(3.0, 0.05);
        let whole = bed(state, 1.0, RATE, 5);
        let mut streamed = vec![0.0_f32; whole.len()];
        let mut streaming = SeaBed::new(RATE, 5);
        // Deliberately not a divisor of the length: a seam that only ever
        // lands on a round boundary is a seam this would not see.
        for chunk in streamed.chunks_mut(1_000) {
            streaming.render_mono(state, chunk);
        }
        for (i, (a, b)) in whole.iter().zip(&streamed).enumerate() {
            assert_eq!(a.to_bits(), b.to_bits(), "sample {i} differs across the buffer joins");
        }
    }

    /// A changed state must not click, and the level must actually arrive.
    ///
    /// Smoothing is what stops the click; a smoothing that never converged
    /// would be a bed permanently quieter than the sea it is reporting, which
    /// no other test here can see.
    #[test]
    fn a_changing_sea_slews_and_arrives() {
        let mut streaming = SeaBed::new(RATE, 5);
        let mut quiet = vec![0.0_f32; RATE as usize];
        streaming.render_mono(sea(0.2, 0.0), &mut quiet);

        // One buffer of the new state, then a second: the first holds the
        // slew, the second is the state itself.
        let mut slew = vec![0.0_f32; RATE as usize / 2];
        streaming.render_mono(sea(6.1, 0.2), &mut slew);
        let mut settled = vec![0.0_f32; RATE as usize];
        streaming.render_mono(sea(6.1, 0.2), &mut settled);

        let step = slew.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0_f32, f32::max);
        let inside = quiet
            .windows(2)
            .chain(settled.windows(2))
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            step <= inside,
            "the level change jumped harder than the sea itself moves: {step} against {inside}"
        );

        let reference = measure(&bed(sea(6.1, 0.2), 1.0, RATE, 5), RATE);
        let arrived = measure(&settled, RATE);
        assert!(
            (arrived.rms - reference.rms).abs() < reference.rms * 0.05,
            "the smoothed level never reached the sea it was given: rms {} against {}",
            arrived.rms,
            reference.rms
        );
    }

    /// **The mix, not the bed** — [`HEADROOM`] is a statistical bound on the
    /// sea *alone*, and the mixer sums the sea with the weather.
    ///
    /// Measured at the worst legal pair — the top rung of [`HS_FULL`] with a
    /// `breaking` fraction well past anything the FFT sea reaches, under rain
    /// at and beyond `rain::FULL_RATE`, thirty seconds at 48 kHz:
    ///
    ///     rain rms 0.1898 peak 0.7802   sea rms 0.1622 peak 0.6883
    ///     mix  rms 0.2496 peak 1.2905   samples past full scale: 105 of 2,880,000
    ///
    /// **So the sum does briefly exceed full scale, and this test says so
    /// rather than hiding it.** 0.0036% of samples, isolated and single —
    /// two independent Gaussian beds, each inside its own trim, whose tails
    /// occasionally land together. There is deliberately no master limiter:
    /// one would change what every existing rain scene renders, and a scene
    /// with no water sounding byte for byte as it did is the acceptance test
    /// this feature was built against. What is asserted instead is that the
    /// overshoot stays vanishingly rare, which fails the moment either bed's
    /// level grows.
    ///
    /// Rain saturates at `rain::FULL_RATE`, so 12 and 40 mm/h measure
    /// identically; the loop runs both to keep that visible.
    #[test]
    fn the_sea_and_the_rain_together_barely_touch_full_scale() {
        let seconds = 30.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = (RATE as f32 * seconds) as usize;
        for intensity in [12.0, 40.0] {
            let mut out = vec![0.0_f32; frames * 2];
            let mut rain = crate::rain::RainBed::new(RATE);
            rain.render(
                crate::rain::RainAudio { intensity, openness: 1.0, volume: 1.0 },
                &mut out,
            );
            SeaBed::new(RATE, SEED).render(
                SeaState { hs: HS_FULL, breaking: 0.2, wind: WIND_FULL },
                &mut out,
            );
            let m = measure(&out, RATE);
            #[allow(clippy::cast_precision_loss)]
            let over = out.iter().filter(|s| s.abs() > 1.0).count() as f64 / out.len() as f64;
            assert!(
                over < 0.0001,
                "{intensity} mm/h over the heaviest sea puts {:.4}% of samples past full scale",
                over * 100.0
            );
            assert!(
                m.peak < 1.5,
                "{intensity} mm/h over the heaviest sea peaks at {}, which is a level error \
                 rather than a coincident tail",
                m.peak
            );
        }
    }

    /// The seed has to reach the samples, or `it_is_deterministic` is passing
    /// on a bed that ignores three of its four arguments.
    #[test]
    fn the_seed_changes_the_texture() {
        let state = sea(3.0, 0.05);
        assert_ne!(bed(state, 0.25, RATE, 7), bed(state, 0.25, RATE, 8));
    }

    /// A calm sea is quieter than a gale.
    #[test]
    fn a_bigger_sea_is_louder() {
        let mut last = 0.0;
        for hs in [0.5, 1.5, 3.0, 4.5, 6.1] {
            let m = measure(&bed(sea(hs, 0.0), 1.0, RATE, 1), RATE);
            assert!(m.rms > last, "Hs {hs} m is not louder than the step below: {} vs {last}", m.rms);
            last = m.rms;
        }
    }

    /// **The load-bearing test.** Hold Hs fixed and raise the breaking
    /// fraction: the sound must get *brighter*, not merely louder. Without
    /// this, `breaking` is a volume knob and the sea sounds like one thing at
    /// every state.
    #[test]
    fn a_breaking_sea_is_brighter() {
        let calm = measure(&bed(sea(3.0, 0.0), 1.0, RATE, 1), RATE);
        let breaking = measure(&bed(sea(3.0, 0.2), 1.0, RATE, 1), RATE);
        assert!(
            breaking.tilt > calm.tilt,
            "breaking crests did not brighten the sea: tilt {} against {} unbroken",
            breaking.tilt,
            calm.tilt
        );
    }

    /// `pool.loom` authors `Wind.speed = 0` and must not whisper.
    #[test]
    fn a_still_sea_is_exactly_silent() {
        let samples = bed(SeaState { hs: 0.0, breaking: 0.0, wind: 0.0 }, 0.5, RATE, 3);
        assert!(!samples.is_empty(), "a still sea rendered no samples at all");
        for (i, s) in samples.iter().enumerate() {
            assert_eq!(*s, 0.0, "sample {i} of a still sea is {s}, not silence");
        }
    }

    /// Peak inside ±1.0 across the legal range, including the top rung and a
    /// `breaking` fraction well past anything the FFT sea reaches.
    ///
    /// **Thirty seconds, not two.** The peak of a noise band grows with the
    /// length of the render because the distribution is Gaussian, and a
    /// two-second render of the worst state measured 0.93 untrimmed while
    /// thirty seconds of the same state measured 1.34. A short render is not
    /// evidence of headroom.
    #[test]
    fn it_does_not_clip() {
        for hs in [0.0, 1.0, 3.0, 6.1] {
            for breaking in [0.0, 0.05, 0.2, 1.0] {
                for wind in [0.0, 12.0, 25.0] {
                    let m = measure(&bed(SeaState { hs, breaking, wind }, 30.0, RATE, 11), RATE);
                    assert!(
                        m.peak <= 1.0,
                        "Hs {hs} / breaking {breaking} / wind {wind} clipped at {}",
                        m.peak
                    );
                }
            }
        }
    }
}

