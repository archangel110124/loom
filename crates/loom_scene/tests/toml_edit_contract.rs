//! What `toml_edit` promises about array-of-tables, pinned.
//!
//! This began as the Stage 1 spike the editor plan makes a stop-and-redesign
//! gate: **can a splice edit `[[node.components.VoxelVolume.ops]]` in place,
//! preserving whichever spelling is on disk?** Sculpting, foliage,
//! prefab-instance duplication and four inspector fields all sit on the
//! answer, so it was asked before any of them were built.
//!
//! The answer is yes, and these tests are kept because the answer is a
//! property of a **pinned** dependency (`toml_edit = "=0.25.13"`) rather than
//! of anything in this repository. A version bump that changed any of it
//! would otherwise show up as scene files quietly reformatting themselves
//! under the editor, which is the diff-noise failure the whole
//! format-preserving DOM exists to avoid.

use toml_edit::{DocumentMut, Item, Table, value};

/// The spelling `.loom` files on disk actually use — see
/// `assets/test/ground.loom:70`, indented under an `[[node]]`.
const HEADER: &str = r#"
[scene]
format = 1

[[node]]
name = "Ground"

  [node.components.VoxelVolume]
  voxel_size = 0.25

  # The slab.
  [[node.components.VoxelVolume.ops]]
  kind = "box"
  center = [12.0, 2.5, 12.0]

  [[node.components.VoxelVolume.ops]]
  kind = "sphere"
  radius = 3.0
"#;

/// The same data, inline. The parser accepts it, so the editor has to too.
const INLINE: &str = r#"
[scene]
format = 1

[[node]]
name = "Ground"
components = { VoxelVolume = { voxel_size = 0.25, ops = [ { kind = "box" }, { kind = "sphere" } ] } }
"#;

/// Navigate to `node[0].components.<ty>.<field>`, as the ops layer does.
fn field<'a>(doc: &'a mut DocumentMut, ty: &str, f: &str) -> &'a mut Item {
    doc["node"]
        .as_array_of_tables_mut()
        .unwrap()
        .get_mut(0)
        .unwrap()
        .get_mut("components")
        .unwrap()
        .as_table_like_mut()
        .unwrap()
        .get_mut(ty)
        .unwrap()
        .as_table_like_mut()
        .unwrap()
        .get_mut(f)
        .unwrap()
}

/// **The two spellings parse to two different `Item` variants.**
///
/// This is the load-bearing fact: "preserve whichever spelling is on disk"
/// needs no policy and no stored flag, because the variant *is* the spelling.
/// A splice branches on it and cannot get it wrong.
#[test]
fn the_two_spellings_are_distinguishable_after_parsing() {
    let mut header: DocumentMut = HEADER.parse().unwrap();
    let item = field(&mut header, "VoxelVolume", "ops");
    assert!(item.is_array_of_tables(), "`[[...]]` must stay an ArrayOfTables");
    assert!(!item.is_array());

    let mut inline: DocumentMut = INLINE.parse().unwrap();
    let item = field(&mut inline, "VoxelVolume", "ops");
    assert!(item.is_array(), "`ops = [...]` must stay an Array");
    assert!(!item.is_array_of_tables());
}

/// Appending to the header form must re-emit a header, not collapse the whole
/// array to an inline one — which is what `SetField` on `ops` would do, and
/// why a splice op has to exist at all.
#[test]
fn appending_preserves_the_array_of_tables_spelling() {
    let mut doc: DocumentMut = HEADER.parse().unwrap();
    let mut op = Table::new();
    op["kind"] = value("capsule");
    op["radius"] = value(1.5);
    field(&mut doc, "VoxelVolume", "ops")
        .as_array_of_tables_mut()
        .unwrap()
        .push(op);

    let text = doc.to_string();
    assert_eq!(
        text.matches("[[node.components.VoxelVolume.ops]]").count(),
        3,
        "the appended op was not written as a header:\n{text}"
    );
    // The human's comment on the first op survives, which is the reason this
    // layer edits a DOM instead of re-emitting the file.
    assert!(text.contains("# The slab."));
}

/// **Append then delete must be byte-identical to where it started.**
///
/// The editor's undo is re-applying an inverse transaction, not restoring a
/// snapshot, so an op that round-trips with even a whitespace difference
/// makes every undo a diff.
#[test]
fn append_then_delete_round_trips_byte_identical() {
    let mut doc: DocumentMut = HEADER.parse().unwrap();
    let before = doc.to_string();

    let mut op = Table::new();
    op["kind"] = value("capsule");
    field(&mut doc, "VoxelVolume", "ops")
        .as_array_of_tables_mut()
        .unwrap()
        .push(op);
    assert_ne!(doc.to_string(), before, "the append did nothing");

    field(&mut doc, "VoxelVolume", "ops")
        .as_array_of_tables_mut()
        .unwrap()
        .remove(2);
    assert_eq!(doc.to_string(), before);
}

/// A splice is not only an append: an op inserted in the middle of a voxel
/// recipe changes the result, because the ops are applied in order.
#[test]
fn a_middle_insert_keeps_order_and_spelling() {
    let mut doc: DocumentMut = HEADER.parse().unwrap();
    let mut op = Table::new();
    op["kind"] = value("capsule");
    field(&mut doc, "VoxelVolume", "ops")
        .as_array_of_tables_mut()
        .unwrap()
        .insert(1, op);

    let kinds: Vec<String> = field(&mut doc, "VoxelVolume", "ops")
        .as_array_of_tables()
        .unwrap()
        .iter()
        .map(|t| t["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds, ["box", "capsule", "sphere"]);
    assert_eq!(
        doc.to_string()
            .matches("[[node.components.VoxelVolume.ops]]")
            .count(),
        3
    );
}

/// The inline form has to work too, or a splice would refuse to edit a file
/// the parser happily loads.
#[test]
fn the_inline_spelling_splices_as_an_array() {
    let mut doc: DocumentMut = INLINE.parse().unwrap();
    let mut op = toml_edit::InlineTable::new();
    op.insert("kind", "capsule".into());
    field(&mut doc, "VoxelVolume", "ops")
        .as_array_mut()
        .unwrap()
        .push(op);

    let text = doc.to_string();
    assert!(text.contains("capsule"));
    assert!(
        !text.contains("[[node.components.VoxelVolume.ops]]"),
        "an inline array must not sprout headers:\n{text}"
    );
}

/// **`Scene::parse` accepts both spellings and cannot tell them apart.**
///
/// If it could, the editor would have to care which one it wrote, and
/// round-tripping a file through the inspector could change what the scene
/// means rather than only how it reads.
#[test]
fn the_parser_accepts_both_spellings_identically() {
    let header = loom_scene::Scene::parse(HEADER).expect("header form must parse");
    let inline = loom_scene::Scene::parse(INLINE).expect("inline form must parse");

    let ops = |s: &loom_scene::Scene| -> usize {
        serde_json::to_value(&s.nodes()[0].components).unwrap()["VoxelVolume"]["ops"]
            .as_array()
            .map_or(0, Vec::len)
    };
    assert_eq!(ops(&header), 2);
    assert_eq!(ops(&header), ops(&inline));
}
