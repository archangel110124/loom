# ADR 0090 — Two ears, not two volumes

- **Date:** 2026-09-07
- **Status:** **accepted** — eighth of the nine subsystems scoped as "what is
  missing for a full-blown game engine", at the general-engine bar.
- **Decision touched:** none. No new crate, dependency, component or asset.
- **Human decision this records:** *"Audio is real, but not middleware"* — and
  the choice of an analytic head model over a measured HRTF dataset.

## 1. What was already there

The survey said audio needed work. Reading the crate first: 3025 lines with
ray-traced occlusion, room measurement by casting rays outward from the
listener, constant-power panning, material muffling, and a reverb whose delay
comes from real path length divided by a per-scene `speed_of_sound` — which is
documented as "not a constant, because a scene is free to be on another planet".

That is not a stub. **The gap was narrower and more specific than "audio":**
three cues were missing, and each one is a thing the ear uses to place a sound.

## 2. What was missing, and why each matters

**Doppler.** Nothing shifted pitch with motion, although `speed_of_sound` was
already a scene parameter and `Voice::step` was already documented as "pitch
shifts are the same knob". A boat passing you sounded like a boat standing
still that happened to get louder.

**Air absorption.** Distance cut the gain and nothing else. A quiet near sound
and a distant one are not the same thing, and the difference the ear actually
uses is the top end — air absorbs high frequencies as sound travels, which is
most of why thunder rumbles and a strike beside you cracks. Without it, distance
was a volume knob.

**A head.** Panning alone puts a sound *inside* the head: both ears get one
signal at two volumes, which never happens in a room. What puts a sound out
there is that it reaches one ear first and arrives duller at the other.

## 3. Decision

All three, in the places they already belonged:

- `doppler(relative, closing, speed_of_sound)` returns a frequency ratio that
  multiplies `Voice::step`. Nothing else in the mixer knows what a doppler shift
  is — the cursor advances faster and the sound is higher.
- `air_absorption(distance)` scales the cutoff of the low-pass that muffling
  already drives. One filter, two reasons to close it.
- The mixer renders each ear separately: an interaural delay on the far ear and
  a one-pole head shadow, on top of the panning that was already there.

**Velocity is differenced, not plumbed.** A source's velocity is not something a
scene declares — some ride a rigid body, some are carried by a character, some
are nodes a script moves — and the one quantity all of them share is where they
ended up. The tick is fixed, so a difference is a velocity, and the first tick
of a source reports zero rather than having crossed its whole offset in a frame.

That difference is taken in **world space, not the listener's frame**. The
rotated vector changes when the player merely turns their head, and differencing
it would pitch-shift the whole scene for turning round.

## 4. Why an analytic head and not a measured HRTF

A measured HRTF is the better sound. It also means shipping and licensing
someone else's measurement data, and convolving per voice.

The analytic model — Woodworth interaural delay plus a shadow filter — carries
the two cues that do most of the work over headphones, costs no dependency and
no asset, and is consistent with how the rest of this crate behaves: the reverb
is already derived from ray-traced geometry rather than a preset, so a head
derived from geometry belongs beside it.

**It does not carry elevation, and front-back is symmetric.** A source behind
you has the same interaural delay as one in front at the same lateral angle.
That is not an approximation error — it is what interaural delay *is*, and it is
exactly the cue a measured HRTF adds. If that becomes the thing DEEPER needs,
the delay-and-shadow stage is where the impulse responses go.

## 5. Traps this hit

**Woodworth holds to a quarter turn.** Taking the azimuth with `atan2` kept
growing past 90 degrees, and at 112 degrees the delay asked for 71 samples where
a head allows 63 — a ring-buffer read from the future. Taking `asin` of the
sideways component folds front and back onto the same angle, bounded by
construction. The test walks all 360 degrees rather than checking the ends.

**The head is not on another planet.** The head geometry uses a fixed 343 m/s,
deliberately *not* `Ears::speed_of_sound`. That one is the scene's, and a scene
may be somewhere else — but the listener's skull is the same skull, and a mixer
that resized the player's head when the wind changed would be a stranger bug
than the one it fixed.

**The pole at Mach 1.** The doppler ratio divides by `c + v`. A boat cannot
reach it; a scripted projectile can, and a divide by zero is not a sound. It is
clamped to a factor of four either way.

## 6. What this does not do

- **No elevation cue** — see §4.
- **No per-ear reverb.** The room returns one signal to both ears.
- **No HRTF dataset**, and no code that expects one.
