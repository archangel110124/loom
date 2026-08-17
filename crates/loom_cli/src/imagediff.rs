//! Comparing two renders.
//!
//! **The gap this closes is named in `CLAUDE.md`'s own definition of green.**
//! The suite has clippy, unit tests, determinism hashes and zero validation
//! messages — and no pixel diff. Every render in this project has been checked
//! by a human opening the PNG and looking at it. That works for one change at
//! a time and it does not scale to the four most visually regression-prone
//! systems in the backlog (water, rain, wind, vegetation), where a numeric
//! change to a shader is otherwise unverifiable.
//!
//! **Not exact equality.** A driver update shifts pixels by a bit or two
//! without anything being wrong, and a harness that cries wolf on every driver
//! bump is a harness people learn to bless without reading. So a pixel counts
//! as different only past a per-channel threshold, and an image fails only
//! when enough pixels differ — or when any single pixel differs enormously,
//! which catches a small but blatant change that a fraction-of-the-image rule
//! would round away.
//!
//! Lives here rather than in `xtask` because `xtask` deliberately has no
//! dependencies: it drives the real `loom` binary as a subprocess so a gate
//! cannot be fooled by the same bug twice. Decoding a PNG needs a decoder, so
//! the comparison is a subcommand and `xtask` orchestrates it.

/// How much difference is not a regression.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Tolerance {
    /// Per-channel difference, 0-255, below which a pixel is "the same".
    /// Absorbs driver rounding without absorbing a visible change.
    pub(crate) channel: u8,
    /// Fraction of pixels allowed to differ before the image fails.
    pub(crate) fraction: f64,
    /// A single pixel differing by more than this fails the image on its own,
    /// however few of them there are. A small bright artifact — one wrong
    /// light, a NaN pixel — moves very few pixels a very long way.
    pub(crate) worst: u8,
}

impl Default for Tolerance {
    fn default() -> Self {
        Self {
            // **Two, calibrated rather than guessed.** Two independent renders
            // of the same scene on this machine are bit-identical — worst
            // channel delta zero — so the only thing tolerance buys is
            // survival across a driver update, which shifts a count or two.
            //
            // Eight was the first guess and it was useless: multiplying the
            // diffuse term by 0.96, a real one-line shader change, moves 17%
            // of the pixels in `materials.loom` and its worst single-channel
            // delta is 4. At a threshold of 8 that change was invisible and
            // the harness reported everything fine.
            channel: 2,
            // A tenth of a percent. At 320x200 that is 64 pixels: more than
            // driver noise, far less than any change worth seeing.
            fraction: 0.001,
            worst: 72,
        }
    }
}

/// What the comparison found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Difference {
    pub(crate) pixels: usize,
    pub(crate) differing: usize,
    /// Largest per-channel difference anywhere in the image.
    pub(crate) worst: u8,
    /// Mean absolute per-channel difference, for a sense of scale.
    pub(crate) mean: f64,
}

impl Difference {
    pub(crate) fn fraction(&self) -> f64 {
        if self.pixels == 0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        {
            self.differing as f64 / self.pixels as f64
        }
    }

    pub(crate) fn passes(&self, tolerance: Tolerance) -> bool {
        self.worst <= tolerance.worst && self.fraction() <= tolerance.fraction
    }
}

/// A decoded image, as 8-bit RGBA.
pub(crate) struct Image {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<u8>,
}

/// Read a PNG as RGBA8.
///
/// # Errors
/// A message naming what went wrong, for the caller to report as JSON.
pub(crate) fn load(path: &std::path::Path) -> Result<Image, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buffer = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    buffer.truncate(info.buffer_size());

    // Everything this project writes is RGBA8; anything else is a file that
    // did not come from here, and guessing at it would compare nonsense.
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "{}: expected 8-bit RGBA, found {:?} at {:?}",
            path.display(),
            info.color_type,
            info.bit_depth
        ));
    }
    Ok(Image {
        width: info.width,
        height: info.height,
        pixels: buffer,
    })
}

/// How much of a frame is *temporal noise* rather than motion.
///
/// **The measurement `compare` cannot make.** Counting pixels that changed
/// between two frames conflates flicker with parallax: widening distant grass
/// so it stops twinkling covers more of the screen, so more pixels change, and
/// a pixel-count metric reports the fix as a regression. That happened, which
/// is why this exists.
///
/// Coherent motion is smooth in time — a pixel sweeping across an edge moves
/// roughly linearly over three frames, so the middle frame is close to the
/// average of its neighbours. A pixel that flickers is not: it is bright,
/// dark, bright. The second difference `|b - (a + c) / 2|` is near zero for
/// the first and large for the second.
///
/// Returned as a mean over every channel of every pixel, in 0-255 units.
///
/// # Errors
/// When the three frames are not the same size.
pub(crate) fn flicker(a: &Image, b: &Image, c: &Image) -> Result<f64, String> {
    if a.pixels.len() != b.pixels.len() || b.pixels.len() != c.pixels.len() {
        return Err(format!(
            "sizes differ: {}x{}, {}x{}, {}x{}",
            a.width, a.height, b.width, b.height, c.width, c.height
        ));
    }

    let mut total = 0.0_f64;
    for ((x, y), z) in a.pixels.iter().zip(&b.pixels).zip(&c.pixels) {
        let predicted = (f64::from(*x) + f64::from(*z)) * 0.5;
        total += (f64::from(*y) - predicted).abs();
    }
    if b.pixels.is_empty() {
        return Ok(0.0);
    }
    #[allow(clippy::cast_precision_loss)]
    Ok(total / b.pixels.len() as f64)
}

/// Compare two images of the same size.
///
/// # Errors
/// When the dimensions differ — which is a difference no tolerance should
/// absorb, and reporting it as "every pixel changed" would hide the cause.
pub(crate) fn compare(a: &Image, b: &Image, tolerance: Tolerance) -> Result<Difference, String> {
    if a.width != b.width || a.height != b.height {
        return Err(format!(
            "size differs: {}x{} against {}x{}",
            a.width, a.height, b.width, b.height
        ));
    }

    let mut differing = 0_usize;
    let mut worst = 0_u8;
    let mut total = 0_u64;

    for (left, right) in a.pixels.chunks_exact(4).zip(b.pixels.chunks_exact(4)) {
        let mut pixel_worst = 0_u8;
        // Alpha included: an image that went transparent is a regression, and
        // comparing only colour would call it identical.
        for channel in 0..4 {
            let delta = left[channel].abs_diff(right[channel]);
            pixel_worst = pixel_worst.max(delta);
            total += u64::from(delta);
        }
        worst = worst.max(pixel_worst);
        if pixel_worst > tolerance.channel {
            differing += 1;
        }
    }

    let pixels = a.pixels.len() / 4;
    #[allow(clippy::cast_precision_loss)]
    let mean = if pixels == 0 {
        0.0
    } else {
        total as f64 / (pixels * 4) as f64
    };

    Ok(Difference {
        pixels,
        differing,
        worst,
        mean,
    })
}

/// Mean R, G and B over a rectangle, in 0-255 units.
///
/// **The measurement neither `compare` nor `flicker` can make.** Both answer
/// "did it change"; the water work needs "what colour is it", because the
/// acceptance test for refraction is a hue and a green-over-red excess in a
/// named crop of `shore`, hand-measured off a render from before the
/// regression. Without this the only way to check it is a private script,
/// and a number nobody else can reproduce is not a measurement.
///
/// Out-of-bounds is clamped rather than rejected: a crop is a region of
/// interest, not an index.
pub(crate) fn rect_means(image: &Image, rect: (u32, u32, u32, u32)) -> [f64; 3] {
    let (x, y, w, h) = rect;
    let x1 = (x + w).min(image.width);
    let y1 = (y + h).min(image.height);
    let mut total = [0.0_f64; 3];
    let mut count = 0_u64;
    for row in y.min(image.height)..y1 {
        for column in x.min(image.width)..x1 {
            let base = ((row * image.width + column) * 4) as usize;
            for (channel, sum) in total.iter_mut().enumerate() {
                *sum += f64::from(image.pixels[base + channel]);
            }
            count += 1;
        }
    }
    if count == 0 {
        return [0.0; 3];
    }
    #[allow(clippy::cast_precision_loss)]
    let n = count as f64;
    [total[0] / n, total[1] / n, total[2] / n]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(width: u32, height: u32, colour: [u8; 4]) -> Image {
        Image {
            width,
            height,
            pixels: colour
                .iter()
                .copied()
                .cycle()
                .take((width * height * 4) as usize)
                .collect(),
        }
    }

    #[test]
    fn an_image_matches_itself() {
        let a = flat(8, 8, [10, 20, 30, 255]);
        let b = flat(8, 8, [10, 20, 30, 255]);

        let diff = compare(&a, &b, Tolerance::default()).expect("same size");

        assert_eq!(diff.differing, 0);
        assert_eq!(diff.worst, 0);
        assert!(diff.passes(Tolerance::default()));
    }

    /// **The reason this is not exact equality.** A driver update moves pixels
    /// by a count or two with nothing wrong, and a harness that fails on that
    /// is one people learn to bless without reading.
    #[test]
    fn a_bit_of_driver_noise_is_not_a_regression() {
        let a = flat(16, 16, [100, 100, 100, 255]);
        let b = flat(16, 16, [102, 99, 101, 255]);

        let diff = compare(&a, &b, Tolerance::default()).expect("same size");

        assert_eq!(diff.differing, 0, "within the per-channel threshold");
        assert!(diff.passes(Tolerance::default()));
    }

    /// **The calibration, as a test.** A shift of four counts across the image
    /// is not driver noise — it is what multiplying a shader's diffuse term by
    /// 0.96 actually does, measured on `materials.loom`: 17% of pixels move
    /// and the worst single channel moves by 4. The first threshold here was
    /// 8, which made exactly that change invisible.
    #[test]
    fn a_four_count_shift_across_the_image_is_a_regression() {
        let a = flat(64, 64, [120, 120, 120, 255]);
        let b = flat(64, 64, [116, 120, 120, 255]);

        let diff = compare(&a, &b, Tolerance::default()).expect("same size");

        assert_eq!(diff.worst, 4, "the magnitude a 4% shader change produces");
        assert!(
            !diff.passes(Tolerance::default()),
            "a threshold that lets this through cannot verify a shader"
        );
    }

    #[test]
    fn a_changed_image_fails() {
        let a = flat(16, 16, [100, 100, 100, 255]);
        let b = flat(16, 16, [140, 100, 100, 255]);

        let diff = compare(&a, &b, Tolerance::default()).expect("same size");

        assert_eq!(diff.differing, 256, "every pixel");
        assert!(!diff.passes(Tolerance::default()));
    }

    /// **A small blatant artifact must fail.** One wrong light or a NaN pixel
    /// moves very few pixels a very long way, and a rule that only counts the
    /// fraction of the image would round it away.
    #[test]
    fn one_wildly_wrong_pixel_fails_on_its_own() {
        let a = flat(100, 100, [50, 50, 50, 255]);
        let mut b = flat(100, 100, [50, 50, 50, 255]);
        b.pixels[0] = 255;

        let diff = compare(&a, &b, Tolerance::default()).expect("same size");

        assert_eq!(diff.differing, 1, "one pixel in ten thousand");
        assert!(
            diff.fraction() < Tolerance::default().fraction,
            "and it is under the fraction limit, so only `worst` can catch it"
        );
        assert!(!diff.passes(Tolerance::default()));
    }

    /// The opposite: a great many pixels each moved a little. Neither rule
    /// alone catches both shapes of regression, which is why there are two.
    #[test]
    fn a_wide_shallow_change_fails_on_the_fraction() {
        let a = flat(100, 100, [50, 50, 50, 255]);
        let b = flat(100, 100, [70, 50, 50, 255]);

        let diff = compare(&a, &b, Tolerance::default()).expect("same size");

        assert!(diff.worst < Tolerance::default().worst, "no pixel moved far");
        assert!(!diff.passes(Tolerance::default()), "but nearly all of them moved");
    }


    /// **Smooth motion is not flicker.** A pixel ramping evenly across three
    /// frames is exactly the average of its neighbours, so the second
    /// difference is zero — which is the whole point, because a pixel-count
    /// metric would call this a large change.
    #[test]
    fn a_smoothly_moving_pixel_reads_as_no_flicker() {
        let a = flat(8, 8, [10, 10, 10, 255]);
        let b = flat(8, 8, [60, 60, 60, 255]);
        let c = flat(8, 8, [110, 110, 110, 255]);

        let measured = flicker(&a, &b, &c).expect("same size");

        assert!(measured < 0.01, "smooth motion measured as {measured}");
    }

    /// A pixel that goes bright, dark, bright is flicker, and reads as it —
    /// even though its start and end are identical, which a two-frame
    /// comparison of the ends would call no change at all.
    #[test]
    fn a_pixel_that_flickers_is_caught() {
        let a = flat(8, 8, [200, 200, 200, 255]);
        let b = flat(8, 8, [40, 40, 40, 255]);
        let c = flat(8, 8, [200, 200, 200, 255]);

        let measured = flicker(&a, &b, &c).expect("same size");

        assert!(measured > 100.0, "obvious flicker measured as only {measured}");
    }

    /// A still image has neither.
    #[test]
    fn a_still_image_has_no_flicker() {
        let a = flat(8, 8, [77, 88, 99, 255]);

        assert_eq!(flicker(&a, &a, &a).expect("same size"), 0.0);
    }
    /// A resize is not a difference to be measured against a tolerance — it is
    /// a different picture, and saying "100% of pixels changed" would bury the
    /// actual cause.
    #[test]
    fn a_size_change_is_an_error_not_a_difference() {
        let a = flat(8, 8, [0, 0, 0, 255]);
        let b = flat(8, 9, [0, 0, 0, 255]);

        let error = compare(&a, &b, Tolerance::default()).expect_err("sizes differ");

        assert!(error.contains("8x8") && error.contains("8x9"), "{error}");
    }

    /// Transparency counts. An image that lost its alpha is a regression, and
    /// comparing only the colour channels calls it identical.
    #[test]
    fn a_change_in_alpha_alone_is_caught() {
        let a = flat(16, 16, [80, 80, 80, 255]);
        let b = flat(16, 16, [80, 80, 80, 100]);

        let diff = compare(&a, &b, Tolerance::default()).expect("same size");

        assert!(!diff.passes(Tolerance::default()));
    }

    /// **The crop has to be the crop.** The whole value of `--rect` is that it
    /// reports the colour of one band and not of the frame, so an off-by-one
    /// origin or a mean taken over everything is the failure that matters. The
    /// surround here is deliberately the complement of the patch in every
    /// channel: averaging any of it in moves the answer immediately.
    #[test]
    fn rect_means_read_the_rectangle_and_nothing_around_it() {
        let mut image = flat(16, 16, [200, 0, 200, 255]);
        for row in 4..12 {
            for column in 2..10 {
                let base = ((row * 16 + column) * 4) as usize;
                image.pixels[base..base + 4].copy_from_slice(&[10, 60, 110, 255]);
            }
        }

        assert_eq!(rect_means(&image, (2, 4, 8, 8)), [10.0, 60.0, 110.0]);
        // One column further left is one column of surround, and 8 of 64
        // pixels of magenta is a shift no rounding can hide.
        assert!(rect_means(&image, (1, 4, 8, 8))[1] < 60.0);
    }

    /// A crop that runs off the edge is clamped, not an error: it is a region
    /// of interest, not an index, and a render at a different size should
    /// still answer rather than refuse.
    #[test]
    fn a_rect_past_the_edge_is_clamped() {
        let image = flat(8, 8, [40, 50, 60, 255]);

        assert_eq!(rect_means(&image, (4, 4, 999, 999)), [40.0, 50.0, 60.0]);
        assert_eq!(rect_means(&image, (99, 99, 4, 4)), [0.0, 0.0, 0.0]);
    }
}
