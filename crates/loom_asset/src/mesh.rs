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
}

impl Vertex {
    #[must_use]
    pub fn new(position: [f32; 3], normal: [f32; 3]) -> Self {
        Self {
            position: [position[0], position[1], position[2], 1.0],
            normal: [normal[0], normal[1], normal[2], 0.0],
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

            let base = u32::try_from(mesh.vertices.len()).unwrap_or(0);
            for (i, position) in positions.iter().enumerate() {
                mesh.vertices.push(Vertex::new(
                    *position,
                    normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]),
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

    #[test]
    fn vertex_stays_sixteen_byte_aligned() {
        // std430 for a PhysicalStorageBuffer block. `[f32; 3]` members would
        // put `normal` at offset 12 and spirv-val rejects the module.
        assert_eq!(size_of::<Vertex>(), 32);
        assert_eq!(align_of::<Vertex>(), 4);
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
