//! Deterministic CPU particles.
//!
//! # Why these are simulation state and not a visual side-channel
//!
//! Most engines treat particles as decoration: seeded from the wall clock,
//! stepped by whatever frame time happened, excluded from any notion of
//! reproducibility. That is a reasonable trade when a human is watching.
//!
//! It is the wrong trade here. Two of this project's three properties are that
//! the agent can test its own work and that the runtime is deterministic, and
//! `loom sim --assert` is what makes the first true. A subsystem that produced
//! different output every run would either have to be excluded from those
//! assertions — leaving a hole in exactly the visual layer an agent is least
//! able to eyeball — or would poison them.
//!
//! So particles are ordinary simulation state: a seed authored in the scene, a
//! fixed timestep, and an ordered `Vec` rather than a map (never-do #7). Same
//! scene plus same tick count gives the same particles, on any machine, every
//! time. The cost is one seeded PRNG.
//!
//! # What this is not
//!
//! There is no GPU simulation here. The Vulkan doc lists particle update as a
//! candidate for async compute, and at a million particles that is right. At
//! the scale a scene actually authors — thousands — the CPU cost is far below
//! the cost of *drawing* them, which is overdraw-bound, so moving the update to
//! the GPU would optimise the cheap half and give up determinism to do it.

use loom_terrain::noise;

/// A live particle.
///
/// Deliberately plain data in a flat `Vec`: the update is a linear pass, and
/// the render path wants it contiguous to sort and upload.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    /// Seconds lived so far.
    pub age: f32,
    /// Seconds this one gets. Varied per particle so a burst does not
    /// evaporate all at once, which reads as a light switching off.
    pub lifetime: f32,
    /// Per-particle random constant, for anything that should differ between
    /// two particles at the same place — rotation, texture phase, size jitter.
    pub jitter: f32,
}

impl Particle {
    /// How far through its life this particle is, `0.0` to `1.0`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.lifetime <= 0.0 {
            return 1.0;
        }
        (self.age / self.lifetime).clamp(0.0, 1.0)
    }
}

/// Everything an emitter needs, mirroring the `ParticleEmitter` component.
///
/// Split from the component so this crate depends on nothing in the workspace
/// but `loom_terrain`, and so the simulation can be tested without a scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emitter {
    pub rate: f32,
    pub lifetime: f32,
    pub lifetime_jitter: f32,
    pub speed: f32,
    pub spread_degrees: f32,
    pub radius: f32,
    /// Metres per second squared along Y. **Positive rises**: smoke is hotter
    /// than the air around it, and buoyancy is the whole reason a plume goes
    /// up rather than falling like a spark.
    pub gravity: f32,
    /// Fraction of velocity shed per second.
    pub drag: f32,
    /// Strength of the swirl applied per second.
    pub turbulence: f32,
    /// Spatial scale of that swirl, in cycles per metre.
    pub turbulence_scale: f32,
    pub seed: u64,
}

impl Default for Emitter {
    fn default() -> Self {
        Self {
            rate: 40.0,
            lifetime: 3.0,
            lifetime_jitter: 0.35,
            speed: 2.0,
            spread_degrees: 18.0,
            radius: 0.25,
            gravity: 1.2,
            drag: 0.8,
            turbulence: 1.4,
            turbulence_scale: 0.35,
            seed: 1,
        }
    }
}

/// One emitter's live particles.
pub struct System {
    particles: Vec<Particle>,
    rng: noise::Rng,
    /// Fractional particles owed from previous steps.
    ///
    /// Without this a rate that is not a whole number of particles per tick
    /// rounds to zero or to one every tick, and the plume either never starts
    /// or pulses. Carrying the remainder makes the *average* rate correct while
    /// each tick still spawns whole particles.
    owed: f32,
    /// Ticks stepped. Part of the state, so it is reproducible — never a clock
    /// reading (never-do #8).
    ticks: u64,
}

impl System {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            particles: Vec::new(),
            // Offset so two emitters that share a scene seed but differ in
            // their own still diverge.
            rng: noise::Rng::new(seed ^ 0x9E37_79B9_7F4A_7C15),
            owed: 0.0,
            ticks: 0,
        }
    }

    #[must_use]
    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    #[must_use]
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// Advance by one fixed step.
    ///
    /// `dt` is the simulation's fixed timestep, never a measured frame time —
    /// passing a variable `dt` here would make the result depend on how fast
    /// the machine happened to run, which is the failure the fixed timestep
    /// exists to prevent.
    pub fn step(&mut self, dt: f32, emitter: &Emitter, origin: [f32; 3]) {
        self.retire();
        self.advance(dt, emitter);
        self.spawn(dt, emitter, origin);
        self.ticks += 1;
    }

    /// Drop dead particles, preserving the order of the living.
    ///
    /// `retain` rather than swap-remove: swapping would reorder the survivors,
    /// and while that does not change what is drawn, it changes the *order* it
    /// is drawn in — which for alpha-blended particles is visible, and would
    /// make two runs of the same scene differ pixel for pixel.
    fn retire(&mut self) {
        self.particles.retain(|p| p.age < p.lifetime);
    }

    fn advance(&mut self, dt: f32, emitter: &Emitter) {
        // Shed a fixed fraction of velocity per second. Clamped because a drag
        // above 1/dt would otherwise reverse the velocity rather than damp it.
        let damping = (1.0 - emitter.drag * dt).clamp(0.0, 1.0);
        let scale = emitter.turbulence_scale;

        for p in &mut self.particles {
            // Swirl from a divergence-free-ish pair of noise samples. Real curl
            // noise takes derivatives of a potential field; this is the cheap
            // stand-in, and at smoke scale the difference is not visible.
            //
            // Sampled by *position*, so two particles in the same place are
            // pushed the same way and the plume moves as a body rather than as
            // a cloud of independent specks.
            let swirl = |a: f32, b: f32, s: u64| {
                noise::value(a * scale, b * scale, emitter.seed ^ s) * 2.0 - 1.0
            };
            let turbulence = emitter.turbulence * dt;
            p.velocity[0] += swirl(p.position[1], p.position[2], 0x11) * turbulence;
            p.velocity[1] += swirl(p.position[2], p.position[0], 0x22) * turbulence * 0.4;
            p.velocity[2] += swirl(p.position[0], p.position[1], 0x33) * turbulence;

            p.velocity[1] += emitter.gravity * dt;
            for axis in 0..3 {
                p.velocity[axis] *= damping;
                p.position[axis] += p.velocity[axis] * dt;
            }
            p.age += dt;
        }
    }

    fn spawn(&mut self, dt: f32, emitter: &Emitter, origin: [f32; 3]) {
        self.owed += emitter.rate.max(0.0) * dt;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let count = self.owed as u32;
        self.owed -= f32::from(u16::try_from(count).unwrap_or(u16::MAX));

        for _ in 0..count {
            let mut unit = || self.rng.next_f32();
            // Spawn disc, sampled with a square root so particles spread evenly
            // over the *area*. Without it they bunch at the centre, which reads
            // as a thin stalk rather than a fire's footprint.
            let angle = unit() * std::f32::consts::TAU;
            let r = emitter.radius * unit().sqrt();
            let position = [
                origin[0] + angle.cos() * r,
                origin[1],
                origin[2] + angle.sin() * r,
            ];

            // Direction inside a cone about +Y.
            let spread = emitter.spread_degrees.to_radians();
            let theta = unit() * std::f32::consts::TAU;
            let phi = spread * unit().sqrt();
            let (sp, cp) = phi.sin_cos();
            let speed = emitter.speed * (0.7 + 0.6 * unit());

            let jitter = unit();
            self.particles.push(Particle {
                position,
                velocity: [sp * theta.cos() * speed, cp * speed, sp * theta.sin() * speed],
                age: 0.0,
                lifetime: (emitter.lifetime
                    * (1.0 + emitter.lifetime_jitter * (unit() * 2.0 - 1.0)))
                    .max(0.01),
                jitter,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(seed: u64, steps: u32) -> Vec<Particle> {
        let emitter = Emitter { seed, ..Emitter::default() };
        let mut system = System::new(seed);
        for _ in 0..steps {
            system.step(1.0 / 60.0, &emitter, [0.0, 0.0, 0.0]);
        }
        system.particles().to_vec()
    }

    /// **The property the whole design is for.** Same seed, same steps, same
    /// particles — bit for bit, not approximately. If this fails, particles
    /// cannot appear in a `loom sim --assert` hash and the visual layer stops
    /// being something the agent can make assertions about.
    #[test]
    fn the_same_seed_gives_the_same_particles() {
        let a = run(0xC0FFEE, 120);
        let b = run(0xC0FFEE, 120);

        assert!(!a.is_empty(), "120 ticks at 40/s should have spawned some");
        assert_eq!(a, b);
    }

    #[test]
    fn a_different_seed_gives_different_particles() {
        let a = run(1, 90);
        let b = run(2, 90);

        assert_ne!(a, b, "two seeds produced identical plumes");
    }

    /// A plume reaches a steady population: spawn rate times lifetime. If
    /// retirement were broken this would grow without bound, which is the kind
    /// of leak that only shows up after a long run.
    #[test]
    fn the_population_settles_at_rate_times_lifetime() {
        let emitter = Emitter { rate: 60.0, lifetime: 2.0, lifetime_jitter: 0.0, ..Emitter::default() };
        let mut system = System::new(7);
        for _ in 0..600 {
            system.step(1.0 / 60.0, &emitter, [0.0; 3]);
        }

        let expected = 60.0 * 2.0;
        let actual = system.particles().len() as f32;
        assert!(
            (actual - expected).abs() <= 2.0,
            "expected about {expected} particles, found {actual}"
        );
    }

    /// A fractional per-tick rate must still average out. At 1/60 s and 40
    /// particles a second that is 0.667 per tick, which truncates to zero every
    /// tick without the carried remainder.
    #[test]
    fn a_fractional_rate_still_spawns() {
        let emitter = Emitter { rate: 40.0, lifetime: 100.0, lifetime_jitter: 0.0, ..Emitter::default() };
        let mut system = System::new(3);
        for _ in 0..60 {
            system.step(1.0 / 60.0, &emitter, [0.0; 3]);
        }

        let count = system.particles().len();
        assert!((38..=42).contains(&count), "one second at 40/s gave {count}");
    }

    /// Buoyancy is what makes smoke smoke. A positive gravity must carry the
    /// plume upward on average.
    #[test]
    fn a_buoyant_plume_rises() {
        let emitter = Emitter { gravity: 2.0, turbulence: 0.0, ..Emitter::default() };
        let mut system = System::new(11);
        for _ in 0..180 {
            system.step(1.0 / 60.0, &emitter, [0.0; 3]);
        }

        let highest = system
            .particles()
            .iter()
            .fold(f32::MIN, |acc, p| acc.max(p.position[1]));
        assert!(highest > 1.0, "plume only reached {highest} m");
    }
}
