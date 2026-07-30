//! Compile every `.slang` in `assets/shaders/` to SPIR-V.
//!
//! Brief §7.7 and never-do #9: **this must never swallow a shader compile
//! error.** A build script that silently skips a failed shader gives you
//! `cargo check` passing and garbage on screen with no diagnostic anywhere —
//! the single worst debugging position in the project.
//!
//! So: any `slangc` failure fails the build with its full output, and every
//! emitted module is checked with `spirv-val`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let shader_dir = manifest.join("../../assets/shaders");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));

    println!("cargo:rerun-if-changed={}", shader_dir.display());

    let Ok(entries) = std::fs::read_dir(&shader_dir) else {
        // No shader directory yet is fine — an empty one is not an error.
        // A *present* shader that fails to compile always is.
        return;
    };

    let mut shaders: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "slang"))
        .collect();
    // Sorted so the build is reproducible and error output is stable.
    shaders.sort();

    if shaders.is_empty() {
        return;
    }

    for shader in &shaders {
        println!("cargo:rerun-if-changed={}", shader.display());
        compile(shader, &out_dir);
    }
}

fn compile(shader: &Path, out_dir: &Path) {
    let stem = shader
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_else(|| panic!("shader path is not valid UTF-8: {}", shader.display()));
    let spv = out_dir.join(format!("{stem}.spv"));

    let output = Command::new("slangc")
        .arg(shader)
        .args(["-target", "spirv"])
        .args(["-profile", "spirv_1_5"])
        // Matrix layout must match what the Rust side writes. Pinning it here
        // rather than relying on the default keeps the two from drifting.
        .arg("-matrix-layout-row-major")
        .arg("-g2")
        .args(["-o".as_ref(), spv.as_os_str()])
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "failed to run `slangc` ({e}).\n\
                 Slang is not packaged for Fedora — install it from \
                 https://github.com/shader-slang/slang/releases and put slangc on PATH.\n\
                 NOTE: the Fedora package called `slang` is S-Lang, the terminal library. \
                 Not this.\n\
                 See docs/design/README.md 'Environment prerequisites'."
            )
        });

    if !output.status.success() {
        panic!(
            "slangc failed for {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            shader.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    validate(&spv, shader);
}

/// `spirv-val` on every emitted module (brief §7.7).
///
/// Absence of the tool is a warning, not a failure: it is a second opinion on
/// output `slangc` already accepted, so requiring it would block builds on
/// machines that can otherwise compile correctly. A *validation failure*,
/// however, is fatal — that is a real broken module.
fn validate(spv: &Path, source: &Path) {
    match Command::new("spirv-val").arg(spv).output() {
        Ok(out) if !out.status.success() => panic!(
            "spirv-val rejected the module built from {}\n{}",
            source.display(),
            String::from_utf8_lossy(&out.stderr),
        ),
        Ok(_) => {}
        Err(_) => println!(
            "cargo:warning=spirv-val not found; skipping SPIR-V validation for {}. \
             Install spirv-tools (brief §7.7).",
            source.display()
        ),
    }
}
