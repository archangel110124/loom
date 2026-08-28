# Sea rebuild S0 — the ablation harness: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps
> use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make "this feature is drawing nothing" a number the build can fail on, before
anything in the sea rebuild is built that could be invisible.

**Architecture:** One environment variable, `LOOM_ABLATE`, names effects to switch off in
the shipping binary. It rides to the GPU in `viewport.w`, which the environment buffer
already documents as free. A new `cargo xtask ablate` renders each listed scene twice —
once normally, once with one effect ablated — and compares them. **The check fails when
the two are too similar**, which is the inversion that lets it detect an absent feature;
every other gate in this repository fails on difference.

**Tech Stack:** Rust (`loom_render`, `xtask`), Slang (`assets/shaders/scene.slang`),
existing CLI (`loom render`, `loom compare`).

**Spec:** `docs/design/SEA-REBUILD.md` — §7.1 (why), §7.3 (what already exists and what
does not), §8 row S0. Read the spec first; this plan argues from it.

**Branch:** `sea/rebuild`, already cut from `45b9e67`.

## Global Constraints

Copied from `CLAUDE.md` and the spec. Every task's requirements include these.

- **Green is five checks, all of them, every time:** `cargo clippy --workspace -- -D
  warnings`, `cargo xtask validate` (ZERO Vulkan validation messages), `cargo test
  --workspace`, `cargo xtask image`, `cargo xtask repeat`.
- **`cargo check` passing is not done.** The validation layers are the real compiler.
- **Never edit `assets/shaders/generated/*.slang`** — they are emitted from Rust by
  `build.rs`. This plan touches `assets/shaders/scene.slang`, which is hand-written and
  fine to edit.
- **`crates/loom_cli/src/run.rs` and `scripts/green.sh` are the human's, modified and
  uncommitted. Never stage, stash, move or revert either.** Never `git add -A`; every
  commit in this plan names its files explicitly.
- **Small commits, one concern each, each producing something that runs.**
- Never float a dependency version; **`xtask` has no dependencies on purpose** — it drives
  the real `loom` binary as a subprocess, because a gate that shares code with the thing it
  checks can be fooled by the same bug twice. Do not add a dependency to `xtask`, and do
  not link `loom_render` into it.
- `cargo xtask image --bless` is the human's. Do not run it.

## File Structure

| file | responsibility |
|---|---|
| `crates/loom_render/src/ablate.rs` | **new.** The feature registry, the `LOOM_ABLATE` parser, and the mask accessor. Pure arithmetic and string handling; no `ash`, no GPU, fully unit-testable. |
| `crates/loom_render/src/lib.rs` | one line: declare the module. |
| `crates/loom_render/src/renderer.rs` | one line: put the mask in `viewport[3]` where a `0.0` is written today. |
| `assets/shaders/scene.slang` | the bit constants, one `ablated()` helper, and one gate at the foam apply site. |
| `xtask/src/main.rs` | the `ablate` subcommand, its scene table, and a `run_env` sibling to `run`. |

`ablate.rs` is its own file rather than a corner of `renderer.rs` because the parser is the
part with tests and `renderer.rs` is already 3,600 lines.

---

### Task 1: The registry and the parser

Pure Rust. No GPU, no shader, nothing drawn — this task is finished when the parser is
right and tested.

**Files:**
- Create: `crates/loom_render/src/ablate.rs`
- Modify: `crates/loom_render/src/lib.rs` (add `mod ablate;`)
- Test: in `crates/loom_render/src/ablate.rs` (`#[cfg(test)] mod tests`, the convention
  this crate already uses)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub const ABLATIONS: &[(&str, u32)]` — the name/bit table.
  - `pub fn parse_ablations(spec: Option<&str>) -> Result<u32, String>`
  - `pub(crate) fn ablation_mask() -> f32` — the `OnceLock`-cached value for the renderer.

- [ ] **Step 1: Write the failing tests**

Create `crates/loom_render/src/ablate.rs` containing only the doc comment, the table, a
stub, and these tests:

```rust
//! `LOOM_ABLATE` — switch one effect off, so that its contribution can be measured.
//!
//! **This exists because no gate in this project can detect an absent feature.**
//! `Renderer::set_ripples` shipped with no caller and drew dead-flat water through a
//! commit, an ADR and a review; the W9 crown was registered, closed-form-correct and
//! drawn nowhere; grass ran two slices outside `GOLDEN`. In each case a reference image
//! recorded the absence and passed for ever. An ablation asks the opposite question —
//! *what changes if this is removed* — and a feature that is not drawing answers zero.
//!
//! **An environment variable rather than a scene field**, for the reason `ao_rays` gives
//! for the same choice: this is a property of the measurement being taken, not of the
//! scene, and a scene-authored value would make the golden images photograph whatever
//! each `.loom` happened to say.
//!
//! **An unknown name is a hard error and not a silent no-op.** A typo that quietly
//! ablates nothing reports "no difference", which is indistinguishable from the failure
//! this harness exists to catch — it would turn the instrument into a machine for
//! producing false negatives.

/// Every ablatable effect, and its bit.
///
/// **Adding an effect means adding a row here and a row to `xtask`'s own table.** The two
/// are deliberately not shared: `xtask` links nothing from the engine, because a gate that
/// shares code with the thing it checks can be fooled by the same bug twice.
///
/// Bits are assigned explicitly rather than by position, so reordering this table cannot
/// silently repoint an existing name at a different effect.
pub const ABLATIONS: &[(&str, u32)] = &[("water_foam", 1 << 0)];

/// The bit for whitecap and swash foam on the water surface.
pub const WATER_FOAM: u32 = 1 << 0;

/// Parse a `LOOM_ABLATE` value into a mask.
///
/// Comma-separated, whitespace around each name ignored, repeats idempotent. `None` and
/// the empty string are both "ablate nothing", which is every ordinary run.
pub fn parse_ablations(_spec: Option<&str>) -> Result<u32, String> {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_asked_for_is_nothing_ablated() {
        assert_eq!(parse_ablations(None), Ok(0));
        assert_eq!(parse_ablations(Some("")), Ok(0));
        assert_eq!(parse_ablations(Some("   ")), Ok(0));
    }

    #[test]
    fn a_name_is_its_bit() {
        assert_eq!(parse_ablations(Some("water_foam")), Ok(WATER_FOAM));
    }

    #[test]
    fn whitespace_and_repeats_do_not_change_the_mask() {
        assert_eq!(parse_ablations(Some("  water_foam  ")), Ok(WATER_FOAM));
        assert_eq!(parse_ablations(Some("water_foam,water_foam")), Ok(WATER_FOAM));
    }

    /// **The test this module exists for.** A misspelling that parsed to zero would
    /// render an unablated frame, compare it against another unablated frame, and report
    /// "no difference" — which reads exactly like a feature that is not drawing.
    #[test]
    fn an_unknown_name_is_refused_and_says_what_is_valid() {
        let err = parse_ablations(Some("water_form")).unwrap_err();
        assert!(err.contains("water_form"), "the bad name is not in: {err}");
        assert!(err.contains("water_foam"), "the valid names are not in: {err}");
    }

    #[test]
    fn one_bad_name_refuses_the_whole_list() {
        assert!(parse_ablations(Some("water_foam,nonsense")).is_err());
    }

    /// The mask travels in an `f32` (`viewport.w`), whose mantissa is 24 bits, so every
    /// bit up to `1 << 23` survives the round trip exactly. This asserts the table has
    /// not outgrown its carrier.
    #[test]
    fn every_bit_survives_an_f32() {
        for (name, bit) in ABLATIONS {
            assert!(*bit <= 1 << 23, "{name}'s bit {bit} does not fit an f32 mantissa");
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let round_tripped = *bit as f32 as u32;
            assert_eq!(round_tripped, *bit, "{name} does not survive an f32");
        }
    }

    #[test]
    fn no_two_effects_share_a_bit() {
        let mut seen = 0_u32;
        for (name, bit) in ABLATIONS {
            assert_eq!(seen & bit, 0, "{name}'s bit is already taken");
            seen |= bit;
        }
    }
}
```

Add `mod ablate;` to `crates/loom_render/src/lib.rs`, beside the other module
declarations.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p loom_render ablate
```

Expected: FAIL — the tests panic at `unimplemented!()` in `parse_ablations`.

- [ ] **Step 3: Write the implementation**

Replace the `parse_ablations` stub, and add the accessor below it:

```rust
pub fn parse_ablations(spec: Option<&str>) -> Result<u32, String> {
    let mut mask = 0;
    for name in spec.unwrap_or("").split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let Some((_, bit)) = ABLATIONS.iter().find(|(known, _)| *known == name) else {
            let known: Vec<&str> = ABLATIONS.iter().map(|(n, _)| *n).collect();
            return Err(format!(
                "LOOM_ABLATE: unknown effect {name:?}; known effects are {}",
                known.join(", ")
            ));
        };
        mask |= bit;
    }
    Ok(mask)
}

/// The mask this process is running with, for `viewport.w`.
///
/// Read once, like [`crate::renderer::ao_rays`] and for the same reason: nothing polls it,
/// and re-reading the environment per frame would be a syscall in the hot path for a value
/// that cannot change.
///
/// **Panics** on an unknown name, at the first frame, with the list of valid ones. A
/// measurement tool that carried on after being misconfigured would report a number
/// nobody could trust.
pub(crate) fn ablation_mask() -> f32 {
    static MASK: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    #[allow(clippy::cast_precision_loss)]
    let mask = *MASK.get_or_init(|| {
        match parse_ablations(std::env::var("LOOM_ABLATE").ok().as_deref()) {
            Ok(mask) => mask,
            Err(message) => panic!("{message}"),
        }
    });
    mask as f32
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p loom_render ablate
cargo clippy --workspace -- -D warnings
```

Expected: seven tests PASS; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/loom_render/src/ablate.rs crates/loom_render/src/lib.rs
git commit -m "feat(render): name the effects that can be switched off"
```

---

### Task 2: Carry the mask to the GPU and honour it for foam

This task is finished when ablating foam visibly changes `whitecaps.loom` and changes
nothing anywhere else.

**Files:**
- Modify: `crates/loom_render/src/renderer.rs` — the one place `viewport` is assigned
  (search for `self.environment.viewport =`; it currently ends `ao_rays(), 0.0]`)
- Modify: `assets/shaders/scene.slang` — the `viewport` field comment, a bit constant, an
  `ablated()` helper, and the foam apply site
- Test: manual render comparison, in Step 4 — there is no unit test for a shader here, and
  Task 3 is what turns this into an automated one

**Interfaces:**
- Consumes: `crate::ablate::{ablation_mask, WATER_FOAM}` from Task 1.
- Produces: `viewport.w` carries the mask; Slang gains `bool ablated(uint bit)` and
  `LOOM_ABLATE_WATER_FOAM`.

- [ ] **Step 1: Carry the mask in `viewport.w`**

In `crates/loom_render/src/renderer.rs`, the assignment currently reads:

```rust
self.environment.viewport =
    [self.width as f32, self.height as f32, ao_rays(), 0.0];
```

Change the last element:

```rust
self.environment.viewport =
    [self.width as f32, self.height as f32, ao_rays(), crate::ablate::ablation_mask()];
```

And update the doc comment on the `viewport` field (search for `x viewport width in
pixels, y height, zw unused.`) to:

```rust
    /// x viewport width in pixels, y height, z the AO ray dial, w the `LOOM_ABLATE` mask.
```

That comment is already stale — it says `zw unused` while `z` has carried `ao_rays()`
since ADR 0074 — which is worth noticing rather than just fixing: the Slang side's comment
is the one that was kept current, and it is what says `w` is free.

**Leave the `EnvironmentData` default at `[1.0, 1.0, 0.0, 0.0]`.** It is overwritten every
frame by the assignment above. But check, by reading, that no path renders from the
default without that stamp: one that did would silently ignore `LOOM_ABLATE`, report no
difference, and be a false negative of exactly the kind this harness exists to prevent.
If such a path exists, say so and stop — do not paper over it by stamping the default.

- [ ] **Step 2: Teach the shader to read it**

In `assets/shaders/scene.slang`, the environment struct's `viewport` comment says
`w free`. Change it to name the mask, and add the constants and helper beside the other
water constants (near `WATER_FOAM_BREAK`):

```hlsl
/// The `LOOM_ABLATE` mask, as `loom_render::ablate` filled it.
///
/// **Zero on every ordinary run**, so a normal frame takes one compare against zero and
/// nothing else. This is a measurement switch: `cargo xtask ablate` renders a scene with
/// and without an effect and fails when the two are too *similar*, because a feature whose
/// removal changes nothing was never drawing.
static const uint LOOM_ABLATE_WATER_FOAM = 1u;

bool ablated(uint bit) {
    return (uint(push.environment[0].viewport.w) & bit) != 0u;
}
```

- [ ] **Step 3: Gate the foam**

Find the water shading site where the final foam coverage is applied — search for
`float3 foamLit = lerp(WATER_FOAM_ALBEDO, WATER_FOAM_OLD_ALBEDO, foamAge)`. Immediately
**before** that line, insert:

```hlsl
    // Ablation: zero the coverage at its single point of use rather than at each of the
    // terms feeding it, so what is measured is exactly "the foam this frame drew" and
    // not one contributor to it.
    if (ablated(LOOM_ABLATE_WATER_FOAM)) { foam = 0.0; }
```

- [ ] **Step 4: Prove it end to end by hand**

```bash
cargo build --release
./target/release/loom render assets/test/whitecaps.loom --sim 400 --size 640x400 \
    --out target/ablate-on.png
LOOM_ABLATE=water_foam ./target/release/loom render assets/test/whitecaps.loom --sim 400 \
    --size 640x400 --out target/ablate-off.png
./target/release/loom compare target/ablate-on.png target/ablate-off.png
```

Expected: `compare` exits 1 and reports a **large** `fraction` — `whitecaps.loom` measures
`foam.coverage = 0.954` at the origin, so a substantial share of the frame must move.
Record the number; Task 3's floor is set from it.

Then prove the switch is off by default and correctly spelled:

```bash
LOOM_ABLATE=water_form ./target/release/loom render assets/test/whitecaps.loom \
    --sim 400 --size 640x400 --out target/ablate-typo.png
```

Expected: a panic naming `water_form` and listing `water_foam`. **Not** a silent success.

- [ ] **Step 5: Prove nothing else moved**

```bash
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo xtask validate
cargo xtask image
cargo xtask repeat
```

Expected: all green, **and `cargo xtask image` reports no reference moved.** An unset
`LOOM_ABLATE` is a zero mask, so every golden reference must be untouched. If any moved,
the gate is in the wrong place — stop and find out why rather than blessing it.

- [ ] **Step 6: Commit**

```bash
git add crates/loom_render/src/renderer.rs assets/shaders/scene.slang
git commit -m "feat(render): an effect can be switched off from the environment"
```

---

### Task 3: `cargo xtask ablate` — the gate that fails on sameness

**Files:**
- Modify: `xtask/src/main.rs` — the dispatch `match`, the usage string, a new `ABLATE`
  table, `fn ablate()`, and a `run_env` sibling to `run`
- Test: running the task itself, in Step 4

**Interfaces:**
- Consumes: the `LOOM_ABLATE` names from Task 1 — **by literal string**, never by linking.
- Produces: `cargo xtask ablate`, exit 0 when every row moves enough and 1 when a row does
  not.

- [ ] **Step 1: Add the table and the failing task**

In `xtask/src/main.rs`, beside `const GOLDEN`, add:

```rust
/// What each ablatable effect is measured on, and how much of the frame it must move.
///
/// `(scene, LOOM_ABLATE value, minimum fraction of pixels that must change)`
///
/// **This table is hand-kept and deliberately not shared with `loom_render`'s own
/// registry.** `xtask` links nothing from the engine — a gate that shares code with the
/// thing it checks can be fooled by the same bug twice — so adding an effect means adding
/// a row in both places. The floor is set from a measured run and written down with the
/// row, never guessed: too low and the check passes on a feature that has almost stopped
/// drawing, which is the exact failure it exists to catch.
///
/// The scene is chosen to be the one where the effect is *loudest*. `whitecaps.loom`
/// measures `foam.coverage = 0.954` at the origin, which is why foam is measured there
/// and not on `ocean`.
const ABLATE: [(&str, &str, f64); 1] = [("whitecaps", "water_foam", 0.10)];
```

Add `"ablate" => ablate(),` to the dispatch `match` beside `"repeat" => repeat(),`, add
`ablate` to the `GateLock::acquire` arm alongside the other GPU tasks, and add
`    cargo xtask ablate` to the usage string.

Then the task itself, and the env-aware runner beside `fn run`:

```rust
/// `loom`, with environment variables set.
///
/// A sibling of [`run`] rather than a parameter on it, because every other caller passes
/// none and threading an empty slice through all of them would be noise.
fn run_env(loom: &Path, root: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<Output, String> {
    let mut command = Command::new(loom);
    command.args(args).current_dir(root);
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().map_err(|e| e.to_string())
}

/// Render each scene with and without one effect, and fail if they are too alike.
///
/// **This is the only check in this repository that fails on SAMENESS**, and that is the
/// whole point of it. `image` and `repeat` both ask whether a render changed when it
/// should not have. Neither can see a feature that is registered, tested, closed-form
/// correct and drawing nothing — which has shipped four times here. Removing such a
/// feature changes no pixel, so it scores zero and this task fails it.
fn ablate() -> std::process::ExitCode {
    let root = repo_root();
    let loom = match build_debug(&root) {
        Ok(path) => path,
        Err(message) => {
            eprintln!("xtask: {message}");
            return std::process::ExitCode::from(2);
        }
    };
    if !has_vulkan_device(&loom, &root) {
        println!("skip: cargo xtask ablate — no usable Vulkan device on this machine");
        return std::process::ExitCode::SUCCESS;
    }

    let scratch = root.join("target/xtask-ablate");
    let _ = std::fs::create_dir_all(&scratch);

    println!("scene                effect          changed%   floor%   verdict");
    let mut failures = Vec::new();

    for (scene, effect, floor) in ABLATE {
        let path = format!("assets/test/{scene}.loom");
        let with = scratch.join(format!("{scene}_{effect}_with.png"));
        let without = scratch.join(format!("{scene}_{effect}_without.png"));

        let render = |out: &Path, env: &[(&str, &str)]| {
            run_env(
                &loom,
                &root,
                &[
                    "render",
                    &path,
                    "--sim",
                    "400",
                    "--size",
                    GOLDEN_SIZE,
                    "--out",
                    &out.to_string_lossy(),
                ],
                env,
            )
        };

        if render(&with, &[]).is_err() || render(&without, &[("LOOM_ABLATE", effect)]).is_err() {
            failures.push(format!("{scene}/{effect}: render failed"));
            continue;
        }

        let Ok(output) = run(
            &loom,
            &root,
            &["compare", &with.to_string_lossy(), &without.to_string_lossy()],
        ) else {
            failures.push(format!("{scene}/{effect}: compare failed to run"));
            continue;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let Some(fraction) = read_field(&text, "\"fraction\"") else {
            failures.push(format!("{scene}/{effect}: compare printed no fraction"));
            continue;
        };

        let ok = fraction >= floor;
        println!(
            "{scene:<20} {effect:<15} {:>7.3}  {:>7.3}   {}",
            fraction * 100.0,
            floor * 100.0,
            if ok { "ok" } else { "DRAWING NOTHING" }
        );
        if !ok {
            failures.push(format!(
                "{scene}/{effect}: removing it changed {:.3}% of the frame, floor is {:.3}% \
                 — the effect is not drawing",
                fraction * 100.0,
                floor * 100.0
            ));
        }
    }

    if failures.is_empty() {
        println!("\nok: {} ablation(s), every effect is drawing", ABLATE.len());
        return std::process::ExitCode::SUCCESS;
    }
    eprintln!();
    for failure in &failures {
        eprintln!("xtask: {failure}");
    }
    std::process::ExitCode::from(1)
}
```

- [ ] **Step 2: Run it and watch it pass**

```bash
cargo xtask ablate
```

Expected: one row, `whitecaps / water_foam`, `changed%` well above 10.000, verdict `ok`,
exit 0. If `changed%` is close to the floor, raise or lower the floor **to a value the
measurement supports** and say so in the table's comment — do not leave a floor that the
measured value only just clears.

- [ ] **Step 3: Prove the gate actually bites**

This is the step that shows the instrument works, and it is the one that would have caught
all four historical defects. Temporarily break the effect, exactly as if it had shipped
broken:

In `assets/shaders/scene.slang`, change the gate from Task 2 to fire unconditionally:

```hlsl
    foam = 0.0;
```

Then:

```bash
cargo xtask ablate
```

Expected: `changed%` **0.000**, verdict `DRAWING NOTHING`, exit 1, with the message naming
the scene and the effect.

**Revert that edit** before continuing:

```bash
git checkout -- assets/shaders/scene.slang
```

- [ ] **Step 4: Run the full gate set**

```bash
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo xtask validate
cargo xtask image
cargo xtask repeat
cargo xtask ablate
```

Expected: all six green, and **no golden reference moved**.

- [ ] **Step 5: Commit**

```bash
git add xtask/src/main.rs
git commit -m "feat(xtask): a feature that changes nothing was never there"
```

- [ ] **Step 6: Write down how a break is judged**

The sequence instrument already exists and S0 adds nothing to it — the spec's §7.3 says
so, corrected after checking. Record the recipe where the next phase will look for it, by
appending to `docs/design/SEA-REBUILD.md` §7.3:

```markdown
The recipe, verified on `whitecaps.loom`:

    loom render <scene> --frames 8 --spin 0 --step 6 --sim <t> --size 960x600 --out <f>.png

`--spin 0` holds the camera and `--step` advances the simulation between frames, so the
subject moves and the viewer does not. Eight frames at six ticks is one second of a break
at 48 Hz. Judge the sequence; then judge the ablation number. Neither alone is enough —
the sequence says whether it looks right and the ablation says whether it is there.
```

```bash
git add docs/design/SEA-REBUILD.md
git commit -m "docs(sea): how a break gets judged, and how absence gets caught"
```

---

## Self-Review

**Spec coverage.** S0's row in the spec's §8 asks for two things. The ablation harness is
Tasks 1–3. The sequence instrument needed no work — verified by running it, and the spec's
§7.3 was corrected to say so rather than leaving a plan task for something that already
exists. §7.1's requirement that "every new visual is measured against a build with it
removed" now has the mechanism it was describing; later phases add rows to two tables.

**No gaps that belong to S0.** The spec's other sections are S1–S6 and are out of scope
here by design.

**Placeholder scan.** No "TBD", no "add error handling", no "similar to Task N". Every
code step carries the code. Two numbers are deliberately provisional and both say so out
loud: the `0.10` floor is to be replaced by whatever Task 2 Step 4 measures, and Task 3
Step 2 says what to do if the measurement sits near it.

**Type consistency.** `parse_ablations(Option<&str>) -> Result<u32, String>`,
`ablation_mask() -> f32`, `WATER_FOAM: u32` and `ABLATIONS: &[(&str, u32)]` are defined in
Task 1 and used with those exact signatures in Task 2. The Slang side uses
`LOOM_ABLATE_WATER_FOAM = 1u` and `ablated(uint) -> bool`, matching bit `1 << 0`. `xtask`
uses the string `"water_foam"` and links nothing, which is the intended asymmetry and is
called out in both tables' comments. `run_env` mirrors `run`'s existing signature with one
added parameter, and `read_field`, `repo_root`, `build_debug`, `has_vulkan_device`,
`GateLock` and `GOLDEN_SIZE` are all existing `xtask` items used as they are.

**One risk worth naming.** Task 2 Step 5 asserts no golden reference moves. If one does,
the likely cause is that `viewport.w` was not actually free on some path — the field's own
comment says it is, but the comment is the only evidence. That is why the step is a stop
condition rather than a bless.
