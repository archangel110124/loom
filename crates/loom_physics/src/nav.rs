//! Where a character can walk, and how to get there.
//!
//! **Probed, not authored.** A navmesh is usually baked from the level by a
//! separate tool and then goes stale whenever anyone moves a wall — which in
//! this project is constantly, because an agent edits scenes as a matter of
//! course. This is measured from the collision world instead, with the same
//! casts everything else here uses: drop a ray on each cell to find the floor,
//! and cast a capsule's worth of clearance above it to see whether a body
//! fits.
//!
//! **One layer.** Each column keeps the topmost surface and nothing else, so
//! a character can walk *over* a bridge and never *under* it. That is a real
//! limitation and it is stated rather than papered over: routing under an
//! overhang needs several cells per column, which is a different and larger
//! structure. It costs nothing in a blockout, where the ceilings are the sky.
//!
//! It also means a wall is not a special case. The probe finds the top of it,
//! the step check refuses the climb, and the cells on top become an island
//! nothing can reach — which is the same answer as "unwalkable" and takes no
//! code to express.
//!
//! A grid rather than a mesh. A navmesh is smaller and gives smoother paths,
//! and it is a great deal of code — polygon extraction, merging, portals,
//! funnel smoothing. A grid is an array and a loop, it suits a voxel world,
//! and at blockout scale the difference is invisible. `ponytail:` when a level
//! is big enough for the memory to matter, the seam is `NavGrid::path` — a
//! navmesh can answer the same question.
//!
//! Deterministic throughout, like everything else that feeds the simulation:
//! integer costs, ties broken by cell index, no hash iteration. Two runs
//! produce the same path or the assertion built on it is worthless.

use crate::Physics;

/// Millimetres. Costs are integers so the frontier orders identically on
/// every machine — comparing accumulated `f32` would let two equal-length
/// routes swap depending on the order they were summed.
type Cost = u32;

/// A walkable grid over part of the world.
pub struct NavGrid {
    /// World position of cell `(0, 0)`'s centre, on the XZ plane.
    origin: [f32; 2],
    /// Metres per cell.
    cell: f32,
    width: usize,
    depth: usize,
    /// Floor height per cell, and whether a body fits standing on it.
    floor: Vec<f32>,
    walkable: Vec<bool>,
}

/// How the grid is probed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavAgent {
    /// Tallest rise between neighbouring cells that still counts as connected.
    /// A step a character can walk up; anything taller is a wall to it.
    pub step: f32,
}

impl Default for NavAgent {
    fn default() -> Self {
        Self { step: 0.4 }
    }
}

impl NavGrid {
    /// Probe a box of the world for somewhere to stand.
    ///
    /// `min` and `max` are the XZ extent to cover; `ceiling` is where the
    /// downward probes start, and must be above anything walkable.
    ///
    /// # The world it probes is the one the last `step` left
    ///
    /// Same caveat as every query here: the tree these casts walk is built
    /// during [`Physics::step`]. Before the first one there is no floor
    /// anywhere and the whole grid comes back unwalkable.
    #[must_use]
    pub fn bake(
        physics: &Physics,
        min: [f32; 2],
        max: [f32; 2],
        cell: f32,
        ceiling: f32,
    ) -> Self {
        let cell = cell.max(0.05);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let width = (((max[0] - min[0]) / cell).ceil().max(1.0)) as usize;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let depth = (((max[1] - min[1]) / cell).ceil().max(1.0)) as usize;

        let mut floor = vec![f32::NAN; width * depth];
        let mut walkable = vec![false; width * depth];

        for z in 0..depth {
            for x in 0..width {
                #[allow(clippy::cast_precision_loss)]
                let at = [
                    min[0] + (x as f32 + 0.5) * cell,
                    min[1] + (z as f32 + 0.5) * cell,
                ];
                let from = [at[0], ceiling, at[1]];
                let Some(ground) = physics.raycast(from, [0.0, -1.0, 0.0], ceiling * 2.0 + 1.0)
                else {
                    continue;
                };

                floor[z * width + x] = ground.point[1];
                walkable[z * width + x] = true;
            }
        }

        Self {
            origin: min,
            cell,
            width,
            depth,
            floor,
            walkable,
        }
    }

    #[must_use]
    pub fn is_walkable(&self, at: [f32; 3]) -> bool {
        self.cell_of(at)
            .is_some_and(|(x, z)| self.walkable[z * self.width + x])
    }

    /// Cells that can be stood on, for reporting how much of a level is
    /// reachable at all.
    #[must_use]
    pub fn walkable_cells(&self) -> usize {
        self.walkable.iter().filter(|w| **w).count()
    }

    /// A route from `from` to `to`, as world positions, or empty when there
    /// is none.
    ///
    /// The first element is the next place to walk to, not where the caller
    /// already is — a path that starts where you stand makes every follower
    /// waste its first step deciding it has arrived.
    #[must_use]
    pub fn path(&self, from: [f32; 3], to: [f32; 3], agent: NavAgent) -> Vec<[f32; 3]> {
        let (Some(start), Some(goal)) = (self.cell_of(from), self.cell_of(to)) else {
            return Vec::new();
        };
        if !self.walkable[start.1 * self.width + start.0] {
            return Vec::new();
        }
        if start == goal {
            return Vec::new();
        }

        let count = self.width * self.depth;
        let mut best: Vec<Cost> = vec![Cost::MAX; count];
        let mut came: Vec<u32> = vec![u32::MAX; count];
        let start_index = start.1 * self.width + start.0;
        let goal_index = goal.1 * self.width + goal.0;
        best[start_index] = 0;

        // Min-heap by (estimated total, cell). The cell in the key is what
        // makes two equally promising routes resolve the same way every run.
        let mut frontier = std::collections::BinaryHeap::new();
        frontier.push(std::cmp::Reverse((self.heuristic(start, goal), start_index)));

        while let Some(std::cmp::Reverse((_, index))) = frontier.pop() {
            if index == goal_index {
                return self.walk_back(&came, start_index, goal_index);
            }
            let here = (index % self.width, index / self.width);
            let cost_here = best[index];

            for (nx, nz, step_cost) in self.neighbours(here) {
                let next = nz * self.width + nx;
                if !self.walkable[next] {
                    continue;
                }
                // A step too tall is a wall. Without this the grid happily
                // routes a character up the side of a crate.
                let rise = (self.floor[next] - self.floor[index]).abs();
                if !rise.is_finite() || rise > agent.step {
                    continue;
                }

                let candidate = cost_here.saturating_add(step_cost);
                if candidate < best[next] {
                    best[next] = candidate;
                    came[next] = u32::try_from(index).unwrap_or(u32::MAX);
                    frontier.push(std::cmp::Reverse((
                        candidate.saturating_add(self.heuristic((nx, nz), goal)),
                        next,
                    )));
                }
            }
        }

        Vec::new()
    }

    fn walk_back(&self, came: &[u32], start: usize, goal: usize) -> Vec<[f32; 3]> {
        let mut route = Vec::new();
        let mut at = goal;
        // Bounded: a came-from chain longer than the grid means a cycle, which
        // would hang the tick rather than merely producing a bad path.
        for _ in 0..came.len() {
            if at == start {
                break;
            }
            route.push(self.centre(at));
            let previous = came[at];
            if previous == u32::MAX {
                return Vec::new();
            }
            at = previous as usize;
        }
        route.reverse();
        route
    }

    /// The eight neighbours, with diagonals costing what they actually are.
    ///
    /// A diagonal charged the same as a straight step makes A* prefer
    /// staircase routes that look like a character cutting corners it cannot.
    fn neighbours(&self, (x, z): (usize, usize)) -> Vec<(usize, usize, Cost)> {
        const STRAIGHT: Cost = 1000;
        // 1000 * sqrt(2), rounded.
        const DIAGONAL: Cost = 1414;

        let mut out = Vec::with_capacity(8);
        for (dx, dz) in [
            (-1_i32, 0_i32),
            (1, 0),
            (0, -1),
            (0, 1),
            (-1, -1),
            (1, -1),
            (-1, 1),
            (1, 1),
        ] {
            let Ok(nx) = usize::try_from(x as i32 + dx) else {
                continue;
            };
            let Ok(nz) = usize::try_from(z as i32 + dz) else {
                continue;
            };
            if nx >= self.width || nz >= self.depth {
                continue;
            }
            let diagonal = dx != 0 && dz != 0;
            // A diagonal between two walls squeezes through a gap that is not
            // there. Both orthogonal neighbours have to be open.
            if diagonal
                && (!self.walkable[z * self.width + nx] || !self.walkable[nz * self.width + x])
            {
                continue;
            }
            out.push((nx, nz, if diagonal { DIAGONAL } else { STRAIGHT }));
        }
        out
    }

    /// Octile distance, which is exact for an eight-connected grid and so
    /// never overestimates — the property A* needs to stay optimal.
    fn heuristic(&self, (x, z): (usize, usize), (gx, gz): (usize, usize)) -> Cost {
        let dx = x.abs_diff(gx) as Cost;
        let dz = z.abs_diff(gz) as Cost;
        let (long, short) = if dx > dz { (dx, dz) } else { (dz, dx) };
        (long - short) * 1000 + short * 1414
    }

    fn cell_of(&self, at: [f32; 3]) -> Option<(usize, usize)> {
        let x = (at[0] - self.origin[0]) / self.cell;
        let z = (at[2] - self.origin[1]) / self.cell;
        if x < 0.0 || z < 0.0 || !x.is_finite() || !z.is_finite() {
            return None;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (x, z) = (x as usize, z as usize);
        (x < self.width && z < self.depth).then_some((x, z))
    }

    fn centre(&self, index: usize) -> [f32; 3] {
        let (x, z) = (index % self.width, index / self.width);
        #[allow(clippy::cast_precision_loss)]
        let out = [
            self.origin[0] + (x as f32 + 0.5) * self.cell,
            self.floor[index],
            self.origin[1] + (z as f32 + 0.5) * self.cell,
        ];
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    /// A twenty-metre floor with whatever is put on it.
    fn room(build: impl FnOnce(&mut Physics)) -> Physics {
        let mut physics = Physics::new(1.0 / 60.0);
        physics.add_static_box([0.0, -0.5, 0.0], FLAT, [10.0, 0.5, 10.0]);
        build(&mut physics);
        physics.step();
        physics
    }

    fn grid(physics: &Physics) -> NavGrid {
        NavGrid::bake(
            physics,
            [-10.0, -10.0],
            [10.0, 10.0],
            0.5,
            20.0,
        )
    }

    #[test]
    fn open_floor_is_walkable() {
        let grid = grid(&room(|_| {}));

        assert!(grid.is_walkable([0.0, 0.0, 0.0]));
        assert!(grid.is_walkable([7.0, 0.0, -6.0]));
        assert!(!grid.is_walkable([50.0, 0.0, 0.0]), "outside the grid");
    }

    /// **One layer, stated in a test.** The probe keeps the topmost surface,
    /// so the top of a slab is walkable and the floor beneath it is not even
    /// considered. Routing under an overhang needs several cells per column,
    /// which this deliberately is not.
    #[test]
    fn only_the_topmost_surface_is_known() {
        let grid = grid(&room(|physics| {
            physics.add_static_box([4.0, 1.0, 0.0], FLAT, [2.0, 0.1, 2.0]);
        }));

        assert!(grid.is_walkable([4.0, 0.0, 0.0]), "the top of the slab");
        assert!(
            grid.path([-4.0, 0.0, 0.0], [4.0, 0.0, 0.0], NavAgent::default())
                .is_empty(),
            "and it is an island: 1.1 m is not a step"
        );
    }

    #[test]
    fn a_path_across_open_floor_goes_roughly_straight() {
        let grid = grid(&room(|_| {}));

        let route = grid.path([-6.0, 0.0, 0.0], [6.0, 0.0, 0.0], NavAgent::default());

        assert!(!route.is_empty(), "no route across an empty floor");
        // Straight enough that nothing wanders: twelve metres at half-metre
        // cells is 24 steps, and a detour would show immediately.
        assert!(route.len() < 30, "wandered: {} steps", route.len());
        for step in &route {
            assert!(step[2].abs() < 2.0, "strayed off the line: {step:?}");
        }
    }

    /// **The point of pathfinding.** A straight line is blocked; a route
    /// exists around the end of the wall, and it has to be found.
    #[test]
    fn a_path_goes_around_a_wall() {
        let grid = grid(&room(|physics| {
            // A wall across the middle, open at the north end only.
            physics.add_static_box([0.0, 1.5, -3.0], FLAT, [0.4, 1.5, 7.0]);
        }));

        let route = grid.path([-5.0, 0.0, -6.0], [5.0, 0.0, -6.0], NavAgent::default());

        assert!(!route.is_empty(), "no way round the wall");
        // It has to go the long way, so it cannot be near the direct distance.
        assert!(route.len() > 24, "cut through the wall: {} steps", route.len());
        // And it must actually clear the wall's open end.
        assert!(
            route.iter().any(|s| s[2] > 3.5),
            "never went round: {:?}",
            route.last()
        );
    }

    #[test]
    fn there_is_no_path_into_a_sealed_room() {
        let grid = grid(&room(|physics| {
            physics.add_static_box([-6.0, 1.5, -2.0], FLAT, [0.3, 1.5, 2.0]);
            physics.add_static_box([-6.0, 1.5, 2.0], FLAT, [0.3, 1.5, 2.0]);
            physics.add_static_box([-8.0, 1.5, 0.0], FLAT, [2.0, 1.5, 0.3]);
            physics.add_static_box([-6.0, 1.5, 0.0], FLAT, [0.3, 1.5, 2.0]);
            physics.add_static_box([-8.0, 1.5, -4.0], FLAT, [2.3, 1.5, 0.3]);
            physics.add_static_box([-8.0, 1.5, 4.0], FLAT, [2.3, 1.5, 0.3]);
            physics.add_static_box([-10.3, 1.5, 0.0], FLAT, [0.3, 1.5, 4.3]);
        }));

        let route = grid.path([0.0, 0.0, 0.0], [-8.0, 0.0, 0.0], NavAgent::default());

        assert!(route.is_empty(), "walked through a sealed wall: {route:?}");
    }

    /// A step is climbable, a crate is not. Without the rise check a grid
    /// routes a character straight up the side of anything.
    #[test]
    fn a_route_climbs_a_step_but_not_a_crate() {
        let over_a_step = grid(&room(|physics| {
            physics.add_static_box([0.0, 0.1, 0.0], FLAT, [1.5, 0.1, 6.0]);
        }));
        let over_a_crate = grid(&room(|physics| {
            physics.add_static_box([0.0, 0.6, 0.0], FLAT, [1.5, 0.6, 6.0]);
        }));

        let across = |g: &NavGrid| g.path([-5.0, 0.0, 0.0], [5.0, 0.0, 0.0], NavAgent::default());

        let stepped = across(&over_a_step);
        let blocked = across(&over_a_crate);
        assert!(!stepped.is_empty(), "a 20 cm step should be walkable");
        assert!(
            blocked.len() > stepped.len(),
            "a 1.2 m crate should force a detour: {} vs {}",
            blocked.len(),
            stepped.len()
        );
    }

    /// **The property an assertion rests on.** Same world, same route, every
    /// run — a path that varies between runs makes every test built on an
    /// enemy's behaviour flaky.
    #[test]
    fn the_same_world_gives_the_same_path() {
        let physics = room(|physics| {
            physics.add_static_box([0.0, 1.5, -3.0], FLAT, [0.4, 1.5, 7.0]);
        });
        let grid = grid(&physics);

        let first = grid.path([-5.0, 0.0, -6.0], [5.0, 0.0, -6.0], NavAgent::default());
        let second = grid.path([-5.0, 0.0, -6.0], [5.0, 0.0, -6.0], NavAgent::default());

        assert_eq!(first, second);
        assert!(!first.is_empty());
    }

    #[test]
    fn a_path_to_where_you_already_are_is_empty() {
        let grid = grid(&room(|_| {}));

        assert!(grid.path([1.0, 0.0, 1.0], [1.1, 0.0, 1.1], NavAgent::default()).is_empty());
    }
}
