//! **W2's exit criterion: the Rust surface and the Slang surface are the same
//! surface.**
//!
//! The water doc calls a silent divergence between these two "a boat floating
//! visibly above or below the surface, with no error anywhere", and it is right
//! that discipline is not enough to prevent it. `loom_field`'s answer — one
//! expression tree, both sides generated from it — cannot apply here, because
//! that tree is a scalar-field language with no loop and no vector result. So
//! the water surface follows the `loom_field::noise` precedent instead: written
//! twice, deliberately, adjacent in one file, and **measured**.
//!
//! # How the Slang half is executed without a GPU
//!
//! `slangc` compiles to C++ as well as to SPIR-V. This test writes the emitted
//! Slang plus a generated kernel that walks a fixed sample set, compiles it with
//! `slangc -target cpp`, links it against a six-line harness, runs it, and
//! compares every number against [`loom_water::sample_water`].
//!
//! **What that proves and what it does not.** It proves the two implement the
//! same formula — the failure that actually happens, where an edit lands on one
//! side only. It does not measure GPU floating point: a GPU `sin` is not libm's
//! and a driver may contract a multiply-add, which is what `loom_render`'s
//! `field_agree` dispatch exists to measure for the wind field. The equivalent
//! dispatch for water belongs beside that one, and it needs the water mesh's
//! shader plumbing first; this is the half that can be had today, cheaply, and
//! it is the half that catches a typo.
//!
//! # W6 rides on the same harness
//!
//! Depth has the identical hazard one layer down: the buoyancy solver subtracts
//! a terrain height the CPU looked up, and the shader draws a shoreline from a
//! terrain height the GPU looked up. Those are two lookups into **one baked
//! grid**, and this test now runs the second one — `loom_voxel::heightfield`'s
//! Slang half — beside the water and compares both the ground height and the
//! depth that comes out of it. The measured difference is again exactly zero;
//! it was 4.8e-7 until the Rust lerp stopped using `mul_add`, which is the kind
//! of thing only a numeric comparison finds.
//!
//! Skips honestly, like every other tool-dependent check here, when `slangc` or
//! a C++ compiler is missing.

use std::path::PathBuf;
use std::process::Command;

use loom_scene::components::{GerstnerWave, WaterBody, WaveSet};
use loom_voxel::heightfield::HeightField;

/// Absolute agreement threshold, on a surface whose values reach a few metres.
///
/// **Measured, not chosen.** Both sides are IEEE `f32` evaluating the same
/// operations in the same order, so what is left is `sinf`/`cosf`/`sqrtf`
/// rounding — the run prints the worst difference it saw, and a threshold loose
/// enough to hide a real formula change would make this test decorative.
///
/// The worst difference measured is **exactly 0.0**: same operations, same
/// order, same libm. `1e-4` was a thousand times looser than that, and a 0.1%
/// mutation of one Slang term cleared it by only 1.6× — so a 0.05% divergence
/// would have passed. This is still four orders of magnitude of headroom over
/// the observed value; raise it only against a printed number.
const EPSILON: f32 = 1e-6;

/// The river current handed to both halves at every sample.
///
/// **Non-zero on purpose.** A flow of zero would let a side that dropped the
/// argument entirely agree with one that kept it, which is exactly the
/// divergence the new parameter could introduce. The two horizontal components
/// differ so a swap shows as a failure rather than as luck.
const FLOW: [f32; 3] = [0.7, 0.0, -0.4];

/// The interactive wavelet handed to both halves at every sample —
/// `(height, dh/dx, dh/dz)`.
///
/// **Non-zero for exactly the reason `FLOW` is**, and it matters more here:
/// this term reaches a *force*, and the CPU grid is the authoritative copy.
/// All three components differ so a swapped slope shows as a failure rather
/// than as luck, and the height is the size of a real wake rather than a
/// token.
const WAVELET: [f32; 3] = [0.07, 0.031, -0.052];

/// How many `(x, z, t)` points both sides evaluate.
const SAMPLES: usize = 512;

/// The sea both sides are asked about.
///
/// Five waves at spread-out wavelengths and directions, all inside the
/// steepness limit, plus one degenerate wave — a zero direction — because the
/// `continue` that skips it is a branch the two halves have to agree about, and
/// a branch is a much easier thing to get wrong than an arithmetic term.
fn sea() -> WaterBody {
    let wave = |wavelength, amplitude, steepness, direction, speed_scale| GerstnerWave {
        wavelength,
        amplitude,
        steepness,
        direction,
        speed_scale,
    };
    WaterBody {
        surface_height: 1.75,
        waves: WaveSet {
            waves: vec![
                wave(31.0, 0.90, 0.55, [1.0, 0.15], 1.0),
                wave(17.0, 0.45, 0.70, [0.85, -0.5], 1.1),
                wave(8.5, 0.22, 0.80, [0.4, 0.9], 0.9),
                wave(3.25, 0.06, 0.95, [-0.6, 0.8], 1.3),
                wave(12.0, 0.30, 0.60, [0.0, 0.0], 1.0),
            ],
            attenuation_depth: 8.0,
            max_height: 3.0,
        },
        ..WaterBody::default()
    }
}

/// The bed both sides are asked about.
///
/// **Built by hand rather than marched out of a voxel volume**, for the same
/// reason the sample set is written into the shader as literals: what is under
/// test is that the two *lookups* agree, and a bake would only add a step both
/// sides share. It carries the three things that could be got differently wrong
/// — a slope, a hole with no ground in it at all
/// (`loom_voxel::heightfield::NO_GROUND`), and a fractional grid position that
/// lands between four different corners.
fn bed() -> HeightField {
    let side = 24;
    let mut height = Vec::with_capacity(side * side);
    for j in 0..side {
        for i in 0..side {
            let (x, z) = (i as f32, j as f32);
            // A beach running up the +X axis, with a bump in it so the
            // interpolation has something to interpolate.
            let h = 0.35f32.mul_add(x, -6.0) + (0.4 * z).sin() * 0.5;
            // A hole punched through the middle: no ground at all, which must
            // survive being blended with the real heights around it.
            height.push(if (10..13).contains(&i) && (6..9).contains(&j) {
                loom_voxel::heightfield::NO_GROUND
            } else {
                h
            });
        }
    }
    HeightField { origin: [-6.0, -9.0], spacing: 0.75, side, height }
}

/// The sample set, generated once in Rust and written into the shader as
/// literals.
///
/// **Not derived from the invocation index on each side.** Deriving them twice
/// would reintroduce the exact hazard this test exists to catch, inside the test
/// meant to catch it. Scattered by an integer hash rather than walked on a grid,
/// because a regular grid can land every sample on the same phase of a sinusoid
/// and agree for the wrong reason.
fn samples() -> Vec<[f32; 3]> {
    (0..SAMPLES)
        .map(|i| {
            let h = |k: u32| {
                let mut x = (i as u32).wrapping_mul(0x9E37_79B9).wrapping_add(k);
                x ^= x >> 16;
                x = x.wrapping_mul(0x7FEB_352D);
                x ^= x >> 15;
                f32::from(x as u16) / f32::from(u16::MAX)
            };
            // **Spread wider than the bed on purpose.** The grid covers x in
            // [−6, 12) and z in [−9, 9), so roughly half of these land inside
            // it and the rest fall off the edge — where both sides have to
            // return the sentinel rather than the nearest corner, which is a
            // branch and therefore the easiest thing here to get differently
            // wrong.
            [
                h(1).mul_add(40.0, -14.0), // x
                h(2).mul_add(40.0, -20.0), // z
                h(3) * 120.0,              // t, seconds
            ]
        })
        .collect()
}

#[test]
fn the_rust_and_the_slang_compute_the_same_surface() {
    let Some(slangc) = tool("slangc") else {
        eprintln!("skipping: slangc is not on PATH");
        return;
    };
    let Some(cxx) = tool("c++").or_else(|| tool("g++")).or_else(|| tool("clang++")) else {
        eprintln!("skipping: no C++ compiler");
        return;
    };

    let body = sea();
    let bed = bed();
    let samples = samples();
    let dir = std::env::temp_dir().join(format!("loom_water_agree_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");

    let shader = dir.join("water.slang");
    std::fs::write(&shader, kernel(&body, &bed, &samples)).expect("write shader");
    std::fs::write(dir.join("harness.cpp"), HARNESS).expect("write harness");

    let kernel_cpp = dir.join("kernel.cpp");
    run(
        Command::new(slangc)
            .arg(&shader)
            .args(["-target", "cpp", "-entry", "computeMain", "-stage", "compute"])
            .arg("-o")
            .arg(&kernel_cpp),
        "slangc",
    );

    let binary = dir.join("harness");
    run(
        Command::new(cxx)
            // -O0 and no fast-math: the point is IEEE arithmetic in source
            // order, which is what the Rust half also does.
            .args(["-O0", "-w", "-std=c++17"])
            .arg(format!("-I{}", dir.display()))
            .arg(dir.join("harness.cpp"))
            .arg("-o")
            .arg(&binary),
        "c++",
    );

    let output = Command::new(&binary).output().expect("run the compiled kernel");
    assert!(output.status.success(), "the kernel exited {}", output.status);
    let text = String::from_utf8(output.stdout).expect("kernel output is utf-8");

    let rows: Vec<Vec<f32>> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split_whitespace().map(|n| n.parse().expect("a number")).collect())
        .collect();
    assert_eq!(rows.len(), samples.len(), "the kernel printed {} rows", rows.len());

    // **Otherwise this test agrees for free.** If the kernel wrote zeros, or
    // the sea evaluated flat, every difference below would be zero and the
    // assertion would pass while proving nothing.
    let largest = rows.iter().flatten().fold(0.0_f32, |a, b| a.max(b.abs()));
    assert!(largest > 1.0, "the Slang surface is flat — largest value {largest}");

    let mut worst = 0.0_f32;
    let mut worst_at = (0, 0);
    // **The depth the physics uses is the depth the shader draws.** The ground
    // height is looked up on each side by its own implementation — Rust's
    // `HeightField::at` here, the generated `loom_ground_height` there — and
    // then fed to the surface, so a divergence in the lookup shows up in every
    // field below rather than only in the last column.
    let mut inside = 0_usize;
    // Reported below rather than hard-coded. The count used to be a literal
    // `12` in the summary line, which is the line a human reads to believe the
    // test ran — so a field added to `LoomWaterSample` and to `expected` but
    // forgotten in the `printf` would have been announced as twelve values
    // compared while thirteen existed.
    let mut values = 0_usize;
    for (i, (sample, row)) in samples.iter().zip(&rows).enumerate() {
        let ground = bed.at(sample[0], sample[1]);
        if HeightField::has_ground(ground) {
            inside += 1;
        }
        let cpu =
            loom_water::sample_water(&body, None, [sample[0], sample[1]], sample[2], ground, FLOW, WAVELET);
        let expected = [
            cpu.height,
            cpu.normal[0],
            cpu.normal[1],
            cpu.normal[2],
            cpu.displacement[0],
            cpu.displacement[1],
            cpu.displacement[2],
            cpu.velocity[0],
            cpu.velocity[1],
            cpu.velocity[2],
            cpu.depth,
            cpu.fold,
            // The breaking criterion and its axis. A divergence here is foam
            // painted somewhere the CPU's `water@x,z.foam` does not see it.
            cpu.mu_max,
            cpu.break_dir[0],
            cpu.break_dir[1],
            ground,
        ];
        assert_eq!(row.len(), expected.len(), "row {i} has {} values", row.len());
        values = expected.len();
        for (field, (rust, slang)) in expected.iter().zip(row).enumerate() {
            let delta = (rust - slang).abs();
            assert!(delta.is_finite(), "sample {i} field {field}: {rust} vs {slang}");
            if delta > worst {
                worst = delta;
                worst_at = (i, field);
            }
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
    eprintln!(
        "water agreement: worst absolute difference {worst:e} over {} samples \
         ({} values each), {inside} of them over real ground",
        samples.len(),
        values
    );
    // Otherwise the depth half of this test proves nothing: a sample set that
    // missed the bed entirely would compare two sentinels and agree.
    assert!(
        inside > samples.len() / 8,
        "only {inside} of {} samples landed on the bed",
        samples.len()
    );
    assert!(
        worst < EPSILON,
        "the Rust and the Slang disagree by {worst} at sample {} field {} — \
         the two halves of `loom_water` have diverged",
        worst_at.0,
        worst_at.1
    );
}

/// **The twin has to compile for the target it actually ships to.**
///
/// The agreement test above runs it through Slang's C++ backend, which accepts
/// things SPIR-V does not; a twin that only builds on the CPU is a twin that
/// breaks the renderer's build the day the water mesh includes it. Same flags
/// as `loom_render/build.rs`, so this fails here rather than there.
#[test]
fn the_slang_half_compiles_for_the_gpu_too() {
    let Some(slangc) = tool("slangc") else {
        eprintln!("skipping: slangc is not on PATH");
        return;
    };

    let dir = std::env::temp_dir().join(format!("loom_water_spirv_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let shader = dir.join("water_probe.slang");
    // A minimal consumer, because a function nobody calls can be dropped before
    // anything checks it.
    let source = format!(
        "{}\nRWStructuredBuffer<float> loom_water_probe;\n\
         // The tile pointer arrives through a push constant because that is\
         // where it comes from in the real shader, `push.environment[0]`. A\
         // pointer to a function-local array compiles for the C++ target and\
         // is a SPIR-V type error, which is what this test exists to catch.\n\
         struct OceanProbePush {{ float* tiles; }};\n\
         [vk::push_constant] OceanProbePush probePush;\n\n\
         [shader(\"compute\")]\n[numthreads(1,1,1)]\n\
         void probeMain(uint3 tid : SV_DispatchThreadID)\n{{\n\
         \x20   LoomWaveSet set;\n    set.count = 1;\n\
         \x20   set.waves[0].direction = float2(1.0, 0.0);\n\
         \x20   set.waves[0].wavelength = 10.0;\n\
         \x20   set.waves[0].amplitude = 0.5;\n\
         \x20   set.waves[0].steepness = 0.5;\n\
         \x20   set.waves[0].speed_scale = 1.0;\n\
         \x20   LoomWaterSample s = loom_sample_water(set, 0.0, -4.0, float2(1.0, 2.0), 3.0, float3(0.4, 0.0, -0.2), float3(0.05, 0.02, -0.03));\n\
         \x20   LoomOcean sea;\n    sea.patch = float4(4.0, 0.0, 0.0, 0.0);\n\
         \x20   sea.n = 1;\n    sea.count = 1;\n    sea.tiles = probePush.tiles;\n\
         \x20   LoomOceanSample c = loom_ocean_at(sea, float2(1.0, 2.0), float4(1.0, 1.0, 1.0, 1.0));\n\
         \x20   LoomWaterSample f = loom_sample_cascade(c, float3(0.0, 0.0, 0.0), 0.0, -4.0, float3(0.4, 0.0, -0.2), float3(0.05, 0.02, -0.03));\n\
         \x20   loom_water_probe[0] = s.height + s.normal.y + s.displacement.x\n\
         \x20       + s.velocity.z + s.depth + s.fold + s.mu_max + s.break_dir.x\n\
         \x20       + f.height + f.normal.y + f.displacement.x + f.fold + f.mu_max;\n}}\n",
        loom_water::slang()
    );
    std::fs::write(&shader, source).expect("write shader");

    let spv = dir.join("water_probe.spv");
    run(
        Command::new(slangc)
            .arg(&shader)
            .args(["-target", "spirv", "-profile", "spirv_1_5"])
            .args(["-matrix-layout-column-major", "-fvk-use-entrypoint-name"])
            .arg("-o")
            .arg(&spv),
        "slangc -target spirv",
    );
    // A second opinion where it is installed, exactly as the build script does.
    if let Some(val) = tool("spirv-val") {
        run(Command::new(val).arg(&spv), "spirv-val");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Absolute agreement threshold for the nappe, on quantities reaching tens of
/// metres per second.
///
/// **Looser than [`EPSILON`] on purpose, and it is S2's own threshold.** The
/// surface above is add-multiply-`sin` in one order on both sides and lands on
/// exactly 0.0; the nappe raises to a fractional power twice, and `powf` on the
/// Rust side is not the same code as `pow` in Slang's C++ backend. The run
/// prints the worst difference it saw — read that, not this.
const NAPPE_EPSILON: f32 = 1.0e-3;

/// **The nappe is force-capable, so its twin gets measured like the surface.**
///
/// A fall pushes what stands under it, which puts `loom_water::nappe` on the
/// same footing as `sample_water`: the number a force would read and the number
/// the vertex shader draws must be one number. The mechanism is the same one
/// the surface uses — written twice, adjacent, and compared here rather than
/// trusted.
#[test]
fn the_rust_and_the_slang_compute_the_same_nappe() {
    let Some(slangc) = tool("slangc") else {
        eprintln!("skipping: slangc is not on PATH");
        return;
    };
    let Some(cxx) = tool("c++").or_else(|| tool("g++")).or_else(|| tool("clang++")) else {
        eprintln!("skipping: no C++ compiler");
        return;
    };

    // The same integer hash the surface's sample set uses, spread over the
    // authored ranges of `Cascade`: discharge across four orders of magnitude,
    // both δ's across the literature's bands, and a fall depth reaching past
    // any `Z_b` those produce so the break-up branch is exercised on both
    // sides.
    let cases: Vec<[f32; 4]> = (0..SAMPLES)
        .map(|i| {
            let h = |k: u32| {
                let mut x = (i as u32).wrapping_mul(0x9E37_79B9).wrapping_add(k);
                x ^= x >> 16;
                x = x.wrapping_mul(0x7FEB_352D);
                x ^= x >> 15;
                f32::from(x as u16) / f32::from(u16::MAX)
            };
            [
                // Log-spread, because a linear sweep over [0.01, 20] puts
                // nineteen twentieths of the samples on canyon falls and never
                // visits the spout the discrimination table turns on.
                (h(1).mul_add(7.6, -4.6)).exp(),
                h(2).mul_add(0.025, 0.015), // spread
                h(3).mul_add(0.005, 0.005), // breakup
                h(4) * 30.0,                // Z, metres below the lip
            ]
        })
        .collect();

    let dir = std::env::temp_dir().join(format!("loom_nappe_agree_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let shader = dir.join("nappe.slang");

    let mut source = String::from(loom_water::nappe::slang());
    source.push_str(
        "\nvoid emit(float q, float dout, float din, float z)\n{\n\
         \x20   LoomNappe n = loom_nappe_brink(q, dout, din);\n\
         \x20   LoomNappeAt s = loom_nappe_at(n, z);\n\
         \x20   printf(\"%.9g %.9g %.9g %.9g %.9g %.9g %.9g\\n\", n.brink_depth,\n\
         \x20       n.exit_speed, loom_nappe_break_length(n), s.throwX, s.speed,\n\
         \x20       s.water_thickness, s.aeration);\n}\n\n\
         [shader(\"compute\")]\n[numthreads(1,1,1)]\n\
         void computeMain(uint3 tid : SV_DispatchThreadID)\n{\n",
    );
    for [q, dout, din, z] in &cases {
        source.push_str(&format!("    emit({q:?}, {dout:?}, {din:?}, {z:?});\n"));
    }
    source.push_str("}\n");
    std::fs::write(&shader, source).expect("write shader");
    std::fs::write(dir.join("harness.cpp"), HARNESS).expect("write harness");

    let kernel_cpp = dir.join("kernel.cpp");
    run(
        Command::new(slangc)
            .arg(&shader)
            .args(["-target", "cpp", "-entry", "computeMain", "-stage", "compute"])
            .arg("-o")
            .arg(&kernel_cpp),
        "slangc",
    );
    let binary = dir.join("harness");
    run(
        Command::new(cxx)
            .args(["-O0", "-w", "-std=c++17"])
            .arg(format!("-I{}", dir.display()))
            .arg(dir.join("harness.cpp"))
            .arg("-o")
            .arg(&binary),
        "c++",
    );

    let output = Command::new(&binary).output().expect("run the compiled kernel");
    assert!(output.status.success(), "the kernel exited {}", output.status);
    let text = String::from_utf8(output.stdout).expect("kernel output is utf-8");
    let rows: Vec<Vec<f32>> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split_whitespace().map(|n| n.parse().expect("a number")).collect())
        .collect();
    assert_eq!(rows.len(), cases.len(), "the kernel printed {} rows", rows.len());

    let mut worst = 0.0_f32;
    let mut worst_at = (0, 0);
    // Otherwise this agrees for free, exactly as the surface's guard says.
    let largest = rows.iter().flatten().fold(0.0_f32, |a, b| a.max(b.abs()));
    assert!(largest > 1.0, "the Slang nappe is degenerate — largest value {largest}");

    for (i, ([q, dout, din, z], row)) in cases.iter().zip(&rows).enumerate() {
        let n = loom_water::nappe::brink(*q, *dout, *din);
        let s = n.at(*z);
        let expected = [
            n.brink_depth,
            n.exit_speed,
            n.break_length(),
            s.throw,
            s.speed,
            s.water_thickness,
            s.aeration,
        ];
        assert_eq!(row.len(), expected.len(), "row {i} has {} values", row.len());
        for (field, (rust, slang)) in expected.iter().zip(row).enumerate() {
            let delta = (rust - slang).abs();
            assert!(delta.is_finite(), "sample {i} field {field}: {rust} vs {slang}");
            if delta > worst {
                worst = delta;
                worst_at = (i, field);
            }
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
    eprintln!(
        "nappe agreement: worst absolute difference {worst:e} over {} samples (7 values each)",
        cases.len()
    );
    assert!(
        worst < NAPPE_EPSILON,
        "the Rust and the Slang disagree by {worst} at sample {} field {} — \
         the two halves of `loom_water::nappe` have diverged",
        worst_at.0,
        worst_at.1
    );
}

/// **The event pool is on the force path, so its twin is measured too.**
///
/// A wavelet raises the surface a pontoon reads (`loom_water::sample_water`'s
/// sixth argument) *and* the surface the water mesh is drawn at. If the two
/// halves disagree, a barrel floats visibly above the ring it is riding — the
/// exact failure this file exists to prevent, one layer further in.
///
/// Five values a sample, because the orbital velocity is what reaches drag and
/// a side that dropped it would otherwise agree for free.
#[test]
fn the_rust_and_the_slang_compute_the_same_wavelets() {
    let Some(slangc) = tool("slangc") else {
        eprintln!("skipping: slangc is not on PATH");
        return;
    };
    let Some(cxx) = tool("c++").or_else(|| tool("g++")).or_else(|| tool("clang++")) else {
        eprintln!("skipping: no C++ compiler");
        return;
    };

    // Four events with different volumes, radii and birth times, so both the
    // source cutoff and the Nyquist fade are exercised at both ends — and one
    // in the future, whose `tau <= 0` branch is a branch the two halves have to
    // agree about.
    let events = [
        loom_water::wavelet::Event::new([0.0, 0.0], 0.0, 0.40, 0.50),
        loom_water::wavelet::Event::new([3.5, -1.25], 0.5, 0.02, 0.15),
        loom_water::wavelet::Event::new([-6.0, 4.0], 1.75, 2.60, 1.20),
        loom_water::wavelet::Event::new([1.0, 1.0], 40.0, 0.30, 0.40),
    ];
    // The same integer hash the surface's sample set uses, over a patch wide
    // enough to reach past every packet's `r_max` and close enough to fall
    // inside every `r_min`.
    let samples: Vec<[f32; 3]> = (0..SAMPLES)
        .map(|i| {
            let h = |k: u32| {
                let mut x = (i as u32).wrapping_mul(0x9E37_79B9).wrapping_add(k);
                x ^= x >> 16;
                x = x.wrapping_mul(0x7FEB_352D);
                x ^= x >> 15;
                f32::from(x as u16) / f32::from(u16::MAX)
            };
            [h(1).mul_add(40.0, -20.0), h(2).mul_add(40.0, -20.0), h(3) * 12.0]
        })
        .collect();

    let dir = std::env::temp_dir().join(format!("loom_wavelet_agree_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let shader = dir.join("wavelet.slang");

    let mut source = String::from(loom_water::wavelet::slang());
    source.push_str(
        "\nvoid emit(LoomWaveletPool pool, float x, float z, float t)\n{\n\
         \x20   LoomWavelet w = loom_wavelets_at(pool, float2(x, z), t);\n\
         \x20   printf(\"%.9g %.9g %.9g %.9g %.9g\\n\", w.height, w.slope.x, w.slope.y,\n\
         \x20       w.velocity.x, w.velocity.y);\n}\n\n\
         [shader(\"compute\")]\n[numthreads(1,1,1)]\n\
         void computeMain(uint3 tid : SV_DispatchThreadID)\n{\n",
    );
    // The events as a local array the pool points at — the same shape the
    // environment buffer hands the shader, so the struct's field order is under
    // test as well as its arithmetic.
    source.push_str(&format!("    LoomWaveletEvent evs[{}];\n", events.len()));
    for (i, e) in events.iter().enumerate() {
        source.push_str(&format!(
            "    evs[{i}].at = float2({:?}, {:?});\n\
             \x20   evs[{i}].t0 = {:?};\n    evs[{i}].volume = {:?};\n\
             \x20   evs[{i}].sigma = {:?};\n    evs[{i}].pad0 = 0.0;\n\
             \x20   evs[{i}].pad1 = 0.0;\n    evs[{i}].pad2 = 0.0;\n",
            e.at[0], e.at[1], e.t0, e.volume, e.sigma,
        ));
    }
    source.push_str(&format!(
        "    LoomWaveletPool pool;\n    pool.events = &evs[0];\n    pool.count = {};\n",
        events.len()
    ));
    for [x, z, t] in &samples {
        source.push_str(&format!("    emit(pool, {x:?}, {z:?}, {t:?});\n"));
    }
    source.push_str("}\n");
    std::fs::write(&shader, source).expect("write shader");
    std::fs::write(dir.join("harness.cpp"), HARNESS).expect("write harness");

    let kernel_cpp = dir.join("kernel.cpp");
    run(
        Command::new(slangc)
            .arg(&shader)
            .args(["-target", "cpp", "-entry", "computeMain", "-stage", "compute"])
            .arg("-o")
            .arg(&kernel_cpp),
        "slangc",
    );
    let binary = dir.join("harness");
    run(
        Command::new(cxx)
            .args(["-O0", "-w", "-std=c++17"])
            .arg(format!("-I{}", dir.display()))
            .arg(dir.join("harness.cpp"))
            .arg("-o")
            .arg(&binary),
        "c++",
    );
    let output = Command::new(&binary).output().expect("run the compiled kernel");
    assert!(output.status.success(), "the kernel exited {}", output.status);
    let text = String::from_utf8(output.stdout).expect("kernel output is utf-8");
    let rows: Vec<Vec<f32>> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split_whitespace().map(|n| n.parse().expect("a number")).collect())
        .collect();
    assert_eq!(rows.len(), samples.len(), "the kernel printed {} rows", rows.len());

    let mut field = loom_water::wavelet::WaveletField::new();
    for e in &events {
        field.emit(e.at, e.t0, e.volume, e.sigma);
    }
    // Otherwise both sides could agree on zero everywhere, which is what a
    // sample set entirely outside every packet would prove.
    let live = samples
        .iter()
        .filter(|[x, z, t]| field.at(*x, *z, *t).height.abs() > 1e-4)
        .count();
    assert!(live > SAMPLES / 20, "only {live} of {SAMPLES} samples are inside a packet");

    let mut worst = 0.0_f32;
    for (i, ([x, z, t], row)) in samples.iter().zip(&rows).enumerate() {
        let w = field.at(*x, *z, *t);
        let expected = [w.height, w.slope[0], w.slope[1], w.velocity[0], w.velocity[1]];
        assert_eq!(row.len(), expected.len(), "row {i} has {} values", row.len());
        for (field_index, (rust, slang)) in expected.iter().zip(row).enumerate() {
            let delta = (rust - slang).abs();
            assert!(delta.is_finite(), "sample {i} field {field_index}: {rust} vs {slang}");
            worst = worst.max(delta);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!(
        "wavelet agreement: worst absolute difference {worst:e} over {} samples \
         (5 values each, {live} inside a packet)",
        samples.len()
    );
    assert!(
        worst < EPSILON,
        "the Rust and the Slang disagree by {worst} — the two halves of \
         `loom_water::wavelet` have diverged"
    );
}

/// The Slang source: both twins, plus a kernel that prints the sample set.
fn kernel(body: &WaterBody, bed: &HeightField, samples: &[[f32; 3]]) -> String {
    // The height field first: the water half does not use it, but the kernel
    // below does, and Slang wants a function declared before it is called.
    let mut out = String::from(loom_voxel::heightfield::slang());
    out.push_str(loom_water::slang());
    out.push_str(
        "\n[shader(\"compute\")]\n[numthreads(1,1,1)]\nvoid computeMain(uint3 tid : SV_DispatchThreadID)\n{\n    LoomWaveSet set;\n",
    );
    // `%.9g` round-trips an f32 exactly, and `{v:?}` on the Rust side emits the
    // shortest decimal that parses back to the same f32 — so the wave table and
    // the sample set cross into Slang without a rounding step of their own.
    out.push_str(&format!("    set.count = {};\n", body.waves.waves.len()));
    out.push_str(&format!(
        "    set.attenuation_depth = {:?};\n",
        body.waves.attenuation_depth
    ));
    for (i, wave) in body.waves.waves.iter().enumerate() {
        out.push_str(&format!(
            "    set.waves[{i}].direction = float2({:?}, {:?});\n\
             \x20   set.waves[{i}].wavelength = {:?};\n\
             \x20   set.waves[{i}].amplitude = {:?};\n\
             \x20   set.waves[{i}].steepness = {:?};\n\
             \x20   set.waves[{i}].speed_scale = {:?};\n",
            wave.direction[0],
            wave.direction[1],
            wave.wavelength,
            wave.amplitude,
            wave.steepness,
            wave.speed_scale,
        ));
    }
    // The bed, as a local array the field points at. **This is the part that
    // makes the test cover W6's real question** — the shader reads its heights
    // through a pointer over buffer device address, and the only thing that
    // differs here is where the pointer comes from.
    out.push_str(&format!(
        "    float heights[{}];\n",
        bed.height.len()
    ));
    for (i, h) in bed.height.iter().enumerate() {
        out.push_str(&format!("    heights[{i}] = {h:?};\n"));
    }
    out.push_str(&format!(
        "    LoomHeightField bed;\n    bed.origin = float2({:?}, {:?});\n\
         \x20   bed.spacing = {:?};\n    bed.side = {};\n    bed.height = &heights[0];\n",
        bed.origin[0], bed.origin[1], bed.spacing, bed.side,
    ));

    for [x, z, t] in samples {
        out.push_str(&format!(
            "    emit(set, bed, {:?}, float2({x:?}, {z:?}), {t:?}, float3({:?}, {:?}, {:?}), \
             float3({:?}, {:?}, {:?}));\n",
            body.surface_height,
            FLOW[0],
            FLOW[1],
            FLOW[2],
            WAVELET[0],
            WAVELET[1],
            WAVELET[2],
        ));
    }
    out.push_str("}\n");

    // Declared above `computeMain` in the emitted text, since Slang wants it
    // before use.
    let emit = "\nvoid emit(LoomWaveSet set, LoomHeightField bed, float surface_height, float2 xz, float t, float3 flow, float3 wavelet)\n\
                {\n    float ground_height = loom_ground_height(bed, xz);\n\
                \x20   LoomWaterSample s = loom_sample_water(set, surface_height, ground_height, xz, t, flow, wavelet);\n\
                \x20   printf(\"%.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g\\n\",\n\
                \x20       s.height, s.normal.x, s.normal.y, s.normal.z,\n\
                \x20       s.displacement.x, s.displacement.y, s.displacement.z,\n\
                \x20       s.velocity.x, s.velocity.y, s.velocity.z, s.depth, s.fold,\n\
                \x20       s.mu_max, s.break_dir.x, s.break_dir.y, ground_height);\n}\n";
    let split = out.find("\n[shader(\"compute\")]").expect("the kernel was just written");
    out.insert_str(split, emit);
    out
}

/// Six lines of C++ to dispatch one thread of the compiled kernel.
const HARNESS: &str = "\
#include <cstdio>
#include \"kernel.cpp\"

int main() {
    ComputeVaryingInput vi = {};
    vi.startGroupID = uint3{0, 0, 0};
    vi.endGroupID = uint3{1, 1, 1};
    computeMain(&vi, nullptr, nullptr);
    return 0;
}
";

/// A tool's path, or `None` if it is not installed.
fn tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Run a build step, failing loudly with its output. **Never swallow a compile
/// error** (never-do #9): a test that silently skipped a failed shader would
/// pass forever on a shader that no longer builds.
fn run(command: &mut Command, what: &str) {
    let output = command.output().unwrap_or_else(|e| panic!("cannot run {what}: {e}"));
    assert!(
        output.status.success(),
        "{what} failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// **The FFT cascade is on the force path too, so its twin is measured like the
/// surface's** — ADR 0076.
///
/// This is the model where a silent divergence is likeliest, because neither
/// side has a closed form to read the answer off: the CPU differences a tile and
/// the GPU interpolates one, and "the boat floats above the water" would be the
/// only symptom. What is compared is every field of `LoomWaterSample` that a
/// `spectrum` body produces, at 512 scattered points of a two-cascade ocean.
///
/// The tiles cross into Slang as literals for the same reason the bed does: what
/// is under test is the *lookup* — the patch wrap, the bilinear tap, the seam,
/// and the cascade sum's order — and a second bake would only add a step both
/// sides share.
#[test]
fn the_rust_and_the_slang_sample_the_same_cascade() {
    let Some(slangc) = tool("slangc") else {
        eprintln!("skipping: slangc is not on PATH");
        return;
    };
    let Some(cxx) = tool("c++").or_else(|| tool("g++")).or_else(|| tool("clang++")) else {
        eprintln!("skipping: no C++ compiler");
        return;
    };

    // Two cascades, because summing them in index order is part of the surface's
    // definition and one cascade cannot check it. Sixteen cells a side so the
    // tiles fit in a source file: 2 x 8 x 256 floats.
    let cut = loom_water::ocean::nyquist(256.0, 16);
    let mut sea = loom_water::ocean::Ocean::new(
        &[
            loom_water::ocean::Cascade { patch: 256.0, n: 16, band: [0.0, cut] },
            loom_water::ocean::Cascade { patch: 32.0, n: 16, band: [cut, f32::INFINITY] },
        ],
        18.0,
        440_000.0,
        [0.9, 0.3],
        0x0C_EA_11,
    );
    // One instant, and every sample below is asked for it: the tiles carry no
    // clock and `sample_water` debug-asserts that the two agree.
    const WHEN: f32 = 7.5;
    sea.evolve(WHEN);
    let render = sea.render_tiles();
    assert_eq!(render.n, 16);
    assert_eq!(render.tiles.len(), 2 * loom_water::ocean::RENDER_TILES_PER_CASCADE * 16 * 16);

    let body = WaterBody {
        surface_height: 1.75,
        wave_model: loom_scene::components::WaveModel::Spectrum,
        ..WaterBody::default()
    };
    let bed = bed();
    let samples = samples();

    let mut source = String::from(loom_voxel::heightfield::slang());
    source.push_str(loom_water::slang());
    source.push_str(
        "\nvoid emit(LoomOcean sea, LoomHeightField bed, float surface_height, float2 xz, \
         float3 orbital, float3 flow, float3 wavelet)\n{\n\
         \x20   float ground_height = loom_ground_height(bed, xz);\n\
         \x20   LoomOceanSample c = loom_ocean_at(sea, xz, float4(1.0, 1.0, 1.0, 1.0));\n\
         \x20   LoomWaterSample s = loom_sample_cascade(c, orbital, surface_height, \
         ground_height, flow, wavelet);\n\
         \x20   printf(\"%.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g \
         %.9g %.9g %.9g %.9g\\n\",\n\
         \x20       s.height, s.normal.x, s.normal.y, s.normal.z,\n\
         \x20       s.displacement.x, s.displacement.y, s.displacement.z,\n\
         \x20       s.velocity.x, s.velocity.y, s.velocity.z, s.depth, s.fold,\n\
         \x20       s.mu_max, s.break_dir.x, s.break_dir.y, ground_height);\n}\n",
    );
    source.push_str(
        "\n[shader(\"compute\")]\n[numthreads(1,1,1)]\n\
         void computeMain(uint3 tid : SV_DispatchThreadID)\n{\n",
    );
    source.push_str(&format!("    float tiles[{}];\n", render.tiles.len()));
    for (i, v) in render.tiles.iter().enumerate() {
        source.push_str(&format!("    tiles[{i}] = {v:?};\n"));
    }
    source.push_str(&format!(
        "    LoomOcean sea;\n    sea.patch = float4({:?}, {:?}, 0.0, 0.0);\n\
         \x20   sea.n = {};\n    sea.count = {};\n    sea.tiles = &tiles[0];\n",
        render.patch[0],
        render.patch[1],
        render.n,
        render.patch.len(),
    ));
    source.push_str(&format!(
        "    float heights[{}];\n",
        bed.height.len()
    ));
    for (i, h) in bed.height.iter().enumerate() {
        source.push_str(&format!("    heights[{i}] = {h:?};\n"));
    }
    source.push_str(&format!(
        "    LoomHeightField bed;\n    bed.origin = float2({:?}, {:?});\n\
         \x20   bed.spacing = {:?};\n    bed.side = {};\n    bed.height = &heights[0];\n",
        bed.origin[0], bed.origin[1], bed.spacing, bed.side,
    ));
    for [x, z, _] in &samples {
        // **The orbital velocity is handed in rather than looked up**, because
        // `loom_ocean_at` is not given the velocity tiles — nothing on the GPU
        // reads a water velocity, so `Ocean::render_tiles` does not upload them.
        // What is checked here is that the two sides add the current to it the
        // same way, which is the only thing this side does with it.
        let v = sea.at(*x, *z).velocity;
        source.push_str(&format!(
            "    emit(sea, bed, {:?}, float2({x:?}, {z:?}), float3({:?}, {:?}, {:?}), \
             float3({:?}, {:?}, {:?}), float3({:?}, {:?}, {:?}));\n",
            body.surface_height,
            v[0],
            v[1],
            v[2],
            FLOW[0],
            FLOW[1],
            FLOW[2],
            WAVELET[0],
            WAVELET[1],
            WAVELET[2],
        ));
    }
    source.push_str("}\n");

    let dir = std::env::temp_dir().join(format!("loom_cascade_agree_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let shader = dir.join("cascade.slang");
    std::fs::write(&shader, source).expect("write shader");
    std::fs::write(dir.join("harness.cpp"), HARNESS).expect("write harness");

    let kernel_cpp = dir.join("kernel.cpp");
    run(
        Command::new(slangc)
            .arg(&shader)
            .args(["-target", "cpp", "-entry", "computeMain", "-stage", "compute"])
            .arg("-o")
            .arg(&kernel_cpp),
        "slangc",
    );
    let binary = dir.join("harness");
    run(
        Command::new(cxx)
            .args(["-O0", "-w", "-std=c++17"])
            .arg(format!("-I{}", dir.display()))
            .arg(dir.join("harness.cpp"))
            .arg("-o")
            .arg(&binary),
        "c++",
    );

    let output = Command::new(&binary).output().expect("run the compiled kernel");
    assert!(output.status.success(), "the kernel exited {}", output.status);
    let text = String::from_utf8(output.stdout).expect("kernel output is utf-8");
    let rows: Vec<Vec<f32>> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split_whitespace().map(|n| n.parse().expect("a number")).collect())
        .collect();
    assert_eq!(rows.len(), samples.len(), "the kernel printed {} rows", rows.len());

    // Otherwise this agrees for free — a kernel that read zeros out of the tile
    // buffer would match a flat sea and prove nothing.
    let largest = rows.iter().flatten().fold(0.0_f32, |a, b| a.max(b.abs()));
    assert!(largest > 1.0, "the Slang cascade is flat — largest value {largest}");

    let mut worst = 0.0_f32;
    let mut worst_at = (0, 0);
    let mut steepest = 0.0_f32;
    for (i, ([x, z, _], row)) in samples.iter().zip(&rows).enumerate() {
        let ground = bed.at(*x, *z);
        let cpu =
            loom_water::sample_water(&body, Some(&sea), [*x, *z], WHEN, ground, FLOW, WAVELET);
        steepest = steepest.max(cpu.mu_max);
        let expected = [
            cpu.height,
            cpu.normal[0],
            cpu.normal[1],
            cpu.normal[2],
            cpu.displacement[0],
            cpu.displacement[1],
            cpu.displacement[2],
            cpu.velocity[0],
            cpu.velocity[1],
            cpu.velocity[2],
            cpu.depth,
            cpu.fold,
            cpu.mu_max,
            cpu.break_dir[0],
            cpu.break_dir[1],
            ground,
        ];
        assert_eq!(row.len(), expected.len(), "row {i} has {} values", row.len());
        for (field, (rust, slang)) in expected.iter().zip(row).enumerate() {
            let delta = (rust - slang).abs();
            assert!(delta.is_finite(), "sample {i} field {field}: {rust} vs {slang}");
            if delta > worst {
                worst = delta;
                worst_at = (i, field);
            }
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
    eprintln!(
        "cascade agreement: worst absolute difference {worst:e} over {} samples \
         (16 values each), steepest mu_max {steepest}",
        samples.len()
    );
    // The breaking columns are the ones a wrong tile index would leave at zero
    // while the height still looked plausible.
    assert!(steepest > 0.01, "the cascade never steepened — mu_max peaked at {steepest}");
    assert!(
        worst < EPSILON,
        "the Rust and the Slang disagree by {worst} at sample {} field {} — \
         the two halves of the FFT ocean have diverged",
        worst_at.0,
        worst_at.1
    );
}
