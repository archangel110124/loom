//! Loading a sound.
//!
//! WAV only, decoded by hand. `ponytail:` a decoder crate would bring Ogg and
//! MP3 with it, and everything this engine has needed so far is a short PCM
//! effect — a gunshot, a footstep, an explosion. Sixteen-bit PCM is eighty
//! lines and no dependency. When something needs a five-minute music track,
//! `Clip::load` is the seam and `symphonia` goes behind it.
//!
//! Mono internally. A positioned sound has to be panned and attenuated per
//! ear, and a stereo source has already decided what each ear hears — so a
//! stereo file is folded down rather than being placed in the world twice.

/// Decoded audio, ready to play.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    /// Mono samples, `-1.0` to `1.0`.
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

/// Why a file could not be read as sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipError {
    pub error: &'static str,
    pub detail: String,
}

impl std::fmt::Display for ClipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error, self.detail)
    }
}

impl Clip {
    #[must_use]
    pub fn seconds(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        {
            self.samples.len() as f32 / self.sample_rate as f32
        }
    }

    /// Read a WAV file.
    ///
    /// # Errors
    /// [`ClipError`] naming what was wrong with it. A sound that will not load
    /// is a warning to the caller, not a reason to refuse to run — the same
    /// reasoning as a missing texture.
    pub fn load(path: &std::path::Path) -> Result<Self, ClipError> {
        let bytes = std::fs::read(path).map_err(|e| ClipError {
            error: "io_error",
            detail: e.to_string(),
        })?;
        Self::decode(&bytes)
    }

    /// Decode WAV bytes.
    ///
    /// # Errors
    /// [`ClipError`] if the header is not RIFF/WAVE, the format is not 16-bit
    /// PCM, or a chunk runs off the end of the data.
    pub fn decode(bytes: &[u8]) -> Result<Self, ClipError> {
        let (samples, sample_rate, _) = Self::parse(bytes, true)?;
        Ok(Self { samples, sample_rate })
    }

    /// Decode WAV bytes **without folding to mono**, as interleaved frames.
    ///
    /// Returns the samples, the rate, and the channel count.
    ///
    /// **For ambient beds, which are the one thing folding is wrong for.** A
    /// positioned sound is panned per ear from its position, so a stereo file
    /// has already made that decision and averaging it is right. Rain is not
    /// positioned: it surrounds the listener, and its stereo image *is* the
    /// effect. Folded to mono it collapses to a point between your eyes.
    ///
    /// # Errors
    /// As [`Clip::decode`].
    pub fn decode_interleaved(bytes: &[u8]) -> Result<(Vec<f32>, u32, u16), ClipError> {
        Self::parse(bytes, false)
    }

    fn parse(bytes: &[u8], fold: bool) -> Result<(Vec<f32>, u32, u16), ClipError> {
        let bad = |detail: &str| ClipError {
            error: "not_a_wav",
            detail: detail.to_owned(),
        };
        if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err(bad("not RIFF/WAVE"));
        }

        let mut at = 12;
        let mut channels = 0_u16;
        let mut sample_rate = 0_u32;
        let mut bits = 0_u16;
        let mut samples = Vec::new();

        // Chunks in order, skipping any that are not `fmt ` or `data`. Files
        // routinely carry LIST and cue chunks between them, and a reader that
        // assumes `data` comes second rejects perfectly ordinary files.
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let size = u32::from_le_bytes([
                bytes[at + 4],
                bytes[at + 5],
                bytes[at + 6],
                bytes[at + 7],
            ]) as usize;
            let body = at + 8;
            let end = body.checked_add(size).ok_or_else(|| bad("chunk overflows"))?;
            if end > bytes.len() {
                return Err(bad("chunk runs past the end of the file"));
            }

            if id == b"fmt " {
                if size < 16 {
                    return Err(bad("fmt chunk too short"));
                }
                let format = u16::from_le_bytes([bytes[body], bytes[body + 1]]);
                channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]);
                sample_rate = u32::from_le_bytes([
                    bytes[body + 4],
                    bytes[body + 5],
                    bytes[body + 6],
                    bytes[body + 7],
                ]);
                bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]);
                // 1 is PCM. 0xFFFE is WAVE_FORMAT_EXTENSIBLE, which for our
                // purposes is PCM wearing a longer header.
                if format != 1 && format != 0xFFFE {
                    return Err(bad("only PCM is supported"));
                }
            } else if id == b"data" {
                if bits != 16 {
                    return Err(bad("only 16-bit samples are supported"));
                }
                if channels == 0 {
                    return Err(bad("data chunk before fmt chunk"));
                }
                let frames = size / 2 / channels as usize;
                samples.reserve(frames);
                for frame in 0..frames {
                    // Folded to mono by averaging when the caller wants a
                    // positioned sound: it is panned and attenuated per ear
                    // here, and a stereo file has already decided what each ear
                    // hears. `decode_interleaved` keeps the channels.
                    let mut total = 0.0_f32;
                    for channel in 0..channels as usize {
                        let index = body + (frame * channels as usize + channel) * 2;
                        let raw = i16::from_le_bytes([bytes[index], bytes[index + 1]]);
                        let value = f32::from(raw) / 32768.0;
                        if fold {
                            total += value;
                        } else {
                            samples.push(value);
                        }
                    }
                    if fold {
                        samples.push(total / f32::from(channels));
                    }
                }
            }

            // Chunks are word-aligned; an odd size is followed by a pad byte.
            at = end + (size & 1);
        }

        if sample_rate == 0 {
            return Err(bad("no fmt chunk"));
        }
        Ok((samples, sample_rate, channels.max(1)))
    }

    /// Encode interleaved samples as a 16-bit PCM WAV.
    ///
    /// **The other half of [`Clip::decode`], and it exists so the offline audio
    /// render can be listened to.** A gate that only prints numbers is a gate
    /// nobody trusts; being able to open the file is what lets a human check
    /// that "louder and darker" actually sounds like rain under a roof.
    ///
    /// Takes `channels` because the render is stereo while a `Clip` is mono —
    /// this writes what it is given rather than assuming.
    #[must_use]
    pub fn encode(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
        let channels = channels.max(1);
        let bytes_per_sample = 2_u32;
        let block_align = u32::from(channels) * bytes_per_sample;
        #[allow(clippy::cast_possible_truncation)]
        let data_len = (samples.len() * bytes_per_sample as usize) as u32;

        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16_u32.to_le_bytes());
        out.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&(sample_rate * block_align).to_le_bytes());
        #[allow(clippy::cast_possible_truncation)]
        out.extend_from_slice(&(block_align as u16).to_le_bytes());
        out.extend_from_slice(&16_u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for &s in samples {
            // Clamped before scaling: a sample past 1.0 would wrap to the
            // opposite rail as an integer, which is a loud click rather than
            // the quiet distortion clipping sounds like.
            #[allow(clippy::cast_possible_truncation)]
            let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV file, built the way a real one is laid out.
    fn wav(channels: u16, rate: u32, frames: &[i16], extra_chunk: bool) -> Vec<u8> {
        let mut data = Vec::new();
        for sample in frames {
            data.extend_from_slice(&sample.to_le_bytes());
        }

        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        fmt.extend_from_slice(&(channels * 2).to_le_bytes());
        fmt.extend_from_slice(&16_u16.to_le_bytes()); // bits

        let mut body = Vec::new();
        body.extend_from_slice(b"WAVE");
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        body.extend_from_slice(&fmt);
        if extra_chunk {
            // A LIST chunk between fmt and data, which real files carry and a
            // reader that assumes data comes second would choke on.
            body.extend_from_slice(b"LIST");
            body.extend_from_slice(&5_u32.to_le_bytes());
            body.extend_from_slice(b"INFO\0");
            body.push(0); // pad to even
        }
        body.extend_from_slice(b"data");
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(&data);

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn a_mono_clip_decodes_to_its_samples() {
        let clip = Clip::decode(&wav(1, 44100, &[0, 16384, -16384, 32767], false)).expect("valid");

        assert_eq!(clip.sample_rate, 44100);
        assert_eq!(clip.samples.len(), 4);
        assert!((clip.samples[1] - 0.5).abs() < 1e-3, "{:?}", clip.samples);
        assert!((clip.samples[2] + 0.5).abs() < 1e-3);
    }

    /// Stereo folds to mono, because a positioned sound is panned here and a
    /// stereo file has already decided what each ear hears.
    #[test]
    fn a_stereo_clip_folds_to_mono() {
        // Two frames: (1.0, 0.0) and (-1.0, -1.0).
        let clip = Clip::decode(&wav(2, 48000, &[32767, 0, -32768, -32768], false)).expect("valid");

        assert_eq!(clip.samples.len(), 2, "frames, not samples");
        assert!((clip.samples[0] - 0.5).abs() < 1e-3, "{:?}", clip.samples);
        assert!((clip.samples[1] + 1.0).abs() < 1e-3);
    }

    /// Real files carry LIST and cue chunks between `fmt ` and `data`. A
    /// reader that assumes `data` is second rejects them.
    #[test]
    fn a_chunk_between_fmt_and_data_is_skipped() {
        let clip = Clip::decode(&wav(1, 22050, &[0, 32767], true)).expect("valid");

        assert_eq!(clip.sample_rate, 22050);
        assert_eq!(clip.samples.len(), 2);
    }

    #[test]
    fn duration_comes_from_the_rate() {
        let clip = Clip::decode(&wav(1, 1000, &[0; 500], false)).expect("valid");

        assert!((clip.seconds() - 0.5).abs() < 1e-4, "{}", clip.seconds());
    }

    #[test]
    fn something_that_is_not_a_wav_is_refused() {
        assert!(Clip::decode(b"this is not audio").is_err());
        assert!(Clip::decode(&[]).is_err());
    }

    /// A truncated file must be an error, not a panic on a slice index. Audio
    /// is loaded from wherever a scene points, and a half-written file is an
    /// ordinary thing to meet.
    #[test]
    fn a_truncated_file_is_refused_rather_than_panicking() {
        let full = wav(1, 44100, &[0, 1, 2, 3], false);
        for cut in [13, 20, 30, full.len() - 2] {
            assert!(Clip::decode(&full[..cut]).is_err(), "cut at {cut}");
        }
    }

    /// **The encoder is checked against the decoder that already worked.** A
    /// hand-written RIFF header is a pile of little-endian offsets, and every
    /// one of them is silently wrong rather than loudly wrong — a bad header
    /// gives a file that opens and plays noise.
    #[test]
    fn an_encoded_wav_decodes_back_to_what_went_in() {
        let original: Vec<f32> =
            (0..2000).map(|i| ((i as f32) * 0.01).sin() * 0.75).collect();
        let bytes = Clip::encode(&original, 48_000, 1);
        let back = Clip::decode(&bytes).expect("the encoder produced a file the decoder rejects");

        assert_eq!(back.sample_rate, 48_000);
        assert_eq!(back.samples.len(), original.len());
        for (i, (a, b)) in original.iter().zip(&back.samples).enumerate() {
            // 16-bit quantisation is one part in 32767; anything worse is a
            // scaling or endianness mistake rather than rounding.
            assert!((a - b).abs() < 1.0 / 16_000.0, "sample {i}: {a} became {b}");
        }
    }

    /// Out-of-range input must not wrap to the opposite rail.
    #[test]
    fn encoding_clamps_instead_of_wrapping() {
        let bytes = Clip::encode(&[2.0, -2.0], 48_000, 1);
        let back = Clip::decode(&bytes).expect("decodes");
        assert!(back.samples[0] > 0.9, "+2.0 wrapped to {}", back.samples[0]);
        assert!(back.samples[1] < -0.9, "-2.0 wrapped to {}", back.samples[1]);
    }
}
