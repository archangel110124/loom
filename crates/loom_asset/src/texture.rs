//! Texture import: PNG in, a mip chain out.
//!
//! # Why mips are generated here rather than skipped
//!
//! A texture sampled without mips aliases the moment it is minified, and the
//! aliasing is *temporal* — it crawls as the camera moves. That matters more
//! in this engine than in most: the agent checks its own work by rendering a
//! PNG and looking at it, so noise that a human would dismiss as shimmer is
//! noise the agent has to reason about. Mips also cut sampling bandwidth
//! sharply, because a minified draw then reads a small level that stays in
//! cache instead of thrashing the full-resolution image.
//!
//! # Colour space is not cosmetic
//!
//! Albedo is authored in sRGB, which is a non-linear encoding. Averaging four
//! sRGB bytes to make a mip texel is therefore averaging the wrong quantity:
//! it darkens every mip level, and the error compounds down the chain, so a
//! checkerboard that should converge to mid-grey converges to something
//! visibly darker instead. Levels are built in linear light and re-encoded.
//!
//! Normal maps are not colours at all — they are vectors that happen to be
//! stored in a colour format — so they are averaged directly.

use crate::AssetError;

/// How a texture's bytes are to be interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    /// Non-linear, gamma-encoded. What an albedo map is authored in.
    Srgb,
    /// Values that mean what they say: normal maps, roughness, masks.
    Linear,
}

/// A decoded texture and its mip chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub space: ColorSpace,
    /// RGBA8, level 0 first, each level half the size of the last down to 1x1.
    pub levels: Vec<Vec<u8>>,
}

impl Texture {
    /// How many mip levels a texture of this size has.
    #[must_use]
    pub fn level_count(&self) -> u32 {
        u32::try_from(self.levels.len()).unwrap_or(1)
    }
}

/// sRGB byte to linear float, per the sRGB transfer function.
fn to_linear(byte: u8) -> f32 {
    let c = f32::from(byte) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear float back to an sRGB byte.
fn to_srgb(linear: f32) -> u8 {
    let c = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (c * 255.0).round().clamp(0.0, 255.0) as u8
    }
}

/// Halve a level, averaging 2x2 blocks.
///
/// Odd dimensions round down and drop the last row or column. That is a real
/// approximation — the correct thing is a weighted three-tap filter — but it
/// only bites on non-power-of-two textures, which are worth avoiding anyway
/// because the GPU pads them. `ponytail:` box filter; upgrade to a Kaiser or
/// three-tap filter if NPOT textures start mattering.
fn halve(src: &[u8], width: u32, height: u32, space: ColorSpace) -> (Vec<u8>, u32, u32) {
    let (w, h) = ((width / 2).max(1), (height / 2).max(1));
    let mut out = Vec::with_capacity((w * h * 4) as usize);

    for y in 0..h {
        for x in 0..w {
            for channel in 0..4 {
                // Alpha is linear even in an sRGB texture: the transfer
                // function applies to colour, not to coverage. Averaging it
                // through the sRGB curve would distort every soft edge.
                let srgb = space == ColorSpace::Srgb && channel < 3;
                let mut total = 0.0_f32;
                for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    let sy = (y * 2 + dy).min(height - 1);
                    let sx = (x * 2 + dx).min(width - 1);
                    let byte = src[((sy * width + sx) * 4 + channel) as usize];
                    total += if srgb { to_linear(byte) } else { f32::from(byte) };
                }
                let mean = total / 4.0;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                out.push(if srgb { to_srgb(mean) } else { mean.round() as u8 });
            }
        }
    }
    (out, w, h)
}

/// Build the full mip chain from level 0, down to 1x1.
#[must_use]
pub fn mip_chain(level0: Vec<u8>, width: u32, height: u32, space: ColorSpace) -> Vec<Vec<u8>> {
    let mut levels = vec![level0];
    let (mut w, mut h) = (width, height);
    while w > 1 || h > 1 {
        let last = levels.last().expect("levels always has level 0");
        let (next, nw, nh) = halve(last, w, h, space);
        levels.push(next);
        w = nw;
        h = nh;
    }
    levels
}

/// Decode a PNG and build its mip chain.
///
/// # Errors
/// [`AssetError::Io`] if the file cannot be read, [`AssetError::Unsupported`]
/// if it is not a PNG this can turn into RGBA8.
pub fn load(path: &std::path::Path, space: ColorSpace) -> Result<Texture, AssetError> {
    let file = std::fs::File::open(path).map_err(AssetError::Io)?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    // Everything becomes 8-bit RGBA: one GPU format for every texture means
    // one bindless array rather than one per format combination.
    decoder.set_transformations(png::Transformations::normalize_to_color8());

    let mut reader = decoder
        .read_info()
        .map_err(|e| AssetError::Unsupported(format!("{}: {e}", path.display())))?;
    let mut raw = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut raw)
        .map_err(|e| AssetError::Unsupported(format!("{}: {e}", path.display())))?;

    let (width, height) = (info.width, info.height);
    if width == 0 || height == 0 {
        return Err(AssetError::Unsupported(format!(
            "{} is {width}x{height}",
            path.display()
        )));
    }

    // Widen whatever came out to RGBA. `normalize_to_color8` leaves the
    // channel count alone, so a greyscale or RGB source still arrives narrow.
    let pixels = (width * height) as usize;
    let rgba = match info.color_type {
        png::ColorType::Rgba => raw,
        png::ColorType::Rgb => (0..pixels)
            .flat_map(|i| [raw[i * 3], raw[i * 3 + 1], raw[i * 3 + 2], 255])
            .collect(),
        png::ColorType::Grayscale => (0..pixels)
            .flat_map(|i| [raw[i], raw[i], raw[i], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => (0..pixels)
            .flat_map(|i| [raw[i * 2], raw[i * 2], raw[i * 2], raw[i * 2 + 1]])
            .collect(),
        other => {
            return Err(AssetError::Unsupported(format!(
                "{}: {other:?} is not a colour type this can widen to RGBA8",
                path.display()
            )));
        }
    };

    Ok(Texture {
        name: path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
        width,
        height,
        space,
        levels: mip_chain(rgba, width, height, space),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_halves_down_to_one_texel() {
        let level0 = vec![128; 8 * 8 * 4];

        let levels = mip_chain(level0, 8, 8, ColorSpace::Linear);

        // 8, 4, 2, 1.
        assert_eq!(levels.len(), 4);
        assert_eq!(levels[1].len(), 4 * 4 * 4);
        assert_eq!(levels[3].len(), 4);
    }

    /// A non-square texture keeps halving until *both* axes reach 1, so the
    /// chain is as long as the longer side demands.
    #[test]
    fn a_non_square_chain_runs_until_both_axes_are_one() {
        let levels = mip_chain(vec![255; 8 * 2 * 4], 8, 2, ColorSpace::Linear);

        // 8x2, 4x1, 2x1, 1x1.
        assert_eq!(levels.len(), 4);
        assert_eq!(levels[3].len(), 4);
    }

    /// **The bug this exists to catch.** Averaging sRGB bytes directly is
    /// averaging a non-linear encoding, which darkens every level. Black and
    /// white average to mid-*grey*, which in sRGB is about 188 — not 128.
    #[test]
    fn srgb_levels_are_averaged_in_linear_light() {
        // A 2x2 checker: two black texels, two white.
        let mut level0 = vec![0_u8; 2 * 2 * 4];
        for (i, byte) in level0.iter_mut().enumerate() {
            let white = (i / 4) % 2 == 0;
            *byte = if i % 4 == 3 || white { 255 } else { 0 };
        }

        let levels = mip_chain(level0, 2, 2, ColorSpace::Srgb);

        let red = levels[1][0];
        assert!(
            (186..=190).contains(&red),
            "half black half white should be sRGB ~188, got {red}"
        );
    }

    /// Normal maps are vectors in a colour container. Running them through the
    /// sRGB curve would bend every one of them.
    #[test]
    fn linear_levels_are_averaged_directly() {
        let mut level0 = vec![255_u8; 2 * 2 * 4];
        for (i, byte) in level0.iter_mut().enumerate() {
            let white = (i / 4) % 2 == 0;
            if i % 4 != 3 && !white {
                *byte = 0;
            }
        }

        let levels = mip_chain(level0, 2, 2, ColorSpace::Linear);

        assert_eq!(levels[1][0], 128, "linear data averages arithmetically");
    }

    /// Write a PNG with the given colour type, and read it back.
    fn round_trip(color: png::ColorType, data: &[u8], w: u32, h: u32) -> Texture {
        let dir = std::env::temp_dir().join(format!("loom_tex_{}_{color:?}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("t.png");

        let file = std::fs::File::create(&path).expect("create");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("header");
        writer.write_image_data(data).expect("write");
        drop(writer);

        let texture = load(&path, ColorSpace::Srgb).expect("a PNG this wrote itself");
        let _ = std::fs::remove_dir_all(&dir);
        texture
    }

    /// A three-channel PNG is ordinary — most albedo maps have no alpha — and
    /// the decoder does not widen it. Missing that produced a texture whose
    /// rows were shifted by a third, which reads as diagonal smearing.
    #[test]
    fn an_rgb_png_is_widened_to_rgba() {
        let texture = round_trip(png::ColorType::Rgb, &[255, 0, 0, 0, 255, 0], 2, 1);

        assert_eq!(texture.width, 2);
        assert_eq!(texture.levels[0], [255, 0, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn a_greyscale_png_is_widened_to_rgba() {
        let texture = round_trip(png::ColorType::Grayscale, &[0, 255], 2, 1);

        assert_eq!(texture.levels[0], [0, 0, 0, 255, 255, 255, 255, 255]);
    }

    /// Alpha is coverage, not colour: the sRGB transfer function does not
    /// apply to it. Encoding it would distort every soft edge in the texture.
    #[test]
    fn alpha_stays_linear_in_an_srgb_texture() {
        let mut level0 = vec![0_u8; 2 * 2 * 4];
        for (i, byte) in level0.iter_mut().enumerate() {
            if i % 4 == 3 && (i / 4) % 2 == 0 {
                *byte = 255;
            }
        }

        let levels = mip_chain(level0, 2, 2, ColorSpace::Srgb);

        assert_eq!(levels[1][3], 128, "half opaque is half opaque");
    }
}
