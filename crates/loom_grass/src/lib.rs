//! Where the blades are. The first slice of P2.
//!
//! **A blade is a pure function of where it is.** Nothing here is stored,
//! streamed or incrementally updated: given a tile and an index, the same
//! blade comes out, every time, on any machine. That single property is what
//! buys three separate things the phase needs:
//!
//! * **Dirty-region regeneration is byte-identical.** Blowing a crater in the
//!   terrain regenerates the tiles it touched and nothing else, and the tiles
//!   it did not touch are provably unchanged — not "probably fine", provably,
//!   because they are computed from coordinates that did not change.
//! * **Order independence.** Tiles can be generated in any order, in parallel,
//!   partially, or twice, and the field is the same.
//! * **The GPU can compute the same thing.** This is the CPU half of what will
//!   become a placement compute shader, so it is written against the frozen
//!   `loom_field::noise::hash` rather than anything platform-specific.
//!
//! # What this is not, yet
//!
//! It is placement only: positions, clumps, and the per-blade payload a shader
//! needs. It does not draw anything. The compute pipeline, the Bézier
//! expansion, LOD, culling and — the phase's real risk — anti-aliasing are all
//! still ahead. See the phase notes in `CLAUDE.md`.
//!
//! # Rendering only
//!
//! Nothing here may ever be read by the deterministic simulation. Grass is
//! visual, it has the same exemption rain does, and the sim hash must be
//! identical with grass on or off. The exemption is not laziness — a million
//! blades in the hash would make every hash a function of the camera.

use loom_field::noise::hash;

/// World size of one placement tile, in metres.
///
/// **The unit of regeneration.** A crater dirties the tiles it overlaps, so
/// this trades regeneration granularity against per-tile overhead: smaller
/// tiles regenerate less on an edit and cost more to iterate. Four metres is
/// a few hundred blades at ordinary density — small enough that a grenade
/// dirties a handful, large enough that a field is not millions of tiles.
pub const TILE: f32 = 4.0;

/// World size of one clump cell, in metres.
///
/// **Clumping is the highest-value realism lever in the whole phase** — more
/// than blade quality. Uniformly random blades read as carpet; blades that
/// agree with their neighbours about which way they lean and what colour they
/// are read as a field. Half a metre is a tussock.
pub const CLUMP: f32 = 0.5;

/// One blade, as the shader will want it.
///
/// `#[repr(C)]` because this becomes a GPU buffer element. The field order is
/// the buffer layout, and the same layout-described-twice hazard applies here
/// as everywhere else — when the compute half lands, its struct is generated
/// from this one rather than typed out again.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Blade {
    /// World position of the base.
    pub position: [f32; 3],
    /// Facing, as a unit vector in the XZ plane.
    pub facing: [f32; 2],
    /// Metres from base to tip before any wind bend.
    pub height: f32,
    /// Metres across at the base. The tip tapers to nothing.
    pub width: f32,
    /// How far the blade already leans at rest, in radians from vertical.
    /// Wind bends further from here; a field of perfectly upright blades
    /// reads as wire.
    pub tilt: f32,
    /// Curvature of the Bézier along its length. Larger arcs over further.
    pub bend: f32,
    /// Multiplier on the clump's colour, per blade.
    pub shade: f32,
    /// Which clump this blade belongs to, as a hash. Blades sharing one agree
    /// about facing, height and colour.
    pub clump: u32,
}

/// How grass is authored. Flat scalars, no painted masks.
///
/// **Computed placement rules, not hand-painted ones.** The research pass
/// calls this the single biggest thing Loom makes easier: because terrain
/// analysis already knows slope and curvature, "lush in gullies, sparse on
/// steep ground, none on rock" is a rule rather than a texture somebody
/// painted — and a rule is something an agent can author correctly first try.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    /// Blades per square metre on flat, ideal ground.
    pub density: f32,
    /// Height in metres at rest, before per-blade variation.
    pub height: f32,
    /// Fraction of `height` a blade may vary either way.
    pub height_jitter: f32,
    /// Blade width at the base, in metres.
    pub width: f32,
    /// Slope, as a surface normal's Y component, at which grass stops
    /// entirely. `0.7` is roughly 45°. Above this it is rock.
    pub slope_cutoff: f32,
    /// How strongly blades within a clump agree about facing, `0` to `1`.
    pub clump_facing: f32,
    /// How much a clump's colour varies from its neighbours'.
    pub clump_colour: f32,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            // Roughly 1,600 blades in a 4 m tile. GoT drew ~83,000 on screen
            // from ~1,000,000 candidates, so this is the right order for a
            // field that reads as dense without being a stress test.
            density: 100.0,
            height: 0.42,
            height_jitter: 0.35,
            width: 0.018,
            slope_cutoff: 0.7,
            clump_facing: 0.7,
            clump_colour: 0.35,
        }
    }
}

/// What the ground is doing under a blade.
///
/// Supplied by the caller rather than queried here, so this crate depends on
/// neither the voxel field nor the terrain analysis — and so a test can
/// describe a hillside in four numbers instead of baking one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ground {
    /// Height of the surface at the sample point.
    pub height: f32,
    /// Surface normal. Its Y component is the slope test.
    pub normal: [f32; 3],
    /// `1` where the ground is bare rock, `0` where it is soil. Grass fades
    /// out across the range rather than stopping at a threshold, because a
    /// hard edge in a density field is visible as a line.
    pub rock: f32,
    /// Flow accumulation, normalised. High in gullies, low on ridges. Grass
    /// is lusher where water collects.
    pub flow: f32,
}

impl Default for Ground {
    fn default() -> Self {
        Self { height: 0.0, normal: [0.0, 1.0, 0.0], rock: 0.0, flow: 0.0 }
    }
}

/// Integer coordinates of one placement tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tile {
    pub x: i32,
    pub z: i32,
}

impl Tile {
    /// The tile containing a world position.
    #[must_use]
    pub fn at(x: f32, z: f32) -> Self {
        #[allow(clippy::cast_possible_truncation)]
        Self { x: (x / TILE).floor() as i32, z: (z / TILE).floor() as i32 }
    }

    /// The tile's minimum corner in world space.
    #[must_use]
    pub fn origin(self) -> [f32; 2] {
        #[allow(clippy::cast_precision_loss)]
        [self.x as f32 * TILE, self.z as f32 * TILE]
    }
}

/// A hash of a lattice cell, mixed enough that neighbours are unrelated.
///
/// Three rounds because two adjacent cells differ in one input bit, and one
/// round of any mixer leaves visible structure in the low bits — which shows
/// up as blades lining up along the grid, the exact artifact clumping exists
/// to hide.
#[must_use]
fn cell_hash(x: i32, z: i32, salt: u32) -> u32 {
    #[allow(clippy::cast_sign_loss)]
    hash(hash(hash(x as u32).wrapping_add(z as u32)).wrapping_add(salt))
}

/// A hash value as a float in `[0, 1)`.
///
/// The same top-24-bits trick the noise lattice uses, and exact for the same
/// reason — so a GPU computing this arrives at the identical number.
#[must_use]
fn unit(h: u32) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    {
        (h >> 8) as f32 * (1.0 / 16_777_216.0)
    }
}

/// The centre of the clump owning a cell, and its identity.
///
/// Jittered inside its cell rather than at the corner: a regular lattice of
/// clump centres is visible as a grid the moment the camera moves, which is
/// one of the motion-only traps the phase warns about.
fn clump_centre(cx: i32, cz: i32) -> ([f32; 2], u32) {
    let id = cell_hash(cx, cz, 0x9e37_79b9);
    #[allow(clippy::cast_precision_loss)]
    let base = [cx as f32 * CLUMP, cz as f32 * CLUMP];
    let jitter = [unit(id), unit(id.rotate_left(11))];
    ([base[0] + jitter[0] * CLUMP, base[1] + jitter[1] * CLUMP], id)
}

/// The clump nearest a point, searching the nine cells around it.
///
/// Nine, not one: a jittered centre can sit anywhere in its cell, so the
/// nearest centre to a point near a cell edge is often in the neighbour. This
/// is a Voronoi lookup, and checking only the home cell produces cells with
/// straight edges along the lattice — visible, and wrong.
fn nearest_clump(x: f32, z: f32) -> ([f32; 2], u32) {
    #[allow(clippy::cast_possible_truncation)]
    let (hx, hz) = ((x / CLUMP).floor() as i32, (z / CLUMP).floor() as i32);

    let mut best = f32::MAX;
    let mut winner = ([0.0, 0.0], 0);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let (centre, id) = clump_centre(hx + dx, hz + dz);
            let (ox, oz) = (centre[0] - x, centre[1] - z);
            let distance = ox.mul_add(ox, oz * oz);
            if distance < best {
                best = distance;
                winner = (centre, id);
            }
        }
    }
    winner
}

/// How much lusher the wettest gully is than ordinary ground.
///
/// **The candidate grid is sized for this, not for `density`.** Acceptance is
/// a probability, so it cannot exceed one — and if flat ground already
/// accepted everything, a gully would have nowhere to go and `flow` would do
/// nothing at all. That was the first version, and the gully test caught it:
/// ordinary ground now accepts `1 / LUSH` of a grid that is `LUSH` times
/// denser, which comes to the requested density and leaves the headroom.
const LUSH: f32 = 1.6;

/// The fraction of candidate positions that become blades, from `0` to `1`.
///
/// Every term is a smooth fade rather than a threshold. A hard cutoff in a
/// density field draws a visible line across a hillside, and the line moves
/// when the terrain is edited — which reads as the grass having a border.
#[must_use]
pub fn coverage(rules: &Rules, ground: &Ground) -> f32 {
    // Slope, from the normal's Y. Fades out over the last 15% rather than
    // stopping dead at the cutoff.
    let steepness = ((ground.normal[1] - rules.slope_cutoff) / 0.15).clamp(0.0, 1.0);
    // Rock, inverted: bare stone carries nothing.
    let soil = (1.0 - ground.rock).clamp(0.0, 1.0);
    // Gullies collect water, so they are lusher — but only up to a point,
    // because a river bed is not a lawn either.
    let lush = 1.0 + ground.flow.clamp(0.0, 1.0) * (LUSH - 1.0);
    (steepness * soil * lush / LUSH).clamp(0.0, 1.0)
}

/// Generate every blade in a tile.
///
/// `ground` is asked for each candidate position, so the caller decides
/// whether that is a heightfield lookup, a voxel SDF query or a flat plane.
///
/// **Candidates are hashed, then rejected.** The blade count follows from the
/// density and the coverage rather than being a loop bound, which is what
/// keeps a blade's identity independent of how many of its neighbours
/// survived — reject one and the rest are unmoved.
#[must_use]
pub fn tile(tile: Tile, rules: &Rules, ground: &dyn Fn(f32, f32) -> Ground) -> Vec<Blade> {
    let origin = tile.origin();

    // Candidates on a jittered grid. The research pass is explicit that full
    // blue noise is overkill for blades — the structure that matters is the
    // clumping, and a jittered grid under it is indistinguishable.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let side = ((rules.density * LUSH * TILE * TILE).sqrt().ceil() as u32).max(1);
    let step = TILE / f32::from(u16::try_from(side).unwrap_or(u16::MAX));

    let mut blades = Vec::new();
    for row in 0..side {
        for column in 0..side {
            let index = row * side + column;
            #[allow(clippy::cast_sign_loss)]
            let seed = cell_hash(tile.x, tile.z, index.wrapping_mul(0x0001_0001));

            // Jitter inside the cell, so the grid does not read as a grid.
            let jx = unit(seed);
            let jz = unit(seed.rotate_left(9));
            let x = f32::from(u16::try_from(column).unwrap_or(u16::MAX))
                .mul_add(step, origin[0] + jx * step);
            let z = f32::from(u16::try_from(row).unwrap_or(u16::MAX))
                .mul_add(step, origin[1] + jz * step);

            let under = ground(x, z);
            let cover = coverage(rules, &under);
            // Rejection sampling against the coverage, from the blade's own
            // hash. A blade's survival depends on nothing but its position.
            if unit(seed.rotate_left(19)) >= cover {
                continue;
            }

            blades.push(blade(x, z, &under, rules, seed));
        }
    }
    blades
}

/// Build one blade at a point that has already been accepted.
fn blade(x: f32, z: f32, ground: &Ground, rules: &Rules, seed: u32) -> Blade {
    let (centre, clump) = nearest_clump(x, z);

    // The clump's own character, so every blade in it agrees.
    let clump_angle = unit(clump) * std::f32::consts::TAU;
    let clump_height = unit(clump.rotate_left(7));
    let clump_shade = unit(clump.rotate_left(13));

    // The blade's own, so they are not identical either.
    let own_angle = unit(seed.rotate_left(3)) * std::f32::consts::TAU;
    let own_height = unit(seed.rotate_left(23));

    // Facing: blended between the clump's and the blade's own. This is the
    // knob that decides whether a field reads as tussocks or as carpet.
    let blend = rules.clump_facing.clamp(0.0, 1.0);
    let angle = own_angle + (clump_angle - own_angle) * blend;

    // Blades lean away from their clump centre, which is what makes a tussock
    // look like it grew outward from a root rather than being a disc of
    // parallel blades.
    let (ox, oz) = (x - centre[0], z - centre[1]);
    let spread = ox.mul_add(ox, oz * oz).sqrt() / CLUMP;

    let height = rules.height
        * (1.0 + (clump_height - 0.5) * rules.height_jitter)
        * (1.0 + (own_height - 0.5) * rules.height_jitter * 0.5);

    Blade {
        position: [x, ground.height, z],
        facing: [angle.cos(), angle.sin()],
        height: height.max(0.01),
        width: rules.width,
        // Rest tilt grows with distance from the clump centre.
        tilt: 0.08 + spread * 0.35,
        bend: 0.5 + unit(seed.rotate_left(29)) * 0.5,
        shade: 1.0 + (clump_shade - 0.5) * rules.clump_colour,
        clump,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> impl Fn(f32, f32) -> Ground {
        |_, _| Ground::default()
    }

    /// **The property everything else rests on.** Generate a tile twice and
    /// get the same blades, bit for bit — otherwise dirty-region regeneration
    /// silently reshuffles the field.
    #[test]
    fn a_tile_is_a_pure_function_of_its_coordinates() {
        let rules = Rules::default();

        let first = tile(Tile { x: 3, z: -7 }, &rules, &flat());
        let second = tile(Tile { x: 3, z: -7 }, &rules, &flat());

        assert_eq!(first.len(), second.len());
        assert_eq!(first, second);
        assert!(!first.is_empty(), "the tile is empty, so this proves nothing");
    }

    /// **The dirty-region property, stated directly.** Regenerating one tile
    /// cannot disturb its neighbours, because they are computed from their own
    /// coordinates and nothing else. This is what makes destroying terrain
    /// under grass a local operation.
    #[test]
    fn regenerating_one_tile_leaves_its_neighbours_untouched() {
        let rules = Rules::default();
        let neighbours: Vec<Vec<Blade>> = [(0, 0), (1, 0), (0, 1), (-1, 0)]
            .iter()
            .map(|(x, z)| tile(Tile { x: *x, z: *z }, &rules, &flat()))
            .collect();

        // A crater under tile (0,0): the ground there is now rock.
        let cratered = |x: f32, z: f32| {
            if Tile::at(x, z) == (Tile { x: 0, z: 0 }) {
                Ground { rock: 1.0, ..Ground::default() }
            } else {
                Ground::default()
            }
        };
        let after: Vec<Vec<Blade>> = [(0, 0), (1, 0), (0, 1), (-1, 0)]
            .iter()
            .map(|(x, z)| tile(Tile { x: *x, z: *z }, &rules, &cratered))
            .collect();

        assert!(after[0].is_empty(), "the cratered tile still has grass");
        for i in 1..4 {
            assert_eq!(after[i], neighbours[i], "neighbour {i} changed");
        }
    }

    /// Tiles do not know about each other, so nothing about their boundaries
    /// may be special — no gap, no seam, no doubled row.
    #[test]
    fn density_is_even_across_a_tile_boundary() {
        let rules = Rules::default();
        let left = tile(Tile { x: 0, z: 0 }, &rules, &flat());
        let right = tile(Tile { x: 1, z: 0 }, &rules, &flat());

        // Count blades in a metre-wide band either side of x = TILE.
        let near_edge = |blades: &[Blade], low: f32, high: f32| {
            blades.iter().filter(|b| b.position[0] >= low && b.position[0] < high).count()
        };
        let before = near_edge(&left, TILE - 1.0, TILE);
        let after = near_edge(&right, TILE, TILE + 1.0);

        let ratio = f64::from(u32::try_from(before).unwrap_or(0))
            / f64::from(u32::try_from(after).unwrap_or(1));
        assert!(
            (0.75..1.35).contains(&ratio),
            "the boundary is visible: {before} blades before, {after} after"
        );
    }

    /// **Grass thins on steep ground without an authored mask** — one of P2's
    /// exit criteria, and the thing the terrain analysis buys.
    #[test]
    fn grass_thins_on_a_slope_and_stops_on_rock() {
        let rules = Rules::default();

        let count = |normal_y: f32, rock: f32| {
            let ground = move |_: f32, _: f32| Ground {
                normal: [(1.0 - normal_y * normal_y).sqrt(), normal_y, 0.0],
                rock,
                ..Ground::default()
            };
            tile(Tile { x: 5, z: 5 }, &rules, &ground).len()
        };

        let level = count(1.0, 0.0);
        let leaning = count(0.8, 0.0);
        let steep = count(0.7, 0.0);
        let stone = count(1.0, 1.0);

        assert!(level > 0, "flat ground has no grass");
        assert!(leaning < level, "a slope is as lush as level ground");
        assert_eq!(steep, 0, "grass survives past the cutoff");
        assert_eq!(stone, 0, "grass grows on bare rock");
    }

    /// Gullies are lusher than ridges. Also a rule rather than a painted mask.
    #[test]
    fn grass_thickens_where_water_collects() {
        let rules = Rules { density: 60.0, ..Rules::default() };
        let with_flow = |flow: f32| {
            let ground = move |_: f32, _: f32| Ground { flow, ..Ground::default() };
            // Several tiles, because one tile's count is noisy.
            (0..6).map(|i| tile(Tile { x: i, z: 2 }, &rules, &ground).len()).sum::<usize>()
        };

        let ridge = with_flow(0.0);
        let gully = with_flow(1.0);

        assert!(gully > ridge, "a gully ({gully}) is no lusher than a ridge ({ridge})");
    }

    /// **Clumping is the realism lever**, so it has to actually do something:
    /// blades sharing a clump agree about facing far more than strangers do.
    #[test]
    fn blades_in_a_clump_agree_about_which_way_they_lean() {
        let rules = Rules::default();
        let blades = tile(Tile { x: 0, z: 0 }, &rules, &flat());

        let spread = |pairs: &[(&Blade, &Blade)]| {
            let total: f32 = pairs
                .iter()
                .map(|(a, b)| a.facing[0].mul_add(b.facing[0], a.facing[1] * b.facing[1]))
                .sum();
            total / f32::from(u16::try_from(pairs.len()).unwrap_or(1))
        };

        let mut same = Vec::new();
        let mut different = Vec::new();
        for (i, a) in blades.iter().enumerate() {
            for b in blades.iter().skip(i + 1).take(40) {
                if a.clump == b.clump {
                    same.push((a, b));
                } else if different.len() < 4000 {
                    different.push((a, b));
                }
            }
        }
        assert!(same.len() > 50, "only {} pairs share a clump", same.len());

        // Mean cosine between facings: 1 is identical, 0 is unrelated.
        let together = spread(&same);
        let apart = spread(&different);
        assert!(
            together > apart + 0.3,
            "clumpmates ({together}) lean no more alike than strangers ({apart})"
        );
    }

    /// Every blade is usable geometry. A zero-height or zero-width blade is a
    /// degenerate triangle, which is a shimmering pixel rather than a plant.
    #[test]
    fn every_blade_is_a_real_blade() {
        let rules = Rules::default();
        for coordinate in -3..3 {
            for b in tile(Tile { x: coordinate, z: coordinate * 2 }, &rules, &flat()) {
                assert!(b.height > 0.0 && b.height.is_finite(), "{b:?}");
                assert!(b.width > 0.0 && b.width.is_finite(), "{b:?}");
                assert!(b.shade > 0.0 && b.shade.is_finite(), "{b:?}");
                let length = b.facing[0].hypot(b.facing[1]);
                assert!((length - 1.0).abs() < 1e-3, "facing is not a unit vector: {b:?}");
                assert!(b.tilt.is_finite() && b.bend.is_finite(), "{b:?}");
            }
        }
    }

    /// Density is a request, not a guarantee — but it has to be in the right
    /// neighbourhood, or every budget downstream is wrong.
    #[test]
    fn the_blade_count_follows_the_requested_density() {
        let ground = flat();
        for density in [25.0_f32, 100.0, 400.0] {
            let rules = Rules { density, ..Rules::default() };
            #[allow(clippy::cast_precision_loss)]
            let count = tile(Tile { x: 0, z: 0 }, &rules, &ground).len() as f32;
            let expected = density * TILE * TILE;
            assert!(
                (count / expected - 1.0).abs() < 0.2,
                "asked for {expected} blades at density {density}, got {count}"
            );
        }
    }

    /// **Clump boundaries must not follow the lattice.**
    ///
    /// Searching only the home cell is cheaper and passes every other test
    /// here — it was tried, as a mutation, and nothing caught it. What it
    /// produces is Voronoi cells with straight edges along the grid, which is
    /// a rectangular pattern visible the moment the camera moves. The
    /// distinguishing property is that a point near a cell edge often belongs
    /// to a centre in the *neighbouring* cell, because centres are jittered.
    #[test]
    fn a_clump_can_own_points_outside_its_own_cell() {
        let mut crossings = 0;
        let mut total = 0;
        for i in 0..4000 {
            #[allow(clippy::cast_precision_loss)]
            let f = i as f32;
            let (x, z) = (f * 0.037 - 60.0, f * 0.023 - 40.0);
            #[allow(clippy::cast_possible_truncation)]
            let home = ((x / CLUMP).floor() as i32, (z / CLUMP).floor() as i32);
            let (_, owner) = nearest_clump(x, z);
            let (_, home_id) = clump_centre(home.0, home.1);
            total += 1;
            if owner != home_id {
                crossings += 1;
            }
        }

        // Jittered centres in a unit grid put roughly a fifth of points closer
        // to a neighbour than to their own cell's centre. Zero means the
        // neighbourhood search is not happening.
        assert!(
            crossings * 10 > total,
            "only {crossings} of {total} points are owned by a neighbouring \
             cell — clump boundaries are following the lattice"
        );
    }

    /// Clump cells tile the plane without gaps: every point has a nearest
    /// clump, and it is never further away than a cell diagonal.
    #[test]
    fn every_point_belongs_to_a_clump_near_it() {
        for i in 0..500 {
            #[allow(clippy::cast_precision_loss)]
            let f = i as f32;
            let (x, z) = (f * 0.31 - 40.0, f * -0.17 + 25.0);
            let (centre, _) = nearest_clump(x, z);
            let distance = (centre[0] - x).hypot(centre[1] - z);
            assert!(
                distance < CLUMP * 1.5,
                "nearest clump to ({x}, {z}) is {distance} away"
            );
        }
    }
}
