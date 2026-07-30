# Scene + recipe format spec

**Status: stub. Write this before the parser (M1).**

A format without a spec is not a contract — an agent authoring against an unspecified format
invents variations. Godot's own contributor docs concede their sub-resource format is only
discoverable by reading engine source. Do not repeat that.

To specify before `loom_scene` parses anything:

- `.loom` scene grammar — nodes, components, references, prefab instances + overrides
- JSON Schema generation from `#[derive(Reflect)]`, and what "schema-validated on load" rejects
- Version token semantics: where it lives, when it bumps, what a stale write does (reject + reload,
  never merge)
- Terrain recipe format — op-list serialization for voxels. Raw voxel arrays are never serialized.
- Stability guarantees: what may change without a migration, what needs one
