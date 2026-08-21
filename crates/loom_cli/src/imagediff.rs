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

/// Isolated pixels: how many sit far from the median of their eight neighbours.
///
/// **The measurement three doc comments in this project already argue from and
/// no code computed.** `REFLECT_MAX_RADIANCE`, ADR 0019 and ADR 0050 each cite
/// a salt count to justify a constant, and each of them re-derived it from
/// prose in a private script. Neither `compare` nor `flicker` can see it:
/// `compare` prices a whole region moving and a firefly is one pixel, and
/// `flicker` needs three frames and cannot tell "wider" from "less stable" —
/// rain's 2.5 px floor was chosen against `flicker`'s objection on exactly
/// this ground.
///
/// A firefly, a stipple and a lone sub-pixel bead all share one shape: a pixel
/// whose neighbourhood does not agree with it. The median of the eight is
/// robust to an edge running through the window, which a mean is not, so a
/// silhouette does not read as salt.
///
/// Per channel, counted where the distance exceeds `threshold` in 0-255 units;
/// the worst channel is what a caller compares. The border row and column are
/// skipped because they have no eight.
pub(crate) fn salt(image: &Image, threshold: u8, rect: Option<(u32, u32, u32, u32)>) -> [u64; 3] {
    let mut counts = [0_u64; 3];
    if image.width < 3 || image.height < 3 {
        return counts;
    }
    // Clamped and inset by one, for `rect_means`' reason: a crop is a region
    // of interest, not an index, and the border has no eight neighbours.
    let (x, y, w, h) = rect.unwrap_or((0, 0, image.width, image.height));
    let first_column = (x.max(1)) as usize;
    let first_row = (y.max(1)) as usize;
    let last_column = x.saturating_add(w).min(image.width - 1) as usize;
    let last_row = y.saturating_add(h).min(image.height - 1) as usize;
    let stride = image.width as usize * 4;
    for row in first_row..last_row {
        for column in first_column..last_column {
            let base = row * stride + column * 4;
            for (channel, count) in counts.iter_mut().enumerate() {
                let mut ring = [0_u8; 8];
                let mut n = 0;
                for dy in [-1_isize, 0, 1] {
                    for dx in [-1_isize, 0, 1] {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let at = (row as isize + dy) as usize * stride
                            + (column as isize + dx) as usize * 4
                            + channel;
                        ring[n] = image.pixels[at];
                        n += 1;
                    }
                }
                ring.sort_unstable();
                let median = (u16::from(ring[3]) + u16::from(ring[4])) / 2;
                let here = u16::from(image.pixels[base + channel]);
                if here.abs_diff(median) > u16::from(threshold) {
                    *count += 1;
                }
            }
        }
    }
    counts
}

/// How much of the image has gone constant across a 2x2 fragment quad.
///
/// **The acceptance test for `ambientVisibility`'s quad share, which nothing
/// else in this repository can compute.** ADR 0074 trades rays for a share
/// across the quad; the failure mode it will eventually produce is not noise
/// and not flicker but *blockiness* — shading going constant over each 2x2. No
/// existing instrument sees it. `flicker` reads exactly 0.00000, by
/// construction, because the pattern is screen-locked and perfectly still.
/// `salt` looks for a pixel unlike its eight neighbours and a 2x2 block is
/// four pixels agreeing. `compare` prices distance from a reference, which
/// *falls* as resolution rises while this rises with it.
///
/// The mechanism is the pixel grid: a horizontal pair `(x, x+1)` lies **inside**
/// one fragment quad when `x` is even and **across** two quads when `x` is odd.
/// So compare the mean absolute neighbour difference on odd pairs against even
/// pairs. An unquantised image has no idea where the quads are and scores 1.0;
/// shading that has gone quad-constant flattens the inside pairs and pushes the
/// ratio up. Measured on `primitives` at 1920x1080: 1.02 converged, 1.36 at the
/// shipped four rays per lane, 1.85 at one.
///
/// Returns `(x, y)`. Both axes, because a filter can be wider in one.
///
/// **Only ever compare a scene against itself.** The absolute level is a
/// property of the content — a frame that is mostly silhouette has no flat
/// region to quantise and sits at 1.00 however badly the share is behaving,
/// which is why `spruce` cannot act as this gate's subject and `primitives`
/// can.
pub(crate) fn quad_ratio(image: &Image) -> (f64, f64) {
    let stride = image.width as usize * 4;
    let mut sums = [0.0_f64; 4];
    let mut counts = [0_u64; 4];
    let mut add = |slot: usize, a: usize, b: usize| {
        let d: u32 = (0..3)
            .map(|c| u32::from(image.pixels[a + c].abs_diff(image.pixels[b + c])))
            .sum();
        sums[slot] += f64::from(d);
        counts[slot] += 1;
    };
    for row in 0..image.height as usize {
        for column in 0..image.width.saturating_sub(1) as usize {
            let at = row * stride + column * 4;
            add(usize::from(column % 2 == 1), at, at + 4);
        }
    }
    for row in 0..image.height.saturating_sub(1) as usize {
        for column in 0..image.width as usize {
            let at = row * stride + column * 4;
            add(2 + usize::from(row % 2 == 1), at, at + stride);
        }
    }
    let mean = |slot: usize| sums[slot] / counts[slot].max(1) as f64;
    // **A zero denominator is the worst case, not a missing answer.** No inside-
    // quad difference at all means the shading is perfectly quad-constant, which
    // is the failure this measures taken to its limit — so it is infinity, and
    // returning 0.0 there (which the first draft did, and a test caught) reads
    // in exactly the wrong direction. Both zero is a flat image: no structure
    // either way, so 1.0.
    let ratio = |inside: usize, across: usize| match (mean(inside), mean(across)) {
        (i, a) if i > 0.0 => a / i,
        (_, a) if a > 0.0 => f64::INFINITY,
        _ => 1.0,
    };
    (ratio(0, 1), ratio(2, 3))
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

    /// The two ends of `quad_ratio`, because a ratio near 1.0 is exactly what
    /// a broken implementation also returns.
    ///
    /// A per-pixel ramp knows nothing about the pixel grid's parity, so it must
    /// score 1.0. The same ramp held constant over each 2x2 — which is what the
    /// AO share does in the limit — must score far above it, and does: the
    /// inside-quad difference goes to zero, so the ratio is bounded only by
    /// arithmetic. Fault-injectable in one character: swap the parity test in
    /// `quad_ratio` and the second assertion inverts.
    #[test]
    fn quad_constant_shading_scores_far_above_a_ramp() {
        let mut ramp = flat(32, 32, [0, 0, 0, 255]);
        let mut blocky = flat(32, 32, [0, 0, 0, 255]);
        for y in 0..32_usize {
            for x in 0..32_usize {
                let at = (y * 32 + x) * 4;
                for channel in 0..3 {
                    ramp.pixels[at + channel] = u8::try_from(x * 4 + y * 2).unwrap_or(255);
                    // Same ramp, sampled at the quad's corner: constant over
                    // each 2x2.
                    blocky.pixels[at + channel] =
                        u8::try_from((x & !1) * 4 + (y & !1) * 2).unwrap_or(255);
                }
            }
        }
        let (rx, ry) = quad_ratio(&ramp);
        assert!((rx - 1.0).abs() < 0.01, "a ramp has no parity: {rx}");
        assert!((ry - 1.0).abs() < 0.01, "a ramp has no parity: {ry}");

        let (bx, by) = quad_ratio(&blocky);
        assert!(bx > 4.0, "quad-constant shading must be visible: {bx}");
        assert!(by > 4.0, "quad-constant shading must be visible: {by}");
    }

    /// The self-check the metric is worthless without: a spike above the
    /// threshold is counted and one below it is not. Every number this project
    /// quotes as "salt" is this function, so a silent off-by-one in it would
    /// re-price two ADRs.
    #[test]
    fn a_spike_over_the_threshold_is_salt_and_one_under_it_is_not() {
        let mut image = flat(16, 16, [10, 10, 10, 255]);
        let at = (8 * 16 + 8) * 4;
        image.pixels[at] = 50; // +40, over a threshold of 24
        assert_eq!(salt(&image, 24, None), [1, 0, 0]);
        image.pixels[at] = 30; // +20, under it
        assert_eq!(salt(&image, 24, None), [0, 0, 0]);
    }

    /// An edge is not salt. The median of eight is what buys this — half the
    /// window agreeing is enough — and a mean would report every silhouette in
    /// the frame, which would make the metric useless for the reflection work
    /// it was written for.
    #[test]
    fn a_straight_edge_is_not_salt() {
        let mut image = flat(16, 16, [10, 10, 10, 255]);
        for row in 0..16 {
            for column in 8..16 {
                image.pixels[(row * 16 + column) * 4] = 200;
            }
        }
        assert_eq!(salt(&image, 24, None), [0, 0, 0]);
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
