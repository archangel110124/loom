// The rain layer's state, shared by the compute shader that moves it and the
// vertex shader that draws it.
//
// **One memory layout described three times** — here, and in
// `loom_render::rain`'s Rust mirrors. Two of those three are this file, which
// is why it exists: `rain_sim.slang` and `scene.slang` are separate modules
// with separate SPIR-V, and a `RainDrop` that disagreed between them would put
// the velocity where the position is and rain sideways with nothing to blame.
//
// Included from a subdirectory because `build.rs` compiles every top-level
// `.slang` as its own module, and a file of declarations with no entry point
// is not a module. Same reason `generated/` is a subdirectory.
//
// **Include `generated/fields.slang` before this**: `loom_hash` is the
// project's one integer hash (frozen ABI, ADR 0006) and everything below seeds
// itself from it rather than carrying a second family of magic constants.

/// One drop. 32 bytes.
///
/// **`float4` twice rather than two `float3`s**, which would be 24 bytes in
/// Rust and 32 in a shader — a `float3` in a buffer still aligns to 16. ADR
/// 0014 estimated 24 bytes at 3 MB; this is 4 MB, and the extra megabyte buys
/// the two trailing scalars below, which are what make a drop more than a
/// position.
struct RainDrop {
    /// xyz world position, w seconds since this drop was last issued.
    ///
    /// The age is not bookkeeping: a reissued drop is a drop that *teleports*,
    /// and with a closed-form field that teleport could be hidden by placing it
    /// where the boundary fade is already zero. State cannot do that — a drop
    /// lands where the world stopped it, which may be anywhere. So a reissued
    /// drop fades in over [`RAIN_SPAWN_FADE`] instead.
    float4 position;
    /// xyz velocity in m/s, w the drop's identity as a bit-cast `uint`.
    ///
    /// **Per-drop velocity is the point of being stateful.** The closed-form
    /// version gave every drop in the scene exactly one velocity, because a
    /// drop's position had to stay a closed form of its index and the time.
    /// Here a drop carries its own, so it can have its own terminal speed, be
    /// blown by its own gust, and — the part that matters — be *stopped*.
    float4 velocity;
};

/// One splash, appended by a drop that actually hit something.
///
/// ADR 0014's trigger 3: a true collision point rather than a position exposure
/// says a drop *should* have reached.
struct RainSplash {
    /// xyz world impact point, w the simulated time it happened at.
    float4 impact;
    /// xyz surface normal, w impact speed in m/s.
    float4 surface;
};

/// Drops the layer simulates. One dispatch thread each.
///
/// 131,072 = 64 x 32 x 64, which is one per cubic metre of [`RAIN_BOX`] — the
/// density the closed-form grid already carried, kept so this change is about
/// behaviour rather than about how much rain there is.
static const uint RAIN_DROPS = 131072u;

/// The volume drops live in, in metres, centred on the eye.
///
/// **Camera-locked, and it has to be.** A world-anchored volume is a volume the
/// camera can walk out of. What the eye actually sees is a horizon's worth of
/// rain in every direction, and the only affordable way to have that everywhere
/// is to carry it. The seam this creates is handled by wrapping rather than by
/// respawning — see `rainWrap`.
static const float3 RAIN_BOX = float3(64.0, 32.0, 64.0);

/// Fraction of each axis a drop fades over, measured in from that face.
///
/// Unchanged from the closed-form layer, and for the same reason: whatever
/// happens at a boundary must happen at zero brightness.
static const float RAIN_FADE = 0.125;

/// Terminal velocity, m/s, before the per-drop variation below.
static const float RAIN_FALL = 8.0;

/// Spread of terminal speed across drops, as a fraction.
///
/// **New with state, and free.** Real rain is a drop-size distribution and
/// terminal velocity goes with size, so a field where every drop falls at
/// exactly 8 m/s is a field of identical objects — which is one of the things
/// that made the closed-form layer read as a texture rather than as weather.
/// 0.25 is +-2 m/s, which is roughly the spread between a 1 mm and a 3 mm drop.
static const float RAIN_SPEED_SPREAD = 0.25;

/// How sharply a drop's velocity relaxes toward the air's, per tick.
///
/// A drop is not weightless: it takes about a second to be turned by a gust.
/// 0.06 per tick at 60 Hz is a time constant near 0.28 s, which is short enough
/// that a scene's wind still reads in the streaks and long enough that a drop
/// entering shelter keeps its lean for a moment instead of snapping upright.
static const float RAIN_DRAG = 0.06;

/// Seconds a reissued drop fades in over.
static const float RAIN_SPAWN_FADE = 0.15;

/// Seconds one streak stands for — a shutter time, not a length.
///
/// See the long note this replaced in `scene.slang`: 0.1 s is a still
/// photograph's number and made streaks read as scratches on the lens. A frame
/// at 60 fps with a 180-degree shutter integrates 1/120 s; this is about double
/// a real frame, deliberately.
static const float RAIN_SHUTTER = 0.03;

/// Streak width in metres.
static const float RAIN_WIDTH = 0.02;

/// Screen-space width no streak may fall below, in pixels. Zero disables it.
///
/// **The other half of the sub-pixel fix, and it needed MSAA first.** A 0.02 m
/// streak is under a pixel wide past about ten metres, and a sub-pixel primitive
/// is decided by whether it happens to cover a sample: it flickers on and off as
/// the camera moves. Multisampling the rain pass reduced that to a quarter
/// amplitude but could not remove it — four samples is still a decision.
///
/// Widening a thin streak to a whole pixel makes its coverage continuous rather
/// than binary. **The widening must be paid for in alpha**, or distant rain gets
/// brighter as it gets further away: the streak covers `widen` times the area,
/// so its strength is divided by `widen` and the total light it adds is
/// unchanged. Rain is additively blended, so that conservation is exact.
///
/// **2.5, not 1.0, and the number is the interesting part.** "At least one
/// pixel" is the conventional statement of this trick and it is measurably the
/// wrong setting: a one-pixel primitive still straddles pixel boundaries, so it
/// produces *more* isolated pixels than no floor at all. Measured on
/// `rain_impact`'s sky band as the fraction of pixels disagreeing sharply with
/// their own neighbourhood — which is what "salt" is:
///
///     floor  0.0  0.75%      1.0  0.95%      1.5  0.62%
///            2.0  0.27%      2.5  0.10%      3.0  0.02%
///
/// It gets worse before it gets better. 2.5 removes 87% of the salt and still
/// reads as rain; past 3 the streaks start reading as scratches, which is the
/// artifact `RAIN_SHUTTER` already exists to avoid.
///
/// **The flicker metric disagrees, and it is the one being overruled.** Five
/// triples of a walking camera: 3.451 with no floor, 3.543 at 2.5 — 2.7%
/// worse. That measurement cannot distinguish "wider" from "less stable",
/// because a wider primitive changes more pixels by construction; the
/// resolution sweep confirms the effect is entirely the widening (it costs
/// 5.2% at 320x200, 0.4% at 1280x800 — largest exactly where the floor does the
/// most widening). Brightness is conserved either way: 0.7577 against 0.7593,
/// so neither number is the brightness artifact ADR 0010 warns about.
///
/// This was measured *worse* twice on grass (`GRASS_MIN_PIXELS`, still 0.0),
/// and the difference is instructive: those readings were taken on a path with
/// no multisampling, where widening a sub-pixel blade recruits more pixels into
/// the same binary decision instead of softening it. They were also taken on
/// flicker alone, which is the metric that cannot see this.
static const float RAIN_MIN_PIXELS = 2.5;

/// Metres from the eye a drop is fully invisible at, and fully visible by.
///
/// **The near field, and it is not a taste call.** A streak's width is constant
/// in *world* space — it has to be, or a distant drop is not visible at all —
/// so its screen width goes as 1/distance. At `RAIN_WIDTH` a drop half a metre
/// from the eye draws about nineteen pixels wide and two hundred and forty
/// long, at full additive brightness, and at one drop per cubic metre there are
/// about three of those inside a one-metre sphere at any instant. They read as
/// lens flares or smoke rather than as rain, and because they enter and leave
/// within a frame or two they take the whole layer's brightness with them: the
/// rain light in `rain_impact`'s sky band swung 14% frame to frame with
/// single-frame spikes of +70%, entirely from this.
///
/// **The boundary fade does not cover it**, and cannot: that fades a drop near
/// a face of the block, and a drop 30 cm from the eye is in the middle of the
/// block, as far from every face as it is possible to be.
///
/// So the nearest couple of metres fade out, which is defensible on its own
/// terms as well: no real camera focuses at 20 cm, and a drop that close is an
/// out-of-focus smear carrying almost no light. It costs about 65 drops of
/// 131,072 — the volume of a 2.5 m sphere — which is nothing.
static const float RAIN_NEAR_MIN = 0.6;
static const float RAIN_NEAR_FULL = 2.5;

/// The fixed timestep the drop simulation runs at.
///
/// **The simulation's, not the frame's.** Rain never sees a variable `dt`
/// (locked decision: fixed timestep), which is also what makes the drop state a
/// function of the tick count and therefore reproducible.
static const float RAIN_TICK = 1.0 / 60.0;

/// Rate, in mm/h, at which the visual layer is at full strength.
///
/// **Three readers and one number.** The streaks thin themselves by it, the
/// ripples on wet ground thin themselves by it, and `loom_rain`'s
/// `SPLASH_FULL_RATE` is the third. Moved here from `scene.slang` when the
/// compute shader became the fourth.
static const float RAIN_FULL_RATE = 20.0;

/// Entries in the splash ring.
///
/// **Sized so one tick's landings cannot lap it, and that is a correctness
/// bound rather than a budget.** At one drop per cubic metre falling 8 m/s
/// through a 32 m tall block, about 550 drops land per tick; 16,384 entries is
/// thirty ticks of them, which is longer than [`RAIN_SPLASH_LIFE`]. If a tick
/// could lap the ring the drawn set would depend on the order the atomics
/// happened to resolve in, and the golden gate would flake.
static const uint RAIN_SPLASH_RING = 16384u;

/// Seconds a splash is visible for.
///
/// A quarter of a second, which is about how long a real crown and its ring
/// last. Longer and the ground reads as boiling.
static const float RAIN_SPLASH_LIFE = 0.25;

/// Radius a splash ring expands to, in metres.
///
/// A hand's width. The CPU crowns this replaced ran 0.03 to 0.09 m, and the
/// reasoning has not changed: a splash the size of a smoke puff reads as steam
/// rather than as rain landing.
static const float RAIN_SPLASH_RADIUS = 0.11;

/// Peak brightness of one splash against the sky colour it is lit by.
///
/// Matched to the `CROWN_ALPHA = 0.5` the CPU crowns carried, allowing for the
/// ring's shape term never reaching one over a whole quad.
static const float SPLASH_BRIGHTNESS = 0.9;

/// Four uncorrelated values in `[0, 1)` from one integer.
float4 rainHash4(uint seed) {
    uint h1 = loom_hash(seed);
    uint h2 = loom_hash(h1);
    uint h3 = loom_hash(h2);
    uint h4 = loom_hash(h3);
    const float SCALE = 1.0 / 16777216.0;
    return float4(float(h1 >> 8), float(h2 >> 8), float(h3 >> 8), float(h4 >> 8)) * SCALE;
}

/// The block a drop lives in: its low corner, given the eye.
///
/// Vertically the eye sits in the middle, which is what a camera at head height
/// wants — rain above to fall and rain below to land.
float3 rainBlockLow(float3 eye) {
    return eye - RAIN_BOX * 0.5;
}

/// Whether the drop at `p` is inside the block, as a per-axis 0..1 fade that
/// reaches zero inside the outermost eighth.
float rainEdgeFade(float3 p, float3 eye) {
    float3 inBlock = p - rainBlockLow(eye);
    float3 fromFace = min(inBlock, RAIN_BOX - inBlock);
    float3 faded = smoothstep(float3(0.0), RAIN_BOX * RAIN_FADE, fromFace);
    return min(min(faded.x, faded.y), faded.z);
}

/// How much of a drop survives being too close to the eye. See
/// [`RAIN_NEAR_MIN`].
float rainNearFade(float3 p, float3 eye) {
    return smoothstep(RAIN_NEAR_MIN, RAIN_NEAR_FULL, distance(p, eye));
}

/// This drop's terminal speed, from its identity. Stable across its lifetime.
float rainTerminal(uint seed) {
    return RAIN_FALL * (1.0 + (rainHash4(seed).x * 2.0 - 1.0) * RAIN_SPEED_SPREAD);
}

/// Whether this drop exists at a given rate.
///
/// **Light rain is sparser as well as fainter.** Thinning by shortening the
/// draw would delete a contiguous slab of the buffer rather than thin the
/// field, so a drop keeps or drops itself against the ratio, stably.
bool rainPresent(uint seed, float rate) {
    return rainHash4(seed).y < saturate(rate / RAIN_FULL_RATE);
}
