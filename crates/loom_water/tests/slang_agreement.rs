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
//! Skips honestly, like every other tool-dependent check here, when `slangc` or
//! a C++ compiler is missing.

use std::path::PathBuf;
use std::process::Command;

use loom_scene::components::{GerstnerWave, WaterBody, WaveSet};

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

/// The sample set, generated once in Rust and written into the shader as
/// literals.
///
/// **Not derived from the invocation index on each side.** Deriving them twice
/// would reintroduce the exact hazard this test exists to catch, inside the test
/// meant to catch it. Scattered by an integer hash rather than walked on a grid,
/// because a regular grid can land every sample on the same phase of a sinusoid
/// and agree for the wrong reason.
fn samples() -> Vec<[f32; 4]> {
    (0..SAMPLES)
        .map(|i| {
            let h = |k: u32| {
                let mut x = (i as u32).wrapping_mul(0x9E37_79B9).wrapping_add(k);
                x ^= x >> 16;
                x = x.wrapping_mul(0x7FEB_352D);
                x ^= x >> 15;
                f32::from(x as u16) / f32::from(u16::MAX)
            };
            [
                h(1).mul_add(400.0, -200.0), // x
                h(2).mul_add(400.0, -200.0), // z
                h(3) * 120.0,                // t, seconds
                h(4).mul_add(30.0, -25.0),   // ground height, for depth
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
    let samples = samples();
    let dir = std::env::temp_dir().join(format!("loom_water_agree_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");

    let shader = dir.join("water.slang");
    std::fs::write(&shader, kernel(&body, &samples)).expect("write shader");
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
    for (i, (sample, row)) in samples.iter().zip(&rows).enumerate() {
        let cpu = loom_water::sample_water(&body, [sample[0], sample[1]], sample[2], sample[3]);
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
        "water agreement: worst absolute difference {worst:e} over {} samples \
         ({} values each)",
        samples.len(),
        11
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
        "{}\nRWStructuredBuffer<float> loom_water_probe;\n\n\
         [shader(\"compute\")]\n[numthreads(1,1,1)]\n\
         void probeMain(uint3 tid : SV_DispatchThreadID)\n{{\n\
         \x20   LoomWaveSet set;\n    set.count = 1;\n\
         \x20   set.waves[0].direction = float2(1.0, 0.0);\n\
         \x20   set.waves[0].wavelength = 10.0;\n\
         \x20   set.waves[0].amplitude = 0.5;\n\
         \x20   set.waves[0].steepness = 0.5;\n\
         \x20   set.waves[0].speed_scale = 1.0;\n\
         \x20   LoomWaterSample s = loom_sample_water(set, 0.0, -4.0, float2(1.0, 2.0), 3.0);\n\
         \x20   loom_water_probe[0] = s.height + s.normal.y + s.displacement.x\n\
         \x20       + s.velocity.z + s.depth;\n}}\n",
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

/// The Slang source: the twin, plus a kernel that prints the sample set.
fn kernel(body: &WaterBody, samples: &[[f32; 4]]) -> String {
    let mut out = String::from(loom_water::slang());
    out.push_str(
        "\n[shader(\"compute\")]\n[numthreads(1,1,1)]\nvoid computeMain(uint3 tid : SV_DispatchThreadID)\n{\n    LoomWaveSet set;\n",
    );
    // `%.9g` round-trips an f32 exactly, and `{v:?}` on the Rust side emits the
    // shortest decimal that parses back to the same f32 — so the wave table and
    // the sample set cross into Slang without a rounding step of their own.
    out.push_str(&format!("    set.count = {};\n", body.waves.waves.len()));
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
    for [x, z, t, ground] in samples {
        out.push_str(&format!(
            "    emit(set, {:?}, {:?}, float2({x:?}, {z:?}), {t:?});\n",
            body.surface_height, ground,
        ));
    }
    out.push_str("}\n");

    // Declared above `computeMain` in the emitted text, since Slang wants it
    // before use.
    let emit = "\nvoid emit(LoomWaveSet set, float surface_height, float ground_height, float2 xz, float t)\n\
                {\n    LoomWaterSample s = loom_sample_water(set, surface_height, ground_height, xz, t);\n\
                \x20   printf(\"%.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g\\n\",\n\
                \x20       s.height, s.normal.x, s.normal.y, s.normal.z,\n\
                \x20       s.displacement.x, s.displacement.y, s.displacement.z,\n\
                \x20       s.velocity.x, s.velocity.y, s.velocity.z, s.depth);\n}\n";
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
