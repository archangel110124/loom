# ADR 0001 — Rust edition 2024, not 2021

- **Date:** 2026-07-30
- **Status:** accepted
- **Decision touched:** LOOM-BUILD-BRIEF.md §2, row "Language" ("Rust, 2021 edition, pinned toolchain")

## Context

The build brief's locked-decision table specifies edition 2021. The M0 skeleton commit (96bb0fb)
shipped `edition = "2024"` and `resolver = "3"` in the workspace manifest without recording why.
That is a locked-table entry changed silently, which is exactly the drift §7.12 of the brief warns
about — a future session reading the brief will believe 2021 and a future session reading the
manifest will believe 2024.

The brief was written before the toolchain was pinned. Edition 2024 stabilized in Rust 1.85; the
pinned toolchain is 1.97.1, so it is well-established rather than bleeding-edge. `resolver = "3"`
is the edition-2024 default and is what makes per-dependency feature unification behave sanely in
a workspace this size.

Nothing in the project depends on 2021 semantics. The workspace is currently green on 2024
(`scripts/green.sh` exits 0).

## Decision

Edition 2024 for every crate in the workspace, via `[workspace.package] edition = "2024"` and
`edition.workspace = true` in members. The brief's §2 row is corrected to read 2024. Toolchain
stays pinned exactly in `rust-toolchain.toml`; the edition is not to be varied per crate.

## Consequences

- `unsafe` blocks inside `unsafe fn` are no longer implicit. This lands squarely on `loom_render`
  at M2, which will be the only crate with meaningful `unsafe`. It is a net win there: the Vulkan
  doc §13 already requires wrapping every raw handle in an RAII type, and explicit `unsafe` blocks
  make the audit surface visible rather than ambient.
- `gen` is a reserved keyword. No current code uses it; worth knowing before naming anything in
  `loom_voxel` generation code.
- Match ergonomics and lifetime capture rules changed. Both are stricter, both surface at compile
  time, neither is silent.
- Anyone porting a snippet from a pre-2024 Rust reference may hit these. That is the same class of
  hazard as `ash` API churn (brief §7.2) and gets the same treatment: read the real source.

## Human approval

Pending. Flagged to the human on 2026-07-30 as part of the companion-doc landing. The change is
already in `master` as of 96bb0fb; this ADR records it rather than proposing it. If the human
prefers 2021, reverting is a three-line manifest change while the workspace is still one crate —
it gets expensive after M1.
