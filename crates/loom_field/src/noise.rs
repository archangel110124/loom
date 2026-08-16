//! The noise primitive. **Frozen, and treated as ABI.**
//!
//! S2 asked for the noise implementation to be pinned and its output treated
//! as an interface, because a crate version bump that changes noise output
//! silently invalidates every determinism hash — nothing errors, the sim just
//! stops matching yesterday's. So it is not a dependency. It lives here, in
//! this repository, and changing it is a deliberate edit that fails the hashes
//! loudly.
//!
//! # Why the lattice is integer
//!
//! Value noise hashes lattice corners and interpolates between them. The
//! obvious hash — `frac(sin(dot(p, k)) * 43758.5453)` — is the standard
//! shader one-liner and it is **unusable here**. `sin` of a large argument
//! depends on the argument reduction, which differs between libm and a GPU,
//! and `frac` of a large product amplifies a last-bit difference into a
//! completely different number. The CPU and GPU would disagree per corner, by
//! a lot, and no epsilon would save it.
//!
//! Integer arithmetic has no such freedom: `u32` multiply, xor and shift are
//! exact everywhere. `(h >> 8) as f32 * 2^-24` is exact too — a 24-bit integer
//! converts to `f32` without rounding, and scaling by a power of two is exact.
//! So **every lattice value is bit-identical on both sides**, and only the
//! interpolation between them is floating point, where a last-bit difference
//! stays a last-bit difference instead of exploding.
//!
//! # Written twice, and why that is not the thing S2 forbids
//!
//! A *field* is written once and generated twice, because a field is where the
//! two sides drift apart in ways nobody notices. This primitive is written in
//! Rust below and in Slang beside it, because an integer hash cannot be
//! expressed in the float expression tree. That is safe for a reason the
//! fields were not: the agreement test asserts them **exactly equal**, not
//! within an epsilon, so a divergence here is a hard failure rather than a
//! slow drift.

/// One round of a bit-mixing integer hash.
///
/// The constants are from the public-domain `lowbias32` search. What matters
/// is not which mixer this is but that it never changes: its output is in the
/// determinism hashes.
#[must_use]
pub fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// The value at one integer lattice corner, in `[0, 1)`.
#[must_use]
pub fn lattice(x: i32, y: i32, z: i32) -> f32 {
    #[allow(clippy::cast_sign_loss)]
    let h = hash(
        hash(hash(x as u32).wrapping_add(y as u32)).wrapping_add(z as u32),
    );
    // Top 24 bits: exactly representable in an f32, and scaling by 2^-24 is
    // exact. Using all 32 would round, and the rounding is a place the two
    // sides could differ.
    #[allow(clippy::cast_precision_loss)]
    {
        (h >> 8) as f32 * (1.0 / 16_777_216.0)
    }
}

/// Value noise at a point, in `[0, 1)`.
///
/// Smoothstep interpolation rather than linear: linear gives a field with
/// visible creases along the lattice, and a wind field made of creases reads
/// as a grid rather than as weather.
#[must_use]
pub fn value(p: [f32; 3]) -> f32 {
    let base = [p[0].floor(), p[1].floor(), p[2].floor()];
    let frac = [p[0] - base[0], p[1] - base[1], p[2] - base[2]];

    #[allow(clippy::cast_possible_truncation)]
    let cell = [base[0] as i32, base[1] as i32, base[2] as i32];

    // 3t² - 2t³, the Hermite smoothstep. C1 at the lattice, which is what
    // stops the creases.
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let w = [smooth(frac[0]), smooth(frac[1]), smooth(frac[2])];

    let lerp = |a: f32, b: f32, t: f32| (b - a).mul_add(t, a);
    let corner = |dx: i32, dy: i32, dz: i32| lattice(cell[0] + dx, cell[1] + dy, cell[2] + dz);

    let x00 = lerp(corner(0, 0, 0), corner(1, 0, 0), w[0]);
    let x10 = lerp(corner(0, 1, 0), corner(1, 1, 0), w[0]);
    let x01 = lerp(corner(0, 0, 1), corner(1, 0, 1), w[0]);
    let x11 = lerp(corner(0, 1, 1), corner(1, 1, 1), w[0]);
    let y0 = lerp(x00, x10, w[1]);
    let y1 = lerp(x01, x11, w[1]);
    lerp(y0, y1, w[2])
}

/// The Slang half, emitted verbatim into the generated shader.
///
/// **This and the Rust above are one thing written twice.** Keep them
/// adjacent, change them together, and rely on the agreement test — which
/// compares them for exact equality — to catch it when someone does not.
#[must_use]
pub fn slang() -> &'static str {
    r#"
// The noise primitive. Frozen: its output is in every determinism hash.
// The Rust half is `loom_field::noise`; they are one implementation written
// twice, and `field_agree` asserts they are exactly equal.
uint loom_hash(uint x) {
    x ^= x >> 16;
    x *= 0x7feb352du;
    x ^= x >> 15;
    x *= 0x846ca68bu;
    x ^= x >> 16;
    return x;
}

// Integer lattice, so every corner is bit-identical to the CPU's. Only the
// interpolation below is floating point.
float loom_lattice(int x, int y, int z) {
    uint h = loom_hash(loom_hash(loom_hash(uint(x)) + uint(y)) + uint(z));
    return float(h >> 8) * (1.0 / 16777216.0);
}

float loom_value_noise(float3 p) {
    float3 base = floor(p);
    float3 frac = p - base;
    int3 cell = int3(base);

    float3 w = frac * frac * (3.0 - 2.0 * frac);

    float x00 = lerp(loom_lattice(cell.x, cell.y, cell.z),
                     loom_lattice(cell.x + 1, cell.y, cell.z), w.x);
    float x10 = lerp(loom_lattice(cell.x, cell.y + 1, cell.z),
                     loom_lattice(cell.x + 1, cell.y + 1, cell.z), w.x);
    float x01 = lerp(loom_lattice(cell.x, cell.y, cell.z + 1),
                     loom_lattice(cell.x + 1, cell.y, cell.z + 1), w.x);
    float x11 = lerp(loom_lattice(cell.x, cell.y + 1, cell.z + 1),
                     loom_lattice(cell.x + 1, cell.y + 1, cell.z + 1), w.x);
    float y0 = lerp(x00, x10, w.y);
    float y1 = lerp(x01, x11, w.y);
    return lerp(y0, y1, w.z);
}
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The ABI, written down as numbers.** If this test fails, every
    /// determinism hash in the project has changed and the change was almost
    /// certainly not intended. Regenerate deliberately or revert.
    #[test]
    fn the_hash_is_frozen() {
        assert_eq!(hash(0), 0);
        assert_eq!(hash(1), 0x6889_90c0);
        assert_eq!(hash(0xffff_ffff), 0x6768_824a);
        // And the lattice value derived from it, which is what actually
        // reaches a field — the shift and the scale are part of the ABI too.
        assert_eq!(lattice(0, 0, 0).to_bits(), 0.0_f32.to_bits());
        assert_eq!(lattice(1, 2, 3), 0.248_284_22);
    }

    /// Values stay in range, including at negative coordinates — a wind field
    /// covers the whole world, not the positive octant.
    #[test]
    fn noise_stays_inside_its_range() {
        for i in 0..2000 {
            #[allow(clippy::cast_precision_loss)]
            let f = i as f32;
            let p = [f * 0.37 - 300.0, f * -0.11 + 50.0, f * 0.53 - 120.0];
            let n = value(p);
            assert!((0.0..1.0).contains(&n), "noise({p:?}) = {n}");
        }
    }

    /// Smoothstep interpolation means no jump across a lattice boundary. A
    /// crease there is the artifact linear interpolation produces, and it
    /// reads as a grid laid over the world.
    #[test]
    fn there_is_no_seam_at_a_lattice_boundary() {
        let before = value([2.0 - 1e-4, 0.3, 0.7]);
        let at = value([2.0, 0.3, 0.7]);
        let after = value([2.0 + 1e-4, 0.3, 0.7]);

        assert!((at - before).abs() < 1e-3, "{before} -> {at}");
        assert!((after - at).abs() < 1e-3, "{at} -> {after}");
    }

    /// The lattice actually varies. A hash that collapsed would make every
    /// corner equal and the noise a constant — which several of the tests
    /// above would happily pass.
    #[test]
    fn neighbouring_lattice_corners_differ() {
        let mut seen = std::collections::BTreeSet::new();
        for x in 0..8 {
            for y in 0..8 {
                for z in 0..8 {
                    seen.insert(lattice(x, y, z).to_bits());
                }
            }
        }
        assert_eq!(seen.len(), 512, "lattice values collide");
    }

    /// The two halves are written twice, so at minimum they must *look* the
    /// same — same constants, same shifts. Exact numeric agreement is the
    /// compute test in `loom_render`; this catches a typo before a GPU is
    /// involved.
    #[test]
    fn the_slang_half_uses_the_same_constants() {
        let slang = slang();
        for needle in ["0x7feb352du", "0x846ca68bu", "16777216.0", "3.0 - 2.0"] {
            assert!(slang.contains(needle), "the Slang half is missing `{needle}`");
        }
    }
}
