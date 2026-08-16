//! Turning authored `Material` components into what the GPU reads.
//!
//! The scene names textures by `[[asset]]` alias; the GPU names them by index
//! into one bindless array. This is the translation, and it is the only place
//! that knows both spellings.
//!
//! **A texture that will not load is a warning, not a failure.** The same
//! reasoning as a missing mesh (design doc §2.6): an agent halfway through
//! authoring a scene has broken references all the time, and a render that
//! refuses to run tells it far less than a render where the surface is
//! visibly untextured.

use std::collections::BTreeMap;

use loom_ecs::World;
use loom_render::{FLAG_TRIPLANAR, MaterialData, NO_TEXTURE};
use loom_scene::Scene;

/// Every texture and material a scene needs, plus which entity uses which.
#[derive(Default)]
pub(crate) struct MaterialLibrary {
    pub(crate) textures: Vec<loom_asset::Texture>,
    pub(crate) materials: Vec<MaterialData>,
    /// Entity index (position in `world.entities()`) to material index.
    by_entity: BTreeMap<usize, u32>,
    /// Aliases that did not load, so the caller can report them.
    pub(crate) missing: Vec<String>,
    /// Alias to bindless slot, for the handful of textures that are named
    /// rather than reached through a `Material`.
    pub(crate) by_alias: BTreeMap<String, u32>,
}

/// The average linear colour of a bindless slot, or white for [`NO_TEXTURE`].
///
/// White rather than black is what keeps an untextured material's reflection
/// colour bit-identical to its authored `albedo`: multiplying by exactly 1.0
/// is exact.
fn mean_of(textures: &[loom_asset::Texture], slot: u32) -> [f32; 3] {
    textures
        .get(slot as usize)
        .map_or([1.0; 3], loom_asset::Texture::mean_linear)
}

impl MaterialLibrary {
    /// The material index for an entity, or [`NO_TEXTURE`] for "no material",
    /// which leaves the object on its debug palette colour.
    pub(crate) fn index_for(&self, entity_index: usize) -> u32 {
        self.by_entity.get(&entity_index).copied().unwrap_or(NO_TEXTURE)
    }

    /// Resolve every `Material` a scene declares.
    pub(crate) fn for_scene(world: &World, scene: &Scene, base: &std::path::Path) -> Self {
        let mut library = Self::default();
        // Alias to slot, so two materials naming the same texture upload it
        // once. A scene that dresses forty crates in one texture should cost
        // one image, not forty.
        let mut slots: BTreeMap<String, u32> = BTreeMap::new();

        for (index, entity) in world.entities().iter().enumerate() {
            let Some(component) = world.material(*entity) else {
                continue;
            };

            let field = |name: &str| component.get(name);
            let scalar = |name: &str, fallback: f32| {
                #[allow(clippy::cast_possible_truncation)]
                field(name)
                    .and_then(serde_json::Value::as_f64)
                    .map_or(fallback, |v| v as f32)
            };
            let vector = |name: &str, fallback: [f32; 3]| {
                let Some(values) = field(name).and_then(serde_json::Value::as_array) else {
                    return fallback;
                };
                let mut out = fallback;
                for (slot, value) in out.iter_mut().zip(values) {
                    if let Some(v) = value.as_f64() {
                        #[allow(clippy::cast_possible_truncation)]
                        {
                            *slot = v as f32;
                        }
                    }
                }
                out
            };

            // Takes the reference itself rather than a field name, so the
            // steep-slope layer's nested `albedo_map` goes through exactly the
            // same load, cache and missing-alias reporting as the parent's.
            let mut map = |slot_ref: Option<&serde_json::Value>,
                           space: loom_asset::ColorSpace|
             -> u32 {
                let alias = slot_ref
                    .and_then(|m| m.get("asset"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|a| !a.is_empty());
                let Some(alias) = alias else {
                    return NO_TEXTURE;
                };
                if let Some(slot) = slots.get(alias) {
                    return *slot;
                }
                let Some(path) = scene.asset_path(alias) else {
                    library.missing.push(alias.to_owned());
                    return NO_TEXTURE;
                };
                match loom_asset::texture::load(&base.join(path), space) {
                    Ok(texture) => {
                        let slot = u32::try_from(library.textures.len()).unwrap_or(0);
                        library.textures.push(texture);
                        slots.insert(alias.to_owned(), slot);
                        library.by_alias.insert(alias.to_owned(), slot);
                        slot
                    }
                    Err(_) => {
                        library.missing.push(alias.to_owned());
                        NO_TEXTURE
                    }
                }
            };

            // sRGB for colour, linear for normals. Getting this backwards runs
            // the gamma curve over a set of vectors, which tilts every surface
            // toward the texture's brighter channels.
            let albedo_map = map(field("albedo_map"), loom_asset::ColorSpace::Srgb);
            let normal_map = map(field("normal_map"), loom_asset::ColorSpace::Linear);

            let albedo = vector("albedo", [0.8; 3]);
            let uv_scale = {
                let pair = vector("uv_scale", [1.0, 1.0, 0.0]);
                [pair[0], pair[1]]
            };
            let triplanar = field("triplanar")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            // **The steep-slope layer, pushed BEFORE its parent** so the
            // parent can name a slot that already exists. It is a full
            // `MaterialData`, not a colour: the two must be able to differ in
            // roughness and porosity or they read as one substance painted
            // twice.
            let layer_slot: Option<u32> = match field("layer") {
                Some(v) if !v.is_null() => {
                    let lf = |name: &str| v.get(name);
                    let lvec = |name: &str, fallback: [f32; 3]| {
                        lf(name)
                            .and_then(serde_json::Value::as_array)
                            .map(|a| {
                                let mut out = fallback;
                                for (slot, item) in out.iter_mut().zip(a) {
                                    if let Some(n) = item.as_f64() {
                                        *slot = n as f32;
                                    }
                                }
                                out
                            })
                            .unwrap_or(fallback)
                    };
                    let lscalar = |name: &str, fallback: f32| {
                        lf(name).and_then(serde_json::Value::as_f64).map_or(fallback, |n| n as f32)
                    };
                    let la = lvec("albedo", [0.8; 3]);
                    let luv = lvec("uv_scale", [1.0, 1.0, 0.0]);
                    // **Hoisted out of the struct literal below**, because the
                    // mean has to read `library.textures` and `map` borrows it.
                    let layer_albedo_map = map(lf("albedo_map"), loom_asset::ColorSpace::Srgb);
                    let layer_normal_map = map(lf("normal_map"), loom_asset::ColorSpace::Linear);
                    let lmean = mean_of(&library.textures, layer_albedo_map);
                    let slot = u32::try_from(library.materials.len()).unwrap_or(0);
                    library.materials.push(MaterialData {
                        albedo: [la[0], la[1], la[2], lscalar("porosity", 0.15)],
                        // **The slope cosine rides in the layer's metallic
                        // lane.** Ground is not metal, the lane was already
                        // there, and the alternative is growing a struct whose
                        // layout is pinned by an offset test in two places.
                        params: [
                            lscalar("slope", 0.7),
                            lscalar("roughness", 0.8),
                            luv[0],
                            luv[1],
                        ],
                        maps: [
                            layer_albedo_map,
                            layer_normal_map,
                            // **Inherited, not defaulted, and this is the bug
                            // that ships a grey terrain.** The shader reads
                            // `FLAG_TRIPLANAR` from whichever record it is
                            // sampling, and every voxel vertex carries
                            // `uv = [0, 0]` — so a layer without the flag
                            // stretches one texel across the entire volume.
                            // It still moves the reference by more than the
                            // tolerance, so no golden gate would catch it.
                            if triplanar { FLAG_TRIPLANAR } else { 0 },
                            // A layer has no layer of its own.
                            NO_TEXTURE,
                        ],
                        mean_albedo: [la[0] * lmean[0], la[1] * lmean[1], la[2] * lmean[2], 0.0],
                    });
                    // The slope cosine rides in the parent's spare metallic
                    // lane below rather than growing the struct.
                    Some(slot)
                }
                _ => None,
            };

            // **What a ray-traced reflection shades with.** It has no UVs, so
            // it cannot fetch a texel; this is the texture's average colour
            // instead, tinted by the authored `albedo` so a material that
            // tints its map still reflects the tint.
            let mean = mean_of(&library.textures, albedo_map);
            let slot = u32::try_from(library.materials.len()).unwrap_or(0);
            library.materials.push(MaterialData {
                // `w` is porosity — how much this surface darkens when wet.
                // It rides in the albedo's spare lane rather than growing the
                // struct: `MaterialData` is one memory layout described twice
                // and the trailing scalar was already there for the taking.
                albedo: [albedo[0], albedo[1], albedo[2], scalar("porosity", 0.4)],
                params: [
                    scalar("metallic", 0.0),
                    scalar("roughness", 0.8),
                    uv_scale[0],
                    uv_scale[1],
                ],
                maps: [
                    albedo_map,
                    normal_map,
                    if triplanar { FLAG_TRIPLANAR } else { 0 },
                    layer_slot.unwrap_or(NO_TEXTURE),
                ],
                // `w` is the alpha-test threshold, riding in the lane that was
                // documented as unused. Zero means opaque, which is what every
                // material that does not ask for a cutout carries.
                mean_albedo: [
                    albedo[0] * mean[0],
                    albedo[1] * mean[1],
                    albedo[2] * mean[2],
                    scalar("alpha_cutoff", 0.0),
                ],
            });
            library.by_entity.insert(index, slot);
        }

        // **Textures nothing references still have to be loaded.** The bindless
        // array is filled by walking `Material` components, so an asset no
        // material names — the fire flipbook, which the particle shader reaches
        // by a reserved alias — is declared, validated, and never uploaded.
        // Silently: the render simply does not use it.
        for alias in ["fire_flipbook"] {
            if library.by_alias.contains_key(alias) {
                continue;
            }
            let Some(path) = scene.asset_path(alias) else {
                continue;
            };
            match loom_asset::texture::load(&base.join(path), loom_asset::ColorSpace::Srgb) {
                Ok(texture) => {
                    let slot = u32::try_from(library.textures.len()).unwrap_or(0);
                    library.textures.push(texture);
                    library.by_alias.insert((*alias).to_owned(), slot);
                }
                Err(_) => library.missing.push((*alias).to_owned()),
            }
        }

        library
    }
}

#[cfg(test)]
mod tests {
    use super::MaterialLibrary;
    use loom_ecs::World;
    use loom_render::{FLAG_TRIPLANAR, NO_TEXTURE};

    fn library(source: &str) -> MaterialLibrary {
        let scene = loom_scene::Scene::parse(source).expect("valid scene");
        let world = World::from_scene(&scene);
        // No `[[asset]]` entries, so no texture ever loads and the base path is
        // never read. What is under test is the *record*, not the image.
        MaterialLibrary::for_scene(&world, &scene, std::path::Path::new("."))
    }

    const WITH_LAYER: &str = r#"
[scene]
format = 1
id = "0f8c1d24-6b39-4e57-9a02-71d4e6835bca"

[[node]]
name = "Ground"

  [node.components.Material]
  albedo = [1.0, 1.0, 1.0]
  roughness = 0.95
  triplanar = true

  [node.components.Material.layer]
  roughness = 0.5
  slope = 0.55
"#;

    /// **The layer must inherit the parent's triplanar flag, and this is the
    /// bug that ships a grey terrain.**
    ///
    /// The shader reads `FLAG_TRIPLANAR` from whichever record it is sampling.
    /// Surface Nets emits no UVs, so every voxel vertex carries `uv = [0, 0]`
    /// — a layer without the flag therefore samples one texel and stretches it
    /// across the entire volume. Rendered, the hill becomes a featureless grey
    /// blob.
    ///
    /// Mutation: set the layer's flags to `0` in `for_scene` and this fails.
    /// Verified — and the render was kept, because a golden reference moves
    /// enormously under this and a human could bless it without knowing why.
    #[test]
    fn the_layer_inherits_the_parents_triplanar_flag() {
        let lib = library(WITH_LAYER);
        assert_eq!(lib.materials.len(), 2, "a layer must add a second record");
        let parent = lib.materials.iter().find(|m| m.maps[3] != NO_TEXTURE).expect("parent");
        let layer = &lib.materials[parent.maps[3] as usize];
        assert_eq!(
            layer.maps[2] & FLAG_TRIPLANAR,
            parent.maps[2] & FLAG_TRIPLANAR,
            "the layer would sample one texel across the whole volume"
        );
    }

    /// The parent names its layer, and the layer names none — a layer has no
    /// layer of its own, and a cycle would be an infinite regress in a shader.
    #[test]
    fn the_layer_index_points_at_the_layer_and_stops_there() {
        let lib = library(WITH_LAYER);
        let parent = lib.materials.iter().find(|m| m.maps[3] != NO_TEXTURE).expect("parent");
        let layer = &lib.materials[parent.maps[3] as usize];
        assert_eq!(layer.maps[3], NO_TEXTURE, "a layer must not carry a layer");
        // The slope threshold rides in the layer's metallic lane; ground is not
        // metal and the lane was already there.
        assert!((layer.params[0] - 0.55).abs() < 1e-6, "slope lost: {}", layer.params[0]);
        assert!((layer.params[1] - 0.5).abs() < 1e-6, "roughness lost: {}", layer.params[1]);
    }

    /// **A material with no layer must produce exactly one record with the
    /// sentinel**, or every untextured surface in the project picks up material
    /// zero as its cliff face — `maps[3]`'s old default was `0`, which is a
    /// perfectly valid index.
    #[test]
    fn a_material_without_a_layer_names_no_layer() {
        let lib = library(
            r#"
[scene]
format = 1
id = "3a7e0c51-9d24-4b68-8f13-25c7e094ab6d"

[[node]]
name = "Plain"

  [node.components.Material]
  albedo = [0.5, 0.5, 0.5]
"#,
        );
        assert_eq!(lib.materials.len(), 1);
        assert_eq!(lib.materials[0].maps[3], NO_TEXTURE);
    }

    /// **An untextured material's reflection colour must not move at all.**
    /// `mean_albedo` is `albedo` times the texture's mean, and with no texture
    /// the mean is exactly 1.0 — so this is bit-identical, not merely close,
    /// and every scene in the project that reflects a plain-coloured surface
    /// renders byte for byte as it did.
    #[test]
    fn an_untextured_material_reflects_exactly_its_albedo() {
        let lib = library(
            r#"
[scene]
format = 1
id = "3a7e0c51-9d24-4b68-8f13-25c7e094ab6d"

[[node]]
name = "Plain"

  [node.components.Material]
  albedo = [0.3, 0.27, 0.24]
"#,
        );
        let m = &lib.materials[0];
        assert_eq!(m.mean_albedo[..3], m.albedo[..3], "bit-identical, not close");
    }

    /// **A material that authors no cutoff must carry exactly zero**, because
    /// zero is what the shader reads as "this surface is opaque, do not test
    /// alpha at all". Anything else turns every existing scene into an
    /// alpha-tested one against a texture whose alpha nobody authored.
    #[test]
    fn a_material_is_opaque_unless_it_asks_not_to_be() {
        let lib = library(
            r#"
[scene]
format = 1
id = "3a7e0c51-9d24-4b68-8f13-25c7e094ab6d"

[[node]]
name = "Plain"

  [node.components.Material]
  albedo = [0.5, 0.5, 0.5]
"#,
        );
        assert_eq!(lib.materials[0].mean_albedo[3], 0.0);
    }

    /// The authored cutoff has to reach the GPU record, in the lane the shader
    /// reads. It rides in `mean_albedo.w`, which was documented as unused.
    #[test]
    fn an_authored_alpha_cutoff_reaches_the_gpu_record() {
        let lib = library(
            r#"
[scene]
format = 1
id = "3a7e0c51-9d24-4b68-8f13-25c7e094ab6d"

[[node]]
name = "Leaf"

  [node.components.Material]
  albedo = [1.0, 1.0, 1.0]
  alpha_cutoff = 0.5
"#,
        );
        assert_eq!(lib.materials[0].mean_albedo[3], 0.5);
    }
}
