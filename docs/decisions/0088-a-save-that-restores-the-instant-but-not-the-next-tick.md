# ADR 0088 — A save that restores the instant but not the next tick

- **Date:** 2026-09-07
- **Status:** **accepted** — sixth of the nine subsystems scoped as "what is
  missing for a full-blown game engine", at the general-engine bar.
- **Decision touched:** none. No new crate, dependency, pass or component. Two
  CLI flags, a JSON snapshot, and four accessors on things that already existed.
- **Human decision this records:** *"everything the game reads"* as the scope of
  a snapshot, and *"the same state hash after save and load"* as the proof.

## 1. What a save turned out to be

The obvious reading of "save the game" is *write down where everything is*. That
reading produced a save file that was correct at the instant it was loaded and
wrong one tick later, and it stayed wrong in that exact shape through four
separate causes.

The state of this engine at a tick is not the pose of its nodes. It is the pose
of its nodes **plus everything one tick of simulation is about to read**, and a
surprising amount of that is a tick behind on purpose.

## 2. Decision

`loom sim` gains `--save <path>` and `--load <path>`. `--load` runs before the
tick loop, so `--ticks` means "and then this many more" and a save composes with
simulating rather than replacing it.

The snapshot holds, in one JSON object: the rules' `GameState`, the cumulative
`EventLog`, every dynamic body's `BodyState`, every character's script memory,
controller position, grounded flag, velocity and cached route, every node's
local transform, the wavelet pool, the debounced float state, the live helm
thrust, and the sim's own tick counter.

Node scripts keep nothing between ticks — `host.tick` reads a node's current
transform and returns the next one — so **a node script's memory is its
transform**, and saving transforms saves them.

## 3. The four things that were a tick behind

Each of these restored an instant that hashed correctly and a run that diverged
immediately, which is why they had to be found one at a time.

1. **The sim's own tick counter.** `self.tick += 1` was an internal counter that
   only ever counted from zero; it was never set from the caller's tick. Water
   is a function of position and `self.tick * TICK_SECONDS`, so a loaded game
   put its boat on the wave phase of `t = 0` while the run it continued was ten
   seconds into the swell.

2. **Live helm thrust.** The step runs *before* the scripts that steer, so every
   step applies the helm vector the previous tick wrote. A load started from the
   authored `Propulsion` and dropped one tick of the propeller — 934 kN on a
   57 t hull, which surfaced as the boat being 0.27 m/s slow.

3. **A kinematic body's pending target.** Same ordering: the step consumes the
   `next_kinematic_translation` the previous tick's character move left behind.
   A freshly loaded body had none, rapier derived a velocity of zero, and the
   player stopped shoving the boat for exactly one tick.

4. **The character controller's position.** A `Character` owns its position; its
   body is kinematic and *follows* that field every step. Restoring the body
   alone was silently undone by the next move, which starts from the field — so
   the player snapped back to their spawn one tick after the load.

## 4. Why the state hash was not enough on its own

`World::state_hash` covers node transforms. It matched at the instant of every
one of the four failures above, because transforms are exactly what the naive
save restores. What it could not see was velocity, thrust, or any counter.

So the check that matters is not "does the hash match at the load" but **does
the whole state match 600 ticks later**. `scripts/green.sh` compares a straight
1000-tick run against a 400-tick save resumed for 600, byte for byte, on
`deeper_demo` — a floating hull, a character riding it, a scripted fight and
four node scripts — and on `proving_ground`, which is already over when it is
saved.

That second row found a fifth bug that is not about the snapshot at all: the
tick loop checked `is_over()` *after* ticking, so loading a finished game
simulated one tick past its own ending. The check now runs at the top of the
loop, which is identical for a fresh run — it cannot start finished — and
correct for a loaded one.

## 5. Two things the byte-identity row found that the hash could not

Both were invisible to `state_hash` and to a 600-tick continuation, and both
only surfaced once the gate compared whole save files.

**serde_json's float parser is not correctly rounded.** It reads
`0.9680542349815369` — a value a `proving_ground` hunter really had in
`last_seen` — as the f64 one ULP below it. Rust's own `str::parse::<f64>` gets
it right, and so does every other parser tried; `{:.17}` and `{:.20}` forms did
not help, so this is not a shortest-repr problem. The fix is serde_json's own
`float_roundtrip` feature, which exists for exactly this, enabled on the two
crates that own the path. 1.0.151 is the current release, so there was no
upgrade to take instead.

A save that cannot survive being loaded is not a save, and this had been
silently rounding script memory on every load.

**A kinematic body's velocity is derived, and was being saved anyway.** rapier
recomputes it each step from the pending target and ignores writes to it, so
the field could be written to a file but never restored from one. A game saved
*after it had ended* never steps again, so the reloaded velocities stayed zero
while the run they continued still held their last values.

The character entry now stores pose only. **A field that loading cannot
reproduce does not belong in a save**, because its presence claims a fidelity
the format does not have.

## 6. What this does not do

- **No versioning.** A save names its `format` and `scene` and nothing migrates
  an old file. Refusing to load one is honest; silently loading half of one is
  not.
- **No mid-tick saves.** The snapshot is taken between ticks, which is the only
  place the engine has a consistent state to write down.
- **Not a replay.** This records where the world *is*, not the input that got it
  there. `xtask repeat` already covers replay determinism.
- **Not compressed, not binary.** 85 KB of JSON for a whole game. When that is
  the problem, it will be a measured one.
