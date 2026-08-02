//! Turning positioned sounds into samples.
//!
//! **This is where the acoustics stop being a number.** `Acoustics` measures
//! what a place does to a sound — how much is blocked, how much material it
//! crossed, how much comes back and how open the space is. Until something
//! renders samples, all of that is a report nobody hears. Each of those four
//! numbers drives one thing here:
//!
//! - `occlusion` cuts the gain — less of it arrives.
//! - `muffling` closes a low-pass — what arrives has lost its top end, which
//!   is what makes a wall sound like a wall rather than a volume knob.
//! - `reverb_gain` and `reverb_delay` feed a delay line — the room answering.
//! - `openness` scales that send down, because a field has nothing to answer.
//!
//! Deterministic: the same voices, the same acoustics and the same buffer
//! length produce the same samples. That is what makes any of it testable
//! without ears, and the reason none of the DSP here reads a clock.

use crate::Acoustics;

/// One sound playing somewhere in the world.
pub struct Voice {
    /// Mono source samples.
    pub samples: std::sync::Arc<Vec<f32>>,
    /// How far through it playback has got, in source samples.
    pub cursor: f64,
    /// Source samples per output sample. Resampling is a straight rate ratio;
    /// pitch shifts are the same knob.
    pub step: f64,
    pub looping: bool,
    /// Authored loudness, before anything the world does to it.
    pub volume: f32,
    /// Where it is, relative to the listener: right, up, forward.
    pub relative: [f32; 3],
    /// What the world does to it.
    pub acoustics: Acoustics,
    /// Distance at which it has faded to nothing.
    pub range: f32,
    /// Low-pass state, carried between buffers. A filter reset every buffer
    /// clicks at every boundary.
    lowpass: [f32; 2],
}

impl Voice {
    #[must_use]
    pub fn new(
        samples: std::sync::Arc<Vec<f32>>,
        source_rate: u32,
        output_rate: u32,
        volume: f32,
        range: f32,
    ) -> Self {
        Self {
            samples,
            cursor: 0.0,
            step: f64::from(source_rate.max(1)) / f64::from(output_rate.max(1)),
            looping: false,
            volume,
            relative: [0.0, 0.0, -1.0],
            acoustics: Acoustics {
                occlusion: 0.0,
                muffling: 0.0,
                reverb_delay: 0.0,
                reverb_gain: 0.0,
                openness: 1.0,
                distance: 0.0,
            },
            range,
            lowpass: [0.0; 2],
        }
    }

    #[must_use]
    pub fn finished(&self) -> bool {
        #[allow(clippy::cast_precision_loss)]
        {
            !self.looping && self.cursor >= self.samples.len() as f64
        }
    }

    /// How loud this voice is, per ear, after everything the world does.
    fn gains(&self) -> (f32, f32) {
        // Linear falloff to the range, not inverse-square. Inverse-square is
        // correct and unusable: it is deafening at a metre and inaudible at
        // ten, and every game applies a curve a designer can reason about.
        let reach = if self.range > 0.0 {
            (1.0 - self.acoustics.distance / self.range).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let through = 1.0 - self.acoustics.occlusion;
        let level = self.volume * reach * through;

        // Constant-power panning. A linear pan dips in the middle, which is
        // audible as a sound getting quieter while it passes in front of you.
        let side = normalized(self.relative)[0].clamp(-1.0, 1.0);
        let angle = (side + 1.0) * std::f32::consts::FRAC_PI_4;
        (level * angle.cos(), level * angle.sin())
    }
}

/// Sums voices into an interleaved stereo buffer.
pub struct Mixer {
    pub sample_rate: u32,
    /// A single delay line shared by every voice, which is what a room is:
    /// one space, not one per sound.
    reverb: Vec<f32>,
    reverb_at: usize,
}

impl Mixer {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        // Two seconds is longer than any room this measures.
        let length = (sample_rate.max(1) as usize * 2).max(1);
        Self {
            sample_rate: sample_rate.max(1),
            reverb: vec![0.0; length],
            reverb_at: 0,
        }
    }

    /// Render `out` (interleaved stereo) from `voices`, replacing its contents.
    ///
    /// Finished voices are left in place rather than removed — the caller owns
    /// the list and knows what a finished voice means to it.
    pub fn render(&mut self, voices: &mut [Voice], out: &mut [f32]) {
        out.fill(0.0);

        for voice in voices.iter_mut() {
            if voice.finished() || voice.samples.is_empty() {
                continue;
            }
            let (left_gain, right_gain) = voice.gains();
            // A low-pass that closes as material thickens. `muffling` of zero
            // leaves the signal alone; one takes almost everything above a
            // few hundred hertz, which is what a closed door does.
            let cutoff = 1.0 - voice.acoustics.muffling.clamp(0.0, 1.0) * 0.97;
            let send = voice.acoustics.reverb_gain * (1.0 - voice.acoustics.openness);
            let delay = self.delay_samples(voice.acoustics.reverb_delay);

            for frame in out.chunks_exact_mut(2) {
                let Some(raw) = voice.sample() else { break };

                // Two poles, so the slope is steep enough to sound like a wall
                // rather than a blanket.
                voice.lowpass[0] += (raw - voice.lowpass[0]) * cutoff;
                voice.lowpass[1] += (voice.lowpass[0] - voice.lowpass[1]) * cutoff;
                let shaped = voice.lowpass[1];

                frame[0] += shaped * left_gain;
                frame[1] += shaped * right_gain;

                if send > 1e-4 {
                    let at = (self.reverb_at + delay) % self.reverb.len();
                    self.reverb[at] += shaped * send;
                }
                self.reverb_at = (self.reverb_at + 1) % self.reverb.len();
            }
        }

        self.tail(out);
    }

    /// Add what the room is still returning, and decay it.
    fn tail(&mut self, out: &mut [f32]) {
        // The write cursor advanced once per voice above, so rewind it to the
        // start of this buffer before reading the tail out.
        let frames = out.len() / 2;
        let mut at = (self.reverb_at + self.reverb.len() - frames % self.reverb.len())
            % self.reverb.len();

        for frame in out.chunks_exact_mut(2) {
            let wet = self.reverb[at];
            // Fed to both ears equally: a reflection arrives from everywhere,
            // which is what distinguishes it from the direct sound.
            frame[0] += wet * 0.5;
            frame[1] += wet * 0.5;
            // Decayed rather than cleared, so a room rings out instead of
            // stopping dead when the source does.
            self.reverb[at] *= 0.35;
            at = (at + 1) % self.reverb.len();
        }
        self.reverb_at = at;
    }

    fn delay_samples(&self, seconds: f32) -> usize {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let samples = (seconds.max(0.0) * self.sample_rate as f32) as usize;
        samples.min(self.reverb.len() - 1)
    }
}

impl Voice {
    /// The next source sample, linearly interpolated, advancing the cursor.
    fn sample(&mut self) -> Option<f32> {
        #[allow(clippy::cast_precision_loss)]
        let length = self.samples.len() as f64;
        if self.cursor >= length {
            if !self.looping {
                return None;
            }
            self.cursor %= length;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let index = self.cursor as usize;
        let fraction = self.cursor - self.cursor.floor();
        let a = self.samples[index];
        // Interpolated, not nearest: resampling by picking the closest sample
        // is audible as a rasp on anything that is not at the output rate.
        let b = if index + 1 < self.samples.len() {
            self.samples[index + 1]
        } else if self.looping {
            self.samples[0]
        } else {
            a
        };
        #[allow(clippy::cast_possible_truncation)]
        let out = a + (b - a) * fraction as f32;
        self.cursor += self.step;
        Some(out)
    }
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length < 1e-6 {
        return [0.0, 0.0, -1.0];
    }
    [v[0] / length, v[1] / length, v[2] / length]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(length: usize) -> std::sync::Arc<Vec<f32>> {
        // Alternating, so a low-pass has something to remove.
        std::sync::Arc::new((0..length).map(|i| if i % 2 == 0 { 1.0 } else { -1.0 }).collect())
    }

    fn voice(distance: f32) -> Voice {
        let mut v = Voice::new(tone(4096), 48_000, 48_000, 1.0, 20.0);
        v.relative = [0.0, 0.0, -distance];
        v.acoustics.distance = distance;
        v
    }

    fn peak(out: &[f32]) -> f32 {
        out.iter().fold(0.0_f32, |a, b| a.max(b.abs()))
    }

    fn render(mut voices: Vec<Voice>) -> Vec<f32> {
        let mut mixer = Mixer::new(48_000);
        let mut out = vec![0.0; 512];
        mixer.render(&mut voices, &mut out);
        out
    }

    #[test]
    fn a_distant_sound_is_quieter_than_a_near_one() {
        let near = peak(&render(vec![voice(1.0)]));
        let far = peak(&render(vec![voice(15.0)]));

        assert!(near > far * 2.0, "near {near} vs far {far}");
        assert!(far > 0.0, "still audible inside its range");
    }

    #[test]
    fn a_sound_past_its_range_is_silent() {
        assert!(peak(&render(vec![voice(50.0)])) < 1e-6);
    }

    /// **Occlusion is a volume knob and muffling is not.** A wall does not
    /// just make a sound quieter, it takes the top off — and that is what
    /// makes it read as a wall rather than as distance.
    #[test]
    fn muffling_removes_more_than_it_quietens() {
        let plain = render(vec![voice(2.0)]);
        let muffled = render(vec![{
            let mut v = voice(2.0);
            v.acoustics.muffling = 1.0;
            v
        }]);

        // The alternating source is the highest frequency representable, so a
        // closed low-pass should nearly erase it.
        assert!(
            peak(&muffled) < peak(&plain) * 0.2,
            "muffled {} vs plain {}",
            peak(&muffled),
            peak(&plain)
        );
    }

    #[test]
    fn occlusion_quietens_a_sound() {
        let clear = peak(&render(vec![voice(2.0)]));
        let blocked = peak(&render(vec![{
            let mut v = voice(2.0);
            v.acoustics.occlusion = 0.75;
            v
        }]));

        assert!(blocked < clear * 0.5, "clear {clear} vs blocked {blocked}");
    }

    /// Panning has to actually place a sound, or a first-person view gives no
    /// clue where anything is.
    #[test]
    fn a_sound_on_the_right_is_louder_in_the_right_ear() {
        let mut v = voice(2.0);
        v.relative = [2.0, 0.0, 0.0];
        let out = render(vec![v]);

        let left = out.iter().step_by(2).fold(0.0_f32, |a, b| a.max(b.abs()));
        let right = out.iter().skip(1).step_by(2).fold(0.0_f32, |a, b| a.max(b.abs()));
        assert!(right > left * 2.0, "left {left} right {right}");
    }

    /// A room answers; a field does not. `openness` is what tells them apart,
    /// and without it every outdoor sound would drag a reverb tail around.
    #[test]
    fn a_room_adds_a_tail_and_open_ground_does_not() {
        let indoors = {
            let mut v = voice(3.0);
            v.acoustics.reverb_gain = 0.9;
            v.acoustics.reverb_delay = 0.02;
            v.acoustics.openness = 0.0;
            v
        };
        // **The same reverb gain**, differing only in openness. Letting the
        // gain differ too would let this pass with openness ignored entirely,
        // which is precisely the mistake it exists to catch: a field with
        // reflective ground still reports a gain, and it is `openness` that
        // says the sound went up into the sky instead of coming back.
        let outdoors = {
            let mut v = voice(3.0);
            v.acoustics.reverb_gain = 0.9;
            v.acoustics.reverb_delay = 0.02;
            v.acoustics.openness = 1.0;
            v
        };

        // Render, then render silence: what comes out of the second buffer is
        // the room still ringing.
        let tail_of = |v: Voice| {
            let mut mixer = Mixer::new(48_000);
            let mut voices = vec![v];
            let mut out = vec![0.0; 4096];
            mixer.render(&mut voices, &mut out);
            let mut after = vec![0.0; 4096];
            mixer.render(&mut [], &mut after);
            peak(&after)
        };

        assert!(
            tail_of(indoors) > tail_of(outdoors) + 0.01,
            "a field rang like a room"
        );
    }

    #[test]
    fn a_finished_voice_stops_and_a_looping_one_does_not() {
        let mut once = Voice::new(tone(64), 48_000, 48_000, 1.0, 20.0);
        let mut forever = Voice::new(tone(64), 48_000, 48_000, 1.0, 20.0);
        forever.looping = true;

        let mut mixer = Mixer::new(48_000);
        let mut out = vec![0.0; 512];
        mixer.render(std::slice::from_mut(&mut once), &mut out);
        mixer.render(std::slice::from_mut(&mut forever), &mut out);

        assert!(once.finished(), "a 64-sample clip is done in 256 frames");
        assert!(!forever.finished());
    }

    /// Resampling by rate ratio, so a clip recorded at another rate plays at
    /// the right pitch and length rather than fast and high.
    #[test]
    fn a_clip_at_half_the_output_rate_plays_at_half_speed() {
        let mut v = Voice::new(tone(100), 24_000, 48_000, 1.0, 20.0);
        let mut mixer = Mixer::new(48_000);
        let mut out = vec![0.0; 2 * 100];
        mixer.render(std::slice::from_mut(&mut v), &mut out);

        // A hundred output frames at half step consumes fifty source samples.
        assert!((v.cursor - 50.0).abs() < 1.0, "cursor at {}", v.cursor);
        assert!(!v.finished());
    }

    /// **What makes any of this testable.** The same voices and the same
    /// buffer produce the same samples — no clock, no randomness.
    #[test]
    fn the_same_mix_renders_identically() {
        let a = render(vec![voice(4.0)]);
        let b = render(vec![voice(4.0)]);

        assert_eq!(a, b);
        assert!(peak(&a) > 0.0);
    }

    /// Silence in, silence out — and no NaN from an empty voice list, which
    /// is what a scene with no sounds in it renders every buffer.
    #[test]
    fn nothing_playing_is_silence_rather_than_noise() {
        let out = render(Vec::new());

        assert!(out.iter().all(|s| s.abs() < 1e-9 && s.is_finite()), "{out:?}");
    }
}
