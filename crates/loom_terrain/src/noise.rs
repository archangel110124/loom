//! Value noise, fBm, ridged multifractal, and domain warping.
//!
//! Written here rather than pulled from a noise crate because the terrain doc
//! is explicit about why: prefab fractal types hide the octave combination and
//! the derivative access, and both are needed — derivatives give analytical
//! normals with no finite-difference artifacts, and octave control is what
//! separates a multifractal from a plain sum.

/// A small deterministic PRNG.
///
/// Seeded from the recipe, never thread-local (§A.3): a scene with random
/// features must replay identically, and `thread_rng` makes that impossible.
/// `clippy.toml` disallows `thread_rng` precisely to stop this being
/// accidental.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        // Avoid the zero state, which would make xorshift emit only zeroes.
        Self(seed.max(1).wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }

    /// xorshift64*, deterministic across platforms.
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in 0..1.
    pub fn next_f32(&mut self) -> f32 {
        #[allow(clippy::cast_precision_loss)]
        {
            (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
        }
    }
}

/// Deterministic hash of a lattice point.
fn hash(x: i32, y: i32, seed: u64) -> f32 {
    let mut h = seed
        ^ (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    #[allow(clippy::cast_precision_loss)]
    {
        (h >> 40) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

/// Smootherstep — C2 continuous, so second derivatives do not jump and
/// analytical normals stay smooth across lattice cells.
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Value noise in -1..1.
#[must_use]
pub fn value(x: f32, y: f32, seed: u64) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (fade(x - x.floor()), fade(y - y.floor()));

    let a = hash(x0, y0, seed);
    let b = hash(x0 + 1, y0, seed);
    let c = hash(x0, y0 + 1, seed);
    let d = hash(x0 + 1, y0 + 1, seed);

    (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy
}

/// Fractal sum: octaves at doubling frequency and halving amplitude.
///
/// Musgrave's terminology, because it is what every artist and reference
/// already uses: **frequency** is feature size, **amplitude** is height,
/// **lacunarity** is the frequency multiplier per octave.
#[must_use]
pub fn fbm(x: f32, y: f32, frequency: f32, octaves: u32, lacunarity: f32, gain: f32, seed: u64) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut freq = frequency;
    let mut norm = 0.0;
    for octave in 0..octaves.max(1) {
        sum += value(x * freq, y * freq, seed ^ u64::from(octave)) * amp;
        norm += amp;
        amp *= gain;
        freq *= lacunarity;
    }
    if norm > 0.0 { sum / norm } else { 0.0 }
}

/// Ridged multifractal.
///
/// Two things distinguish it from fBm. The absolute value inverted per octave
/// makes ridges instead of rolling hills. And **each octave is weighted by the
/// previous one**, so ground that is already high receives more detail while
/// valleys stay smooth — that weighting is what makes commercial terrain look
/// better than a naive fractal sum.
#[must_use]
pub fn ridged(x: f32, y: f32, frequency: f32, octaves: u32, seed: u64) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = frequency;
    let mut weight = 1.0_f32;
    let mut norm = 0.0;

    for octave in 0..octaves.max(1) {
        let n = 1.0 - value(x * freq, y * freq, seed ^ u64::from(octave)).abs();
        let n = n * n * weight;
        weight = (n * 2.0).clamp(0.0, 1.0);
        sum += n * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    if norm > 0.0 { sum / norm * 2.0 - 1.0 } else { 0.0 }
}

/// Domain warping: distort the *input* coordinates with another noise sample.
///
/// The cheapest large quality win on the list. It produces terrain that reads
/// as eroded — river valleys, steep peaks — for the cost of extra samples, and
/// it has only two parameters, so an agent uses it correctly.
#[must_use]
pub fn warp(x: f32, y: f32, strength: f32, frequency: f32, seed: u64) -> (f32, f32) {
    if strength.abs() < 1e-6 {
        return (x, y);
    }
    let wx = value(x * frequency * 0.5, y * frequency * 0.5, seed ^ 0xA5A5);
    let wy = value(x * frequency * 0.5, y * frequency * 0.5, seed ^ 0x5A5A);
    (x + wx * strength, y + wy * strength)
}

/// Value noise plus its analytical derivatives, as `(value, d/dx, d/dy)`.
///
/// Analytical rather than finite-difference: it is faster, exact, and gives
/// normals with no differencing artifacts — free correct shading for terrain.
#[must_use]
pub fn value_with_derivative(x: f32, y: f32, seed: u64) -> (f32, f32, f32) {
    #[allow(clippy::cast_possible_truncation)]
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (tx, ty) = (x - x.floor(), y - y.floor());
    let (u, v) = (fade(tx), fade(ty));
    // d/dt of smootherstep.
    let du = 30.0 * tx * tx * (tx * (tx - 2.0) + 1.0);
    let dv = 30.0 * ty * ty * (ty * (ty - 2.0) + 1.0);

    let a = hash(x0, y0, seed);
    let b = hash(x0 + 1, y0, seed);
    let c = hash(x0, y0 + 1, seed);
    let d = hash(x0 + 1, y0 + 1, seed);

    let k0 = a;
    let k1 = b - a;
    let k2 = c - a;
    let k3 = a - b - c + d;

    (
        k0 + k1 * u + k2 * v + k3 * u * v,
        (k1 + k3 * v) * du,
        (k2 + k3 * u) * dv,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rng_is_deterministic_for_a_seed() {
        let a: Vec<u64> = (0..8).scan(Rng::new(42), |r, _| Some(r.next_u64())).collect();
        let b: Vec<u64> = (0..8).scan(Rng::new(42), |r, _| Some(r.next_u64())).collect();

        assert_eq!(a, b);
        assert_ne!(a[0], Rng::new(43).next_u64(), "a different seed must differ");
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn value_noise_stays_in_range_and_is_continuous() {
        let mut previous = value(0.0, 0.0, 1);
        for step in 1..200 {
            let x = step as f32 * 0.05;
            let v = value(x, 0.0, 1);
            assert!((-1.0..=1.0).contains(&v), "out of range at {x}: {v}");
            assert!((v - previous).abs() < 0.35, "discontinuity at {x}");
            previous = v;
        }
    }

    #[test]
    fn fbm_is_deterministic_and_bounded() {
        let a = fbm(1.5, 2.5, 0.01, 5, 2.0, 0.5, 7);
        let b = fbm(1.5, 2.5, 0.01, 5, 2.0, 0.5, 7);

        assert!((a - b).abs() < f32::EPSILON);
        assert!((-1.0..=1.0).contains(&a), "fbm out of range: {a}");
    }

    /// Ridged terrain should spend more of its range up high — that is what
    /// makes it read as ridges rather than rolling hills.
    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn ridged_differs_from_plain_fbm() {
        let f: f32 = (0..64).map(|i| fbm(i as f32, 3.0, 0.05, 5, 2.0, 0.5, 3)).sum();
        let r: f32 = (0..64).map(|i| ridged(i as f32, 3.0, 0.05, 5, 3)).sum();

        assert!((f - r).abs() > 1.0, "ridged should not equal fbm");
    }

    #[test]
    fn zero_strength_warp_is_the_identity() {
        assert_eq!(warp(3.0, 4.0, 0.0, 0.01, 1), (3.0, 4.0));
        assert_ne!(warp(3.0, 4.0, 10.0, 0.01, 1), (3.0, 4.0));
    }

    /// The analytical derivative must match a finite difference, or "free
    /// correct normals" is just "free wrong normals".
    #[test]
    fn analytical_derivatives_match_finite_differences() {
        const H: f32 = 1e-3;
        for (x, y) in [(0.3, 0.7), (1.25, 2.9), (5.5, 0.1)] {
            let (_, dx, dy) = value_with_derivative(x, y, 11);
            let fd_x = (value(x + H, y, 11) - value(x - H, y, 11)) / (2.0 * H);
            let fd_y = (value(x, y + H, 11) - value(x, y - H, 11)) / (2.0 * H);

            assert!((dx - fd_x).abs() < 0.02, "d/dx at ({x},{y}): {dx} vs {fd_x}");
            assert!((dy - fd_y).abs() < 0.02, "d/dy at ({x},{y}): {dy} vs {fd_y}");
        }
    }
}
