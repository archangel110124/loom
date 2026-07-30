# ADR 0004 — `schemars` instead of a hand-written `#[derive(Reflect)]`

- **Date:** 2026-07-30
- **Status:** accepted
- **Decision touched:** LOOM-BUILD-BRIEF.md §6 M1; `ai-native-engine-design.md` §2.1

## Context

The design doc calls the reflection derive "the keystone" and "the biggest single reason Rust is
right for this project." Brief M1 says: *"`#[derive(Reflect)]` generating serde impls, a JSON Schema
entry, a type-registry entry, and doc strings."* Neither appears in the brief's §2 locked table, so
both are plan rather than lock (§7.13).

Reading the real `schemars` 1.2.2 source before writing any code showed that its derive already
emits all four of those things:

| §2.1 wants | `schemars` 1.2.2 provides | Verified in |
| --- | --- | --- |
| serde impls | `serde`'s own derive | — |
| JSON Schema entry | `#[derive(JsonSchema)]` | `macros.rs:19` |
| range/enum constraints | `#[schemars(range(min, max))]` → `minimum`/`maximum` | `attr/validation.rs:80` |
| doc strings queryable | `///` → `description`, first line → `title` | `attr/mod.rs:187-197` |
| arbitrary extra metadata | `#[schemars(extend("k" = v))]` | `attr/mod.rs:124` |

The type registry is then a `BTreeMap<String, Schema>` — because **the JSON Schema already *is* the
registry entry.** It carries field names, types, docs, defaults, and constraints in one artifact
that is also exactly what the agent's tool API needs to consume.

A hand-written `Reflect` derive would have had to re-derive all of that, and a derive macro cannot
add `#[derive(Serialize)]` to its input anyway — it can only append items — so `#[derive(Reflect)]`
as literally specified could never have generated the serde impls without becoming an attribute
macro that rewrites the struct.

## Decision

No custom proc-macro crate for M1. Component types are declared:

```rust
#[derive(Serialize, Deserialize, JsonSchema)]
struct Light {
    /// Luminous intensity. Interior lights are typically 100-800.
    #[schemars(range(min = 0.0, max = 10000.0))]
    intensity: f32,
}
```

and registered explicitly, `registry.register::<Light>("Light")`, once per type in one function.

`register` takes the name rather than using `T::schema_name()`: the scene format addresses
components by the name authors write in `.loom` files, and that must not silently change when
someone renames a Rust type. A rename is a format migration (`docs/format/README.md` §9).

## Consequences

- **The design doc's promise survives intact.** "The agent's API surface is never written by hand"
  still holds — schema, validation, docs, and (at M5.5) inspector widgets all derive from the type
  declaration. Only the *derive macro's authorship* changed, not the property it guaranteed.
- **Field doc comments become rejection hints for free.** `description` flows into `FieldError.hint`,
  so writing a good doc comment and teaching the agent are the same act. That was going to be
  bespoke work in the `#[loom(doc = ...)]` design.
- **Registration is a hand-maintained list.** Six entries at M1. This is the one thing lost, and it
  is a real drift risk once the list is long. Threshold for revisiting: roughly 20 component types,
  or the first time a type is added and someone forgets to register it. The fix then is a small
  `#[derive(Reflect)]` that emits only a registration entry via `inventory`/`linkme` — purely
  additive, no consumer changes, because the registry API is already the seam.
- **`#[loom(...)]` attribute vocabulary is not implemented.** `#[schemars(...)]` is used directly.
  If Loom-specific metadata is ever needed beyond what schemars models, `extend("x-loom-...")` is
  the slot, and no new macro is required.
- Three dependencies instead of a proc-macro crate: `schemars`, `serde`, `serde_json`, all pinned
  exactly. `loom_reflect` still depends on nothing else in the workspace (verified via `cargo tree`).

## Human approval

Not required — neither `#[derive(Reflect)]` nor the reflection mechanism appears in the brief's §2
locked table, so this is an implementation choice under §7.13. Flagged on 2026-07-30. If a bespoke
derive is wanted anyway, it is additive: the registry API does not change.
