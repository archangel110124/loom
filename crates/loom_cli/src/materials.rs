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

            let mut map = |name: &str, space: loom_asset::ColorSpace| -> u32 {
                let alias = field(name)
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
            let albedo_map = map("albedo_map", loom_asset::ColorSpace::Srgb);
            let normal_map = map("normal_map", loom_asset::ColorSpace::Linear);

            let albedo = vector("albedo", [0.8; 3]);
            let uv_scale = {
                let pair = vector("uv_scale", [1.0, 1.0, 0.0]);
                [pair[0], pair[1]]
            };
            let triplanar = field("triplanar")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

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
                    0,
                ],
            });
            library.by_entity.insert(index, slot);
        }

        library
    }
}
