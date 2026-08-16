//! What the cloud coverage curve actually does across a landscape.
//!
//! **This exists because a scene's rain assertions depend on it.** Once rain is
//! multiplied by cloud cover, `rain_overhang`'s `rate > 7.9` at a point is a
//! claim about this distribution — so the mapping from an authored `cover` to
//! "how much of the world is under full cloud" has to be a measured number
//! rather than a guess.

#[test]
fn coverage_reaches_one_everywhere_only_near_full_cover() {
    let field = loom_field::clouds();
    let mut rows = Vec::new();
    for cover in [0.5_f32, 0.7, 0.85, 0.95, 0.99, 1.0] {
        let mut params = loom_field::cloud_defaults();
        params.set("cloud_cover", cover);
        let (mut full, mut dry, mut n) = (0_u32, 0_u32, 0_u32);
        let mut min = 1.0_f32;
        // A few kilometres across, which is the scale a cloud mass works at.
        for i in 0_i16..60 {
            for j in 0_i16..60 {
                let x = (f32::from(i) - 30.0) * 120.0;
                let z = (f32::from(j) - 30.0) * 120.0;
                let v = field.body[0].eval_with([x, 0.0, z], 7.0, &params);
                assert!((0.0..=1.0).contains(&v), "coverage escaped [0,1]: {v}");
                if v > 0.99 {
                    full += 1;
                }
                if v < 0.01 {
                    dry += 1;
                }
                min = min.min(v);
                n += 1;
            }
        }
        let pct = |k: u32| 100.0 * f64::from(k) / f64::from(n);
        eprintln!(
            "cover {cover:>4}: {:>5.1}% solid  {:>5.1}% clear  min {min:.3}",
            pct(full),
            pct(dry)
        );
        rows.push((cover, pct(full), min));
    }

    // The property the rest of the system rests on: **cover 1.0 is solid
    // everywhere**, so a scene that wants uniform rain has a way to ask for it
    // and every assertion written before clouds existed still holds.
    let (_, solid_at_one, min_at_one) = *rows.last().expect("rows");
    assert!(
        (solid_at_one - 100.0).abs() < f64::EPSILON,
        "cover 1.0 left gaps: only {solid_at_one:.1}% solid, min {min_at_one}"
    );

    // And the opposite end genuinely varies, or the field is decorative.
    let (_, solid_at_half, _) = rows[0];
    assert!(
        solid_at_half < 60.0,
        "cover 0.5 is solid over {solid_at_half:.1}% of the world — no variation to drive rain with"
    );
}
