//! Mesh data in the layout the GPU reads it.

use crate::AssetError;

/// One vertex.
///
/// Two `vec4`s, not two `vec3`s. std430 for a `PhysicalStorageBuffer` block
/// requires 16-byte-aligned members, and `[f32; 3]` would place `normal` at
/// offset 12 — which `spirv-val` rejects outright.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vertex {
    pub position: [f32; 4],
    pub normal: [f32; 4],
    /// Texture coordinates. `[0, 0]` for geometry that has none — voxel
    /// meshes, which are textured triplanar because Surface Nets has no
    /// surface to unwrap.
    pub uv: [f32; 2],
}

impl Vertex {
    /// A vertex with no texture coordinates.
    ///
    /// Kept as the short constructor because most callers are procedural
    /// geometry that genuinely has no UVs, and writing `[0.0, 0.0]` at every
    /// one of them would say nothing.
    #[must_use]
    pub fn new(position: [f32; 3], normal: [f32; 3]) -> Self {
        Self::with_uv(position, normal, [0.0, 0.0])
    }

    #[must_use]
    pub fn with_uv(position: [f32; 3], normal: [f32; 3], uv: [f32; 2]) -> Self {
        Self {
            position: [position[0], position[1], position[2], 1.0],
            normal: [normal[0], normal[1], normal[2], 0.0],
            uv,
        }
    }
}

/// An indexed triangle mesh.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub name: String,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Axis-aligned bounds, for camera framing and collider generation.
    #[must_use]
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for v in &self.vertices {
            for axis in 0..3 {
                min[axis] = min[axis].min(v.position[axis]);
                max[axis] = max[axis].max(v.position[axis]);
            }
        }
        if self.vertices.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        (min, max)
    }

    /// Every index must address a real vertex, and triangles come in threes.
    ///
    /// Checked on import rather than trusted: a malformed index is an
    /// out-of-bounds read on the GPU, which is undefined behaviour that
    /// usually looks like garbage geometry rather than a crash.
    ///
    /// # Errors
    /// [`AssetError::Unsupported`] naming what is wrong.
    pub fn validate(&self) -> Result<(), AssetError> {
        if !self.indices.len().is_multiple_of(3) {
            return Err(AssetError::Unsupported(format!(
                "{}: {} indices is not a whole number of triangles",
                self.name,
                self.indices.len()
            )));
        }
        let count = self.vertices.len();
        if let Some(bad) = self.indices.iter().find(|i| **i as usize >= count) {
            return Err(AssetError::Unsupported(format!(
                "{}: index {bad} addresses vertex {bad} of {count}",
                self.name
            )));
        }
        Ok(())
    }
}

/// Import a Wavefront OBJ, merged into one mesh.
///
/// **Hand-written rather than a crate, and that is the ladder rather than
/// pride.** OBJ is six line types and the parser below is under a hundred
/// lines of `std`; the crates that read it pull in a parser framework and an
/// error library to do the same job. `import_gltf` earns its dependency
/// because glTF is a binary container with buffer views, accessors, sparse
/// storage and a JSON schema. This does not.
///
/// **Everything is merged, exactly as `import_gltf` merges.** A
/// `MeshRenderer` names one mesh, and splitting a multi-object file into
/// separate assets is the importer's job at the point where materials exist
/// to distinguish them — which is still not yet, for the same reason.
///
/// Faces may be triangles or larger polygons; anything with more than three
/// vertices is fanned from its first vertex, which is correct for the convex
/// faces exporters emit and is what every other OBJ reader does.
///
/// **Missing normals are computed per face, not defaulted to up.** An OBJ
/// without `vn` is common — it is the default for several exporters — and a
/// tree lit as though every leaf faced the sky is worse than a slightly
/// faceted one.
///
/// # Errors
/// [`AssetError`] if the file cannot be read or contains no triangles.
pub fn import_obj(path: &std::path::Path) -> Result<Mesh, AssetError> {
    import_obj_object(path, None)
}

/// Import one named `o` group from a Wavefront OBJ, or all of them.
///
/// **A multi-object OBJ is a library, not a model.** The tree file this was
/// written for is eight species in a row spanning a hundred metres, each with
/// its own texture atlas — merging them gives one mesh that can carry one
/// material, so seven of the eight are wrong however it is shaded. Selecting
/// by name is what makes a downloaded pack usable without an authoring tool.
///
/// A scene names it with a fragment: `trees.obj#Oak_Leav`. That needs no
/// schema change, because an asset path is already a string, and it reads the
/// way a URL fragment reads — the part after the `#` names something inside
/// the thing before it.
///
/// # Errors
/// [`AssetError`] if the file cannot be read, or the named object does not
/// exist, or what was selected contains no triangles.
pub fn import_obj_object(
    path: &std::path::Path,
    object: Option<&str>,
) -> Result<Mesh, AssetError> {
    let text = std::fs::read_to_string(path).map_err(AssetError::Io)?;

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut mesh = Mesh {
        name: object.map_or_else(
            || path.file_stem().and_then(|s| s.to_str()).unwrap_or("mesh").to_owned(),
            std::borrow::ToOwned::to_owned,
        ),
        ..Mesh::default()
    };

    // OBJ indices are 1-based and may be negative, which means "counting back
    // from the end of what has been declared so far". Both appear in the wild.
    let resolve = |raw: i64, len: usize| -> Option<usize> {
        if raw > 0 {
            usize::try_from(raw - 1).ok().filter(|i| *i < len)
        } else if raw < 0 {
            len.checked_sub(raw.unsigned_abs() as usize)
        } else {
            None
        }
    };

    let three = |it: &mut std::str::SplitWhitespace<'_>| -> [f32; 3] {
        let mut out = [0.0_f32; 3];
        for slot in &mut out {
            *slot = it.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        }
        out
    };

    // **Vertex data is collected from the WHOLE file even when one object is
    // selected**, because OBJ indices are global: object seven's faces index
    // into the same `v` list object one contributed to. Skipping the vertices
    // of unselected objects would renumber everything after them.
    let mut wanted = object.is_none();
    let mut seen_any = false;

    for line in text.lines() {
        let line = line.trim();
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("o" | "g") => {
                if let Some(target) = object {
                    let name = parts.next().unwrap_or_default();
                    wanted = name == target;
                    seen_any |= wanted;
                }
            }
            Some("v") => positions.push(three(&mut parts)),
            Some("vn") => normals.push(three(&mut parts)),
            Some("vt") => {
                let u = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                let v: f32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
                // **Taken as written, and that is measured rather than
                // reasoned.** The textbook answer is that OBJ's V axis points
                // up while Vulkan's points down, so an importer should flip.
                // Flipping renders this pack's trees with bark on the leaves
                // and foliage on the trunk — its atlases carry leaf cards
                // above a bark region, so a vertical mirror swaps them and
                // says so loudly. Loom's texture upload already accounts for
                // the difference; a flip here applies it twice.
                uvs.push([u, v]);
            }
            Some("f") if wanted => {
                // `v`, `v/vt`, `v//vn` and `v/vt/vn` are all legal.
                let corners: Vec<(i64, Option<i64>, Option<i64>)> = parts
                    .map(|c| {
                        let mut f = c.split('/');
                        let v = f.next().and_then(|x| x.parse().ok()).unwrap_or(0);
                        let t = f.next().and_then(|x| x.parse().ok());
                        let n = f.next().and_then(|x| x.parse().ok());
                        (v, t, n)
                    })
                    .collect();
                if corners.len() < 3 {
                    continue;
                }
                for k in 1..corners.len() - 1 {
                    for corner in [corners[0], corners[k], corners[k + 1]] {
                        let Some(pi) = resolve(corner.0, positions.len()) else {
                            continue;
                        };
                        let uv = corner
                            .1
                            .and_then(|t| resolve(t, uvs.len()))
                            .map_or([0.0, 0.0], |i| uvs[i]);
                        let normal = corner
                            .2
                            .and_then(|n| resolve(n, normals.len()))
                            .map(|i| normals[i]);
                        let index = u32::try_from(mesh.vertices.len()).unwrap_or(0);
                        mesh.vertices.push(Vertex::with_uv(
                            positions[pi],
                            normal.unwrap_or([0.0, 0.0, 0.0]),
                            uv,
                        ));
                        mesh.indices.push(index);
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(target) = object.filter(|_| !seen_any) {
        return Err(AssetError::Unsupported(format!(
            "{} has no object named {target}",
            path.display()
        )));
    }
    if mesh.indices.is_empty() {
        return Err(AssetError::Unsupported(format!(
            "{} contains no triangles",
            path.display()
        )));
    }
    // Selected objects sit wherever they were authored — the tree file has
    // them in a row a hundred metres long — so a single one arrives with a
    // large offset baked in and a scene transform cannot sensibly place it.
    // Recentring on X and Z, but NOT on Y: a tree's base belongs at its
    // origin, and centring vertically would bury half of it.
    if object.is_some() {
        let (lo, hi) = mesh.bounds();
        let shift = [(lo[0] + hi[0]) * 0.5, lo[1], (lo[2] + hi[2]) * 0.5];
        for v in &mut mesh.vertices {
            v.position[0] -= shift[0];
            v.position[1] -= shift[1];
            v.position[2] -= shift[2];
        }
    }

    // Fill in any normal the file did not carry, per face. A zero normal is
    // the sentinel written above; a file with full normals never enters this.
    for tri in mesh.indices.chunks_exact(3) {
        let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        if mesh.vertices[a].normal[0] != 0.0
            || mesh.vertices[a].normal[1] != 0.0
            || mesh.vertices[a].normal[2] != 0.0
        {
            continue;
        }
        let p = |i: usize| mesh.vertices[i].position;
        let (pa, pb, pc) = (p(a), p(b), p(c));
        let u = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let v = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let n = if len > 1e-12 { [n[0] / len, n[1] / len, n[2] / len] } else { [0.0, 1.0, 0.0] };
        for i in [a, b, c] {
            mesh.vertices[i].normal = [n[0], n[1], n[2], 0.0];
        }
    }

    mesh.validate()?;
    Ok(mesh)
}

/// Import every static mesh in a glTF/GLB file, merged into one mesh.
///
/// Merged because a `MeshRenderer` names one mesh; splitting a multi-primitive
/// file into separate assets is the importer's job at the point where materials
/// exist to distinguish them, which is not yet.
///
/// # Errors
/// [`AssetError`] if the file cannot be read or contains no triangles.
pub fn import_gltf(path: &std::path::Path) -> Result<Mesh, AssetError> {
    let (document, buffers, _images) =
        gltf::import(path).map_err(|e| AssetError::Gltf(e.to_string()))?;

    let mut mesh = Mesh {
        name: path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("mesh")
            .to_owned(),
        ..Mesh::default()
    };

    for gltf_mesh in document.meshes() {
        for primitive in gltf_mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                // Points and lines are legal glTF and meaningless to a
                // triangle renderer. Skipped, not an error.
                continue;
            }
            let reader = primitive.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(positions) = reader.read_positions() else {
                continue;
            };
            let positions: Vec<[f32; 3]> = positions.collect();

            // A glTF file may omit normals; the spec says to compute flat
            // normals in that case, so a missing-normals file must still
            // render rather than being rejected.
            let normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map_or_else(|| vec![[0.0, 1.0, 0.0]; positions.len()], Iterator::collect);

            // UV set 0. A glTF may carry several, and may carry none at all —
            // an untextured mesh legitimately has no unwrap. Missing UVs
            // become `[0, 0]`, which a material can still handle by asking
            // for triplanar projection instead.
            let uvs: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map_or_else(Vec::new, |t| t.into_f32().collect());

            let base = u32::try_from(mesh.vertices.len()).unwrap_or(0);
            for (i, position) in positions.iter().enumerate() {
                mesh.vertices.push(Vertex::with_uv(
                    *position,
                    normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]),
                    uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                ));
            }

            match reader.read_indices() {
                Some(indices) => mesh
                    .indices
                    .extend(indices.into_u32().map(|i| i + base)),
                // Non-indexed primitives are legal: the vertices are the
                // triangle list.
                None => mesh
                    .indices
                    .extend((0..positions.len()).map(|i| base + u32::try_from(i).unwrap_or(0))),
            }
        }
    }

    if mesh.indices.is_empty() {
        return Err(AssetError::Unsupported(format!(
            "{}: no triangles",
            path.display()
        )));
    }
    mesh.validate()?;
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_out_of_range_index_is_rejected() {
        let mesh = Mesh {
            name: "bad".into(),
            vertices: vec![Vertex::default(); 3],
            indices: vec![0, 1, 7],
        };

        let err = mesh.validate().expect_err("index 7 of 3 vertices");
        assert!(format!("{err}").contains("addresses vertex 7"));
    }

    #[test]
    fn a_partial_triangle_is_rejected() {
        let mesh = Mesh {
            name: "bad".into(),
            vertices: vec![Vertex::default(); 3],
            indices: vec![0, 1],
        };

        assert!(mesh.validate().is_err(), "2 indices is not a triangle");
    }

    /// This type is CPU-side authoring data and no longer reaches the GPU —
    /// [`crate::PackedVertex`] does, and carries the std430 constraint that
    /// used to live here. What is still worth pinning is the field order,
    /// because `renderer::combine` and the ray-tracing position buffer both
    /// read `position` as the leading three floats.
    #[test]
    fn a_vertex_leads_with_its_position() {
        let v = Vertex::with_uv([1.0, 2.0, 3.0], [0.0, 1.0, 0.0], [0.25, 0.75]);

        assert_eq!(std::mem::offset_of!(Vertex, position), 0);
        assert_eq!(v.position[..3], [1.0, 2.0, 3.0]);
        assert_eq!(v.uv, [0.25, 0.75]);
    }
}

#[cfg(test)]
mod gltf_tests {
    use super::*;

    /// A real glTF file, imported end to end. The fixture is a pyramid — a
    /// shape no primitive produces — so a passing render proves the geometry
    /// came from the file rather than from the primitive library.
    #[test]
    fn imports_a_gltf_file() {
        let path = std::path::Path::new("../../assets/test/gltf/pyramid.gltf");
        let mesh = import_gltf(path).expect("fixture should import");

        assert_eq!(mesh.name, "pyramid");
        assert_eq!(mesh.indices.len(), 18, "6 triangles");
        mesh.validate().expect("imported mesh must be well-formed");

        let (min, max) = mesh.bounds();
        assert!((max[1] - 1.2).abs() < 1e-5, "apex at {}", max[1]);
        assert!((min[1] - -0.6).abs() < 1e-5, "base at {}", min[1]);
    }
}
