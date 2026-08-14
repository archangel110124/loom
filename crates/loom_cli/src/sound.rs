//! Scene sounds, heard from wherever the listener is standing.
//!
//! The join between three things that were built separately: `AudioSource`
//! nodes in a scene, the acoustics solver that measures what a place does to a
//! sound, and the mixer that turns that into samples.
//!
//! **Solved on a budget.** A full acoustic solve is 24 occlusion rays plus 64
//! room rays, and doing that for every source every tick is not affordable.
//! The room half is the same for every sound at a given listener position, so
//! it is solved once per tick and shared; the occlusion half is per source and
//! is spread across ticks, one source at a time. A sound's occlusion is
//! therefore up to a few frames stale, which is inaudible — walls do not move
//! that fast — and it turns an O(sources) cost into a constant one.

use loom_audio::{Acoustics, Audio, Clip, Ears, Voice};
use loom_ecs::World;

/// One playing source, and what the scene said about it.
struct Source {
    entity: loom_ecs::Entity,
    at: [f32; 3],
    range: f32,
    /// Last solve, reused on the ticks this source is not the one being
    /// re-measured.
    acoustics: Acoustics,
}

/// Everything audible in a scene.
pub(crate) struct Sound {
    audio: Audio,
    sources: Vec<Source>,
    /// Which source gets a fresh occlusion solve this tick.
    next: usize,
    ears: Ears,
    /// The scene's unsheltered rain rate in mm/h. Zero in a dry scene, and then
    /// the bed renders exact silence.
    rain: f32,
}

impl Sound {
    /// Open the device and start every source the scene autoplays.
    ///
    /// Returns `None` when there is no sound card, which is an ordinary thing
    /// to meet — headless CI is one. A scene must still run.
    pub(crate) fn start(world: &World, base: &std::path::Path, scene: &loom_scene::Scene) -> Option<Self> {
        let audio = match Audio::open() {
            Ok(audio) => audio,
            Err(e) => {
                crate::log::warn(format!("no audio ({e}); continuing silent"));
                return None;
            }
        };
        crate::log::info(format!("audio — {}", audio.device_name()));

        let mut sound = Self {
            audio,
            sources: Vec::new(),
            next: 0,
            ears: Ears::default(),
            // The rate the sky is producing, before any shelter. Shelter is
            // applied per tick from the room solve, exactly as the visible
            // layer applies it per drop rather than per camera.
            rain: crate::weather::rain_of(scene).map_or(0.0, |r| r.intensity),
        };

        for entity in world.entities() {
            let (Some(component), Some(global)) =
                (world.audio(*entity), world.global_transform(*entity))
            else {
                continue;
            };
            let at = [global.matrix[12], global.matrix[13], global.matrix[14]];
            sound.add(component, scene, base, *entity, at);
        }
        Some(sound)
    }

    fn add(
        &mut self,
        component: &serde_json::Value,
        scene: &loom_scene::Scene,
        base: &std::path::Path,
        entity: loom_ecs::Entity,
        at: [f32; 3],
    ) {
        let defaults = loom_scene::components::AudioSource::default();
        #[allow(clippy::cast_possible_truncation)]
        let scalar = |name: &str, fallback: f32| {
            component
                .get(name)
                .and_then(serde_json::Value::as_f64)
                .map_or(fallback, |v| v as f32)
        };
        let flag = |name: &str, fallback: bool| {
            component
                .get(name)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(fallback)
        };

        let Some(alias) = component
            .get("clip")
            .and_then(|c| c.get("asset"))
            .and_then(serde_json::Value::as_str)
            .filter(|a| !a.is_empty())
        else {
            return;
        };
        // A clip that will not load is a warning, not a failure — the same
        // reasoning as a missing texture. A scene half-authored should still
        // run and show what is wrong.
        let Some(path) = scene.asset_path(alias) else {
            crate::log::warn(format!("audio clip `{alias}` is not declared as an asset"));
            return;
        };
        let clip = match Clip::load(&base.join(path)) {
            Ok(clip) => clip,
            Err(e) => {
                crate::log::warn(format!("audio clip `{alias}` did not load: {e}"));
                return;
            }
        };

        let range = scalar("range", defaults.range);
        if flag("autoplay", defaults.autoplay) {
            let mut voice = Voice::new(
                std::sync::Arc::new(clip.samples),
                clip.sample_rate,
                self.audio.sample_rate(),
                scalar("volume", defaults.volume),
                range,
            );
            voice.looping = flag("looping", defaults.looping);
            self.audio.play(voice);
        }

        self.sources.push(Source {
            entity,
            at,
            range,
            acoustics: Acoustics {
                occlusion: 0.0,
                muffling: 0.0,
                reverb_delay: 0.0,
                reverb_gain: 0.0,
                openness: 1.0,
                distance: 0.0,
            },
        });
    }

    /// Re-measure and hand every voice its position for this tick.
    ///
    /// `submerged` is whether the listener's head is under the water — asked of
    /// the simulation, which owns the surface, rather than worked out here.
    pub(crate) fn update(
        &mut self,
        world: &World,
        physics: &loom_physics::Physics,
        listener: [f32; 3],
        right: [f32; 3],
        forward: [f32; 3],
        submerged: bool,
    ) {
        // The room, once. It is a property of where the listener stands, not
        // of any one sound, so solving it per source would be the same answer
        // computed over and over.
        //
        // **Solved before the early return, because rain needs it.** This used
        // to sit below a `sources.is_empty()` bail, and a scene whose only
        // sound is the weather has no sources at all — the most obviously
        // rainy scene would have been the one that never solved its room.
        let room = Acoustics::solve(physics, listener, listener, &self.ears);

        // The weather. `room.openness` rather than S3's sky exposure: see
        // `RainAudio::openness` — since ADR 0017 the visible rain is occluded
        // by the collision world too, so these agree, and S3 would read a mesh
        // roof as open sky.
        //
        // Underwater, the sky is not what you are listening through. The rain
        // is above the surface and you are not.
        self.audio.set_rain(loom_audio::rain::RainAudio {
            intensity: self.rain,
            openness: if submerged { 0.0 } else { room.openness },
            volume: 1.0,
        });

        if self.sources.is_empty() {
            return;
        }

        // One source's occlusion per tick, round-robin.
        let index = self.next % self.sources.len();
        self.next = self.next.wrapping_add(1);
        {
            let source = &self.sources[index];
            let solved = Acoustics::solve(physics, listener, source.at, &self.ears);
            self.sources[index].acoustics = solved;
        }

        for source in &mut self.sources {
            // Follow the node, so a sound on a moving thing moves with it.
            if let Some(global) = world.global_transform(source.entity) {
                source.at = [global.matrix[12], global.matrix[13], global.matrix[14]];
            }
            source.acoustics.reverb_delay = room.reverb_delay;
            source.acoustics.reverb_gain = room.reverb_gain;
            source.acoustics.openness = room.openness;
            // **A property of the listener, applied per source, here.** It goes
            // on last so it can only ever muffle further, and it goes on every
            // source at once because it is the ears that are under the water,
            // not any one sound.
            if submerged {
                source.acoustics = source.acoustics.underwater();
            }
        }

        let sources = &self.sources;
        self.audio.update(|voice_index, voice| {
            let Some(source) = sources.get(voice_index) else {
                return;
            };
            let offset = [
                source.at[0] - listener[0],
                source.at[1] - listener[1],
                source.at[2] - listener[2],
            ];
            let distance = offset.iter().map(|c| c * c).sum::<f32>().sqrt();
            // Into the listener's own frame: right, up, forward. A world-space
            // offset would pan by compass bearing and ignore which way the
            // player is facing.
            voice.relative = [
                dot(offset, right),
                offset[1],
                dot(offset, forward),
            ];
            voice.range = source.range;
            voice.acoustics = source.acoustics;
            voice.acoustics.distance = distance;
        });
    }

    pub(crate) fn silence(&self) {
        self.audio.silence();
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
