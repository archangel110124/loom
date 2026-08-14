//! The speakers.
//!
//! **Deliberately thin, because it is the one part here that cannot be
//! tested.** Everything upstream — decoding, attenuation, panning, filtering,
//! the room — is arithmetic on buffers and is checked without a sound card.
//! This is the piece that hands those buffers to the operating system, and no
//! assertion I can write proves it made a noise. So it does as little as
//! possible: open a stream, copy from a queue, report what it opened.
//!
//! **Audio is the one place the wall clock is allowed.** The device pulls
//! buffers on its own thread when it needs them, at a rate nothing here
//! controls, and that is exactly the variable timing never-do #8 keeps out of
//! the simulation. The line is the queue below: the simulation writes voices
//! into it on fixed ticks, and the callback drains it whenever the hardware
//! asks. Nothing on the far side of that queue can affect the tick.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::{Mixer, Voice};

/// What could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioError {
    pub error: &'static str,
    pub detail: String,
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error, self.detail)
    }
}

/// Voices the device is playing, shared with its callback thread.
///
/// A mutex rather than a lock-free ring. `ponytail:` the callback holds it for
/// the length of one mix — tens of microseconds — and the simulation touches
/// it once a tick. A lock-free queue is the right answer when that contention
/// shows up in a profile, and it is a great deal more code to get right.
#[derive(Default)]
pub struct Playing {
    pub voices: Vec<Voice>,
    /// The weather, which is not a voice. See [`crate::rain::RainBed`] for why
    /// an ambient bed cannot be one: a voice has a position, a distance and a
    /// range, and rain has none of the three.
    pub rain: crate::rain::RainAudio,
}

/// An open output stream.
pub struct Audio {
    /// Dropping this closes the device, so it is kept even though nothing
    /// reads it.
    _stream: cpal::Stream,
    playing: Arc<Mutex<Playing>>,
    sample_rate: u32,
    name: String,
}

impl Audio {
    /// Open the default output device.
    ///
    /// # Errors
    /// [`AudioError`] when there is no device, no supported configuration, or
    /// the stream will not start. A machine with no sound card is an ordinary
    /// thing to meet — headless CI is one — so the caller is expected to carry
    /// on without sound rather than refuse to run.
    pub fn open() -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(AudioError {
            error: "no_output_device",
            detail: "the host reported no default output".to_owned(),
        })?;
        let config = device.default_output_config().map_err(|e| AudioError {
            error: "no_output_config",
            detail: e.to_string(),
        })?;

        // `SampleRate` is a plain `u32` in cpal 0.18, and the stream config
        // is taken by value — both differ from older releases, and both were
        // wrong when written from memory rather than read.
        let sample_rate = config.sample_rate();
        let channels = config.channels() as usize;
        let name = format!("{sample_rate} Hz, {channels} channels");
        let playing = Arc::new(Mutex::new(Playing::default()));
        let mixer = Mutex::new(Mixer::new(sample_rate));

        let shared = Arc::clone(&playing);
        let mut stereo = Vec::new();
        // Owned by the callback rather than shared: it is pure generator state
        // — filters and a sample counter — and nothing outside the audio thread
        // has any business touching it. What the game changes is the
        // *parameters*, which go through `Playing`.
        let mut bed = crate::rain::RainBed::new(sample_rate);
        let stream = device
            .build_output_stream(
                config.config(),
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let frames = out.len() / channels.max(1);
                    stereo.resize(frames * 2, 0.0);

                    // A poisoned lock means a panic somewhere else. Silence is
                    // the right answer to that, not a second panic on the
                    // audio thread, which would take the process down.
                    match (shared.lock(), mixer.lock()) {
                        (Ok(mut playing), Ok(mut mixer)) => {
                            mixer.render(&mut playing.voices, &mut stereo);
                            playing.voices.retain(|v| !v.finished());
                            // **After the voices, into the same buffer**, which
                            // is why `RainBed::render` adds rather than writes.
                            // The bed lives on the audio thread and its cursor
                            // never resets, so the texture has no loop point —
                            // it is generated, not played back.
                            bed.render(playing.rain, &mut stereo);
                        }
                        _ => stereo.fill(0.0),
                    }

                    // Fanned out to however many channels the device wants.
                    // Asking for stereo and being given 7.1 is common, and a
                    // straight copy would play everything out of two speakers
                    // or garble the interleaving.
                    for (frame, chunk) in out.chunks_mut(channels).enumerate() {
                        let (left, right) = (stereo[frame * 2], stereo[frame * 2 + 1]);
                        for (index, sample) in chunk.iter_mut().enumerate() {
                            *sample = if index % 2 == 0 { left } else { right };
                        }
                    }
                },
                |e| eprintln!("loom: audio stream error: {e}"),
                None,
            )
            .map_err(|e| AudioError {
                error: "stream_failed",
                detail: e.to_string(),
            })?;

        stream.play().map_err(|e| AudioError {
            error: "stream_would_not_start",
            detail: e.to_string(),
        })?;

        Ok(Self {
            _stream: stream,
            playing,
            sample_rate,
            name,
        })
    }

    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.name
    }

    /// How many voices are sounding.
    #[must_use]
    pub fn voice_count(&self) -> usize {
        self.playing.lock().map_or(0, |p| p.voices.len())
    }

    /// Start a voice.
    pub fn play(&self, voice: Voice) {
        if let Ok(mut playing) = self.playing.lock() {
            // A cap, because a script that emits a sound every tick would
            // otherwise grow this without limit and take the callback's
            // budget with it. Oldest first: the newest sound is the one the
            // player just caused and the one they are listening for.
            const MAX_VOICES: usize = 64;
            if playing.voices.len() >= MAX_VOICES {
                playing.voices.remove(0);
            }
            playing.voices.push(voice);
        }
    }

    /// Update every voice's position and acoustics for this tick.
    ///
    /// Takes a closure rather than a list so the caller can match voices to
    /// whatever it is tracking without this crate knowing about scenes.
    pub fn update(&self, mut per_voice: impl FnMut(usize, &mut Voice)) {
        if let Ok(mut playing) = self.playing.lock() {
            for (index, voice) in playing.voices.iter_mut().enumerate() {
                per_voice(index, voice);
            }
        }
    }

    /// Set the weather the ambient bed renders.
    ///
    /// Cheap enough to call every tick: it moves three floats under the lock the
    /// voices already use. The bed smooths them itself, so a rate that jumps
    /// between ticks is still a fade.
    pub fn set_rain(&self, rain: crate::rain::RainAudio) {
        if let Ok(mut playing) = self.playing.lock() {
            playing.rain = rain;
        }
    }

    /// Stop everything, for Stop in the editor.
    pub fn silence(&self) {
        if let Ok(mut playing) = self.playing.lock() {
            playing.voices.clear();
            // **The weather too.** Stop means silence, and a bed that kept
            // raining through it would be the one sound the editor could not
            // stop.
            playing.rain = crate::rain::RainAudio::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A machine with no sound card must still run.** Headless CI is one,
    /// and a scene that refused to start without speakers would fail there
    /// for a reason that has nothing to do with the scene.
    ///
    /// So this asserts the shape of both answers rather than that a device
    /// exists: either it opened and reports a sane rate, or it declined with
    /// a named error. What it deliberately does not assert is that a sound
    /// came out — nothing here can hear.
    #[test]
    fn opening_the_device_either_works_or_declines_cleanly() {
        match Audio::open() {
            Ok(audio) => {
                assert!(audio.sample_rate() >= 8_000, "{}", audio.sample_rate());
                assert_eq!(audio.voice_count(), 0, "nothing plays until asked");
                audio.silence();
                eprintln!("AUDIO_DEVICE_OPENED {}", audio.device_name());
            }
            Err(e) => {
                assert!(!e.error.is_empty() && !e.detail.is_empty(), "{e}");
                eprintln!("AUDIO_DEVICE_ABSENT {e}");
            }
        }
    }
}
