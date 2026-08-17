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
    /// How readily a particle is carried by the wind, per second.
    ///
    /// **Coupling, not a force.** A particle is pulled toward the air'''s
    /// velocity rather than pushed by a constant, which is what makes smoke
    /// end up travelling *with* the wind instead of accelerating forever. High
    /// values are smoke and dust, which match the air almost exactly; low
    /// values are embers and debris, which lag it.
    ///
    /// **Zero by default, deliberately.** Every scene authored before wind
    /// existed must render exactly as it did — the golden images include two
    /// particle scenes, and a non-zero default would silently move both.
    pub wind_response: f32,

    /// Particles released all at once when the emitter starts.
    ///
    /// **This is what makes an explosion expressible.** A rate is a tap; a
    /// blast is a single release of everything at the same instant, and no
    /// number of particles per second is that.
    pub burst: u32,
    /// Seconds before this emitter does anything.
    ///
    /// Lets one scene stage an event out of several emitters: an explosion is
    /// a fire flash now and smoke a beat later, and the delay is what puts
    /// them in that order without any notion of a timeline.
    pub delay: f32,
    /// Seconds of continuous emission after `delay`. Zero means forever.
    ///
    /// Forever is right for a chimney and wrong for a blast, where the fire
    /// feeds smoke for a moment and then stops. Without this an explosion
    /// either puffs once or smoulders for the rest of the scene.
    pub duration: f32,
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
            wind_response: 0.0,
            burst: 0,
            delay: 0.0,
            duration: 0.0,
            seed: 1,
        }
    }
}

/// How many particles this emitter has ever released by the end of tick
/// `tick`, counting from zero.
///
/// **The CPU twin of the GPU pool's spawn ordinal** (ADR 0047). A GPU emitter
/// has no free list and no atomic: slot `i` owns the largest birth ordinal
/// `n < births_by(tick)` with `n ≡ i (mod N)`, and that is the whole of its
/// allocation. So this number is not bookkeeping — it *is* the allocator, and
/// it has to mean the same thing on both sides of the device boundary.
///
/// Written as a closed form rather than as an accumulator, and both halves of
/// that matter:
///
/// - The GPU cannot carry [`System::owed`]'s residual across a dispatch
///   boundary without making the answer depend on how the ticks were grouped,
///   which is exactly the property `--sim N` in one dispatch must not have.
/// - `floor(rate * n * dt)` is a multiply and a floor, both correctly rounded
///   in IEEE-754, so the CPU and the GPU compute it bit-identically from the
///   same inputs. An accumulated sum would not: float addition is not
///   associative and the drift is real (see the test below, which measures it).
///
/// `dt` is the fixed timestep, never a measured frame time.
#[must_use]
pub fn births_by(emitter: &Emitter, tick: u64, dt: f32) -> u32 {
    if dt <= 0.0 {
        return 0;
    }
    // The first step whose age has reached the delay. `System::spawn` compares
    // `ticks as f32 * dt` against `delay`, so this is the same boundary.
    let first = (emitter.delay.max(0.0) / dt).ceil();
    #[allow(clippy::cast_precision_loss)]
    let ticks = tick as f32;
    if ticks <= first {
        // Step `first` has not run yet: nothing has been released at all, not
        // even the burst.
        return 0;
    }

    // Steps that were allowed to emit continuously. A `duration` of zero is
    // "forever" — never "emit nothing" — which is the same asymmetry
    // `System::spawn` documents.
    let last = if emitter.duration > 0.0 {
        ticks.min(((emitter.delay.max(0.0) + emitter.duration) / dt).ceil())
    } else {
        ticks
    };
    let emitting = (last - first).max(0.0);
    let continuous = (emitter.rate.max(0.0) * (emitting * dt)).floor();

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let continuous = continuous.clamp(0.0, f32::from(u16::MAX) * 64.0) as u32;
    emitter.burst.saturating_add(continuous)
}

/// Slots a GPU pool needs so that a slot is never reused before its occupant
/// has died.
///
/// **The capacity rule, and it is a correctness rule rather than a budget.**
/// Ordinal `n` and ordinal `n + N` share a slot, and they are born
/// `N / rate` seconds apart. A particle lives at most
/// `lifetime * (1 + lifetime_jitter)`. So the pool laps — a live particle
/// being overwritten by its successor, which reads as blades of the plume
/// blinking out — unless `N / rate >= max lifetime`. The burst is additive
/// because its ordinals are all born at the same instant and hold their slots
/// for a full lifetime while continuous emission carries on behind them.
///
/// Returned rather than clamped, so the load-time validator can put the
/// required number in its error instead of silently drawing a lapping plume.
#[must_use]
pub fn pool_size(emitter: &Emitter) -> u32 {
    let live = emitter.rate.max(0.0) * emitter.lifetime.max(0.0) * (1.0 + emitter.lifetime_jitter.max(0.0));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let live = live.ceil().clamp(0.0, 4_294_000_000.0) as u32;
    emitter.burst.saturating_add(live).max(1)
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
    /// Whether the one-shot burst has been released.
    ///
    /// A flag rather than a time comparison: the burst has to fire exactly
    /// once, and "is the age past the delay" is true on every step after it.
    burst_done: bool,
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
            burst_done: false,
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
        self.step_in_wind(dt, emitter, origin, &|_| [0.0, 0.0, 0.0]);
    }

    /// Step with air movement.
    ///
    /// `wind` is sampled **per particle position**, not once per emitter: a
    /// plume tall enough to see is tall enough to sit in different air at its
    /// top than at its base, and sampling once would make the whole column
    /// slide sideways as a rigid body. That is the same unison artifact the
    /// turbulence term exists to break, one level up.
    pub fn step_in_wind(
        &mut self,
        dt: f32,
        emitter: &Emitter,
        origin: [f32; 3],
        wind: &dyn Fn([f32; 3]) -> [f32; 3],
    ) {
        self.retire();
        self.advance(dt, emitter, wind);
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

    fn advance(&mut self, dt: f32, emitter: &Emitter, wind: &dyn Fn([f32; 3]) -> [f32; 3]) {
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

            // **Pulled toward the air, not pushed by it.** A constant force
            // would accelerate a particle downwind forever; matching velocity
            // is what makes smoke end up travelling *with* the wind and then
            // staying there. The coefficient is clamped for the same reason
            // drag is — a response above `1/dt` would overshoot past the air
            // and oscillate.
            if emitter.wind_response > 0.0 {
                let air = wind(p.position);
                let follow = (emitter.wind_response * dt).clamp(0.0, 1.0);
                for (velocity, air) in p.velocity.iter_mut().zip(air) {
                    *velocity += (air - *velocity) * follow;
                }
            }

            for axis in 0..3 {
                p.velocity[axis] *= damping;
                p.position[axis] += p.velocity[axis] * dt;
            }
            p.age += dt;
        }
    }

    fn spawn(&mut self, dt: f32, emitter: &Emitter, origin: [f32; 3]) {
        // Seconds simulated so far. From the tick count, never a clock.
        #[allow(clippy::cast_precision_loss)]
        let age = self.ticks as f32 * dt;
        if age < emitter.delay {
            return;
        }

        // The burst, exactly once, on the first step at or past the delay.
        let mut count = 0;
        if !self.burst_done {
            self.burst_done = true;
            count += emitter.burst;
        }

        // Continuous emission, if it has not run out. A `duration` of zero is
        // "no limit" rather than "emit nothing" — most emitters are chimneys,
        // and an unset field must not silently switch one off.
        if emitter.duration <= 0.0 || age < emitter.delay + emitter.duration {
            self.owed += emitter.rate.max(0.0) * dt;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let whole = self.owed as u32;
            self.owed -= f32::from(u16::try_from(whole).unwrap_or(u16::MAX));
            count += whole;
        }

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
mod ordinal_tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    /// Total ever released, by never letting anything die.
    fn released(emitter: &Emitter, ticks: u64) -> u32 {
        let mut system = System::new(emitter.seed);
        for _ in 0..ticks {
            system.step(DT, emitter, [0.0; 3]);
        }
        u32::try_from(system.particles().len()).expect("test population fits")
    }

    fn immortal(rate: f32) -> Emitter {
        Emitter { rate, lifetime: 1.0e9, lifetime_jitter: 0.0, ..Emitter::default() }
    }

    /// **The twin test.** `births_by` is what the GPU pool allocates slots
    /// with, and `System`'s `owed` accumulator is what the CPU emitter has
    /// always spawned with. If they disagree by more than a rounding, a scene
    /// authored with `gpu = true` holds a visibly different number of
    /// particles from the same scene with it off, and nothing else in the
    /// project would say so.
    ///
    /// 37.3 a second at 1/60 s is 0.6217 per tick — deliberately awkward, so
    /// the carried remainder is exercised on every single step rather than
    /// landing on a whole number periodically.
    ///
    /// **Exact, at every one of the ten thousand ticks, and that bound is
    /// measured rather than hoped for.** The two are not the same arithmetic:
    /// the accumulator adds a float once per step and this multiplies once, and
    /// float addition is not associative, so they must eventually part. Swept
    /// out to 200,000 ticks at this rate the first disagreement is a single
    /// particle at tick **26,363** — which is 7.3 minutes of simulation, an
    /// order of magnitude past the longest `--sim` in the repo. Ten thousand is
    /// inside the exact region deliberately, so this is a real gate: rounding
    /// instead of flooring, or dropping the delay boundary, fails it.
    #[test]
    fn births_by_matches_the_owed_accumulator_over_ten_thousand_ticks() {
        let emitter = immortal(37.3);
        // One pass rather than 10,000 replays: with nothing dying, the live
        // population after `t` steps *is* the number ever released.
        let mut system = System::new(emitter.seed);
        for tick in 1..=10_000_u64 {
            system.step(DT, &emitter, [0.0; 3]);
            let accumulated = u32::try_from(system.particles().len()).expect("fits");
            assert_eq!(
                births_by(&emitter, tick, DT),
                accumulated,
                "the closed form and the accumulator parted at tick {tick}"
            );
        }
    }

    /// A rate that divides evenly has no excuse to drift at all.
    #[test]
    fn a_whole_rate_agrees_exactly() {
        let emitter = immortal(60.0);
        for ticks in [1_u64, 30, 120, 601] {
            assert_eq!(
                births_by(&emitter, ticks, DT),
                released(&emitter, ticks),
                "at {ticks} ticks"
            );
        }
    }

    /// The burst, the delay and the duration are the three gates `System`
    /// applies, and the closed form has to apply all three or a GPU explosion
    /// would fire on tick zero and never stop.
    #[test]
    fn the_closed_form_respects_burst_delay_and_duration() {
        let blast = Emitter {
            rate: 60.0,
            burst: 50,
            delay: 0.5,
            duration: 0.25,
            lifetime: 1.0e9,
            lifetime_jitter: 0.0,
            ..Emitter::default()
        };
        // Nothing before the delay: half a second is step 30, and at tick 30
        // that step has not run.
        assert_eq!(births_by(&blast, 30, DT), 0, "fired early");
        assert_eq!(released(&blast, 30), 0, "and the accumulator agrees");
        // Step 30 releases the burst *and* the first continuous particle, in
        // that order — the accumulator does both in one call to `spawn`, so
        // the closed form has to as well.
        assert_eq!(births_by(&blast, 31, DT), 51, "the burst plus one tick");
        assert_eq!(released(&blast, 31), 51);
        // A quarter second of 60/s past the delay, and then it stops for good.
        let at_end = births_by(&blast, 46, DT);
        assert_eq!(at_end, 65, "burst plus 0.25 s at 60/s");
        assert_eq!(births_by(&blast, 600, DT), at_end, "duration did not stop it");
        assert_eq!(released(&blast, 600), at_end, "and the accumulator agrees");
    }

    /// A `duration` of zero means forever, matching `System::spawn`. Reading
    /// it as "emit nothing" would silently switch off every chimney in the
    /// repo the moment it moved to the GPU.
    #[test]
    fn a_duration_of_zero_emits_forever() {
        let emitter = immortal(60.0);
        assert_eq!(births_by(&emitter, 3_600, DT), 3_600);
    }

    /// **The pool-lapping rule.** A slot is reused every `N / rate` seconds
    /// and its occupant lives up to `lifetime * (1 + jitter)`, so the size has
    /// to cover the longer of the two or a live particle is overwritten.
    #[test]
    fn the_pool_covers_the_longest_life_a_particle_can_have() {
        let emitter = Emitter { rate: 40.0, lifetime: 3.0, lifetime_jitter: 0.35, burst: 0, ..Emitter::default() };
        let n = pool_size(&emitter);
        let longest = emitter.lifetime * (1.0 + emitter.lifetime_jitter);
        let laps_after = f32::from(u16::try_from(n).expect("small pool")) / emitter.rate;
        assert!(
            laps_after >= longest,
            "{n} slots lap in {laps_after} s against a {longest} s life"
        );
        // And the burst holds slots of its own for a full lifetime.
        let blast = Emitter { burst: 500, ..emitter };
        assert_eq!(pool_size(&blast), n + 500);
        // Never zero: a dispatch of no threads draws nothing and reports no
        // error, which is the shape of bug this whole crate is written against.
        assert_eq!(pool_size(&Emitter { rate: 0.0, burst: 0, ..emitter }), 1);
    }
}

#[cfg(test)]
mod wind_tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn blown(response: f32, wind: [f32; 3]) -> f32 {
        let emitter = Emitter {
            wind_response: response,
            gravity: 0.0,
            drag: 0.0,
            turbulence: 0.0,
            speed: 0.0,
            spread_degrees: 0.0,
            ..Emitter::default()
        };
        let mut system = System::new(7);
        for _ in 0..180 {
            system.step_in_wind(DT, &emitter, [0.0, 0.0, 0.0], &|_| wind);
        }
        // How far the oldest particle has travelled downwind.
        system.particles().iter().map(|p| p.position[0]).fold(f32::MIN, f32::max)
    }

    /// **Every scene authored before wind existed must render exactly as it
    /// did.** Two of the six golden images are particle scenes, so a non-zero
    /// default would silently move both — which is why the default is zero and
    /// why this is a test rather than a comment.
    /// Compared against a plume that was never told about wind at all, rather
    /// than against zero: an emitter has jitter of its own, and asserting a
    /// bare `0.0` would be testing the jitter instead of the coupling.
    #[test]
    fn wind_does_nothing_unless_a_particle_asks_for_it() {
        let emitter = Emitter { wind_response: 0.0, ..Emitter::default() };

        let mut ignored = System::new(5);
        let mut untold = System::new(5);
        for _ in 0..300 {
            ignored.step_in_wind(DT, &emitter, [0.0, 0.0, 0.0], &|_| [12.0, 3.0, -7.0]);
            untold.step(DT, &emitter, [0.0, 0.0, 0.0]);
        }

        let positions = |s: &System| {
            s.particles().iter().map(|p| p.position).collect::<Vec<[f32; 3]>>()
        };
        assert_eq!(
            positions(&ignored),
            positions(&untold),
            "a gale moved particles that opted out of wind"
        );
    }

    /// Smoke ends up travelling *with* the air, not accelerating forever. That
    /// is the difference between coupling to a velocity and being pushed by a
    /// force, and it is the whole reason this is a lerp toward the wind.
    #[test]
    fn a_coupled_particle_approaches_the_wind_speed_and_stays_there() {
        let emitter = Emitter {
            wind_response: 6.0,
            gravity: 0.0,
            drag: 0.0,
            turbulence: 0.0,
            speed: 0.0,
            spread_degrees: 0.0,
            rate: 0.0,
            burst: 1,
            ..Emitter::default()
        };
        let mut system = System::new(3);
        // One tick to spawn the burst, then let it settle.
        for _ in 0..120 {
            system.step_in_wind(DT, &emitter, [0.0, 0.0, 0.0], &|_| [5.0, 0.0, 0.0]);
        }

        let speed = system.particles()[0].velocity[0];
        assert!(
            (speed - 5.0).abs() < 0.05,
            "a particle in 5 m/s air settles at {speed} m/s — coupling should \
             converge on the wind, not overshoot it or keep accelerating"
        );
    }

    /// Coupling is a property of the particle, not of the weather: embers lag
    /// the air that smoke matches. The scene `assets/test/windy.loom` shows
    /// the two side by side.
    #[test]
    fn a_heavier_particle_lags_a_lighter_one_in_the_same_air() {
        let smoke = blown(4.0, [8.0, 0.0, 0.0]);
        let ember = blown(0.4, [8.0, 0.0, 0.0]);

        assert!(smoke > ember * 1.5, "smoke {smoke}, ember {ember}");
        assert!(ember > 0.0, "the ember is not moving at all: {ember}");
    }

    /// The wind is sampled at each particle's own position. Sampling once per
    /// emitter would slide the whole plume sideways as a rigid body, which is
    /// the unison artifact the turbulence term exists to break.
    #[test]
    fn the_field_is_sampled_per_particle_not_per_emitter() {
        let emitter = Emitter {
            wind_response: 5.0,
            gravity: 3.0,
            drag: 0.0,
            turbulence: 0.0,
            ..Emitter::default()
        };
        let mut system = System::new(11);
        // Wind that only blows above two metres. A per-emitter sample taken at
        // the origin would be zero for everyone, and nothing would move.
        for _ in 0..240 {
            system.step_in_wind(DT, &emitter, [0.0, 0.0, 0.0], &|at| {
                if at[1] > 2.0 { [9.0, 0.0, 0.0] } else { [0.0, 0.0, 0.0] }
            });
        }

        let high = system.particles().iter().filter(|p| p.position[1] > 2.0).count();
        let moved = system.particles().iter().filter(|p| p.position[0] > 0.5).count();
        assert!(high > 0, "nothing reached the windy layer");
        assert!(moved > 0, "the field was not sampled where the particles are");
        assert!(
            moved <= high + 2,
            "particles below the layer were blown too: {moved} moved, {high} are high"
        );
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
    fn a_burst_releases_everything_at_once() {
        let emitter = Emitter { rate: 0.0, burst: 50, lifetime: 10.0, ..Emitter::default() };
        let mut system = System::new(7);

        system.step(1.0 / 60.0, &emitter, [0.0; 3]);

        assert_eq!(system.particles().len(), 50, "the whole blast, on one tick");
    }

    /// A burst that repeats is a fountain. The flag has to latch.
    #[test]
    fn a_burst_fires_only_once() {
        let emitter = Emitter { rate: 0.0, burst: 20, lifetime: 100.0, ..Emitter::default() };
        let mut system = System::new(7);

        for _ in 0..120 {
            system.step(1.0 / 60.0, &emitter, [0.0; 3]);
        }

        assert_eq!(system.particles().len(), 20, "two seconds later, still one blast");
    }

    /// Staging an explosion out of several emitters depends on this: the smoke
    /// must not already be there when the fire goes off.
    #[test]
    fn a_delayed_emitter_waits_its_turn() {
        let emitter = Emitter { rate: 0.0, burst: 10, delay: 0.5, lifetime: 10.0, ..Emitter::default() };
        let mut system = System::new(7);

        for _ in 0..20 {
            system.step(1.0 / 60.0, &emitter, [0.0; 3]);
        }
        assert!(system.particles().is_empty(), "a third of a second in, nothing yet");

        for _ in 0..20 {
            system.step(1.0 / 60.0, &emitter, [0.0; 3]);
        }
        assert_eq!(system.particles().len(), 10, "past the delay, the blast lands");
    }

    #[test]
    fn emission_stops_at_the_end_of_its_duration() {
        let emitter = Emitter {
            rate: 60.0,
            duration: 0.5,
            lifetime: 100.0,
            ..Emitter::default()
        };
        let mut system = System::new(7);

        for _ in 0..30 {
            system.step(1.0 / 60.0, &emitter, [0.0; 3]);
        }
        let at_the_limit = system.particles().len();

        for _ in 0..120 {
            system.step(1.0 / 60.0, &emitter, [0.0; 3]);
        }

        assert!(at_the_limit >= 25, "should have emitted for half a second: {at_the_limit}");
        assert_eq!(
            system.particles().len(),
            at_the_limit,
            "two more seconds must add nothing"
        );
    }

    /// An unset `duration` must not switch an emitter off. Most emitters are
    /// chimneys and never set it.
    #[test]
    fn a_duration_of_zero_emits_forever() {
        let emitter = Emitter { rate: 60.0, lifetime: 100.0, ..Emitter::default() };
        let mut system = System::new(7);

        for _ in 0..120 {
            system.step(1.0 / 60.0, &emitter, [0.0; 3]);
        }

        assert!(system.particles().len() > 100, "still going: {}", system.particles().len());
    }

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
