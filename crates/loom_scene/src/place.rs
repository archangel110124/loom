//! Semantic placement: geometry-aware verbs instead of raw coordinates.
//!
//! Design doc §2.8 calls this the most important 3D decision in the project,
//! and the reasoning is worth keeping in front of you: **do not make the agent
//! compute world coordinates.** It gets them wrong constantly, and that is
//! precisely the failure that leaves a monitor floating 30cm above a desk.
//!
//! "Six desks in two rows with a monitor on each" becomes four calls that are
//! *correct by construction*, instead of forty coordinate writes that need
//! visual correction afterwards. The burden moves off the model's spatial
//! reasoning — its weakest faculty — and onto deterministic geometry code,
//! which is the strongest thing available here.

use serde::{Deserialize, Serialize};

use crate::ops::SceneOp;

/// A world axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }
}

/// Where on a surface to sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    #[default]
    Center,
    Front,
    Back,
    Left,
    Right,
}

/// An axis-aligned box in world space — what placement reasons about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    #[must_use]
    pub fn center(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    #[must_use]
    pub fn size(&self) -> [f32; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    /// Whether two boxes overlap on every axis.
    ///
    /// Touching is not overlapping: a monitor resting exactly on a desk shares
    /// a plane, and reporting that as interpenetration would fire the check on
    /// correct placements.
    ///
    /// The tolerance is not cosmetic. `place_on` computes a resting height by
    /// adding half-extents, and that never lands exactly on the surface top in
    /// floating point — 0.4+0.4 and 1.05-0.25 differ in the last bit. Without
    /// slack, every correctly placed object reports as interpenetrating.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.overlaps_within(other, 1e-4)
    }

    /// [`Self::overlaps`] with an explicit tolerance.
    #[must_use]
    pub fn overlaps_within(&self, other: &Self, tolerance: f32) -> bool {
        (0..3).all(|i| {
            self.min[i] + tolerance < other.max[i] && other.min[i] + tolerance < self.max[i]
        })
    }
}

/// A geometry-aware placement request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "place", rename_all = "snake_case")]
pub enum PlaceOp {
    /// Sit `node` on `surface`'s upper face.
    PlaceOn {
        node: String,
        surface: String,
        #[serde(default)]
        anchor: Anchor,
    },
    /// Distribute `nodes` evenly along an axis, centred on their own midpoint.
    AlignTo {
        nodes: Vec<String>,
        axis: Axis,
        spacing: f32,
    },
    /// Yaw `node` so its forward vector points at `target`.
    FaceToward { node: String, target: String },
    /// Lay out `count` copies on `surface` in a grid.
    GridOn {
        /// Node names are `{prefix}{n}`, starting at 0.
        prefix: String,
        surface: String,
        mesh: String,
        rows: u32,
        cols: u32,
        pitch: [f32; 2],
        /// Height above the surface to sit at.
        #[serde(default)]
        lift: f32,
    },
}

/// Everything placement needs to know about the scene.
///
/// A trait would be premature here (never-do #12); a lookup closure is enough
/// and keeps this module free of scene-loading concerns.
pub struct Geometry<'a> {
    /// World bounds of a node, or `None` if it has no geometry.
    pub bounds_of: &'a dyn Fn(&str) -> Option<Bounds>,
    /// The parent path new nodes should be created under.
    pub parent: String,
}

/// Why a placement could not be computed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlaceError {
    pub error: String,
    pub node: String,
    pub constraint: String,
    pub hint: String,
}

/// Turn a placement request into concrete ops.
///
/// This is where the arithmetic happens, once, in engine code — instead of in
/// the model, every time, differently.
///
/// # Errors
/// [`PlaceError`] if a referenced node has no geometry to place against.
pub fn resolve(op: &PlaceOp, geometry: &Geometry<'_>) -> Result<Vec<SceneOp>, PlaceError> {
    let missing = |name: &str| PlaceError {
        error: "no_geometry".to_owned(),
        node: name.to_owned(),
        constraint: format!("`{name}` has no bounds to place against"),
        hint: "The node must exist and carry a MeshRenderer. Spawn it first."
            .to_owned(),
    };

    match op {
        PlaceOp::PlaceOn {
            node,
            surface,
            anchor,
        } => {
            let target = (geometry.bounds_of)(surface).ok_or_else(|| missing(surface))?;
            let moving = (geometry.bounds_of)(node).ok_or_else(|| missing(node))?;

            let center = target.center();
            let size = target.size();
            let (x, z) = match anchor {
                Anchor::Center => (center[0], center[2]),
                Anchor::Front => (center[0], target.max[2] - size[2] * 0.25),
                Anchor::Back => (center[0], target.min[2] + size[2] * 0.25),
                Anchor::Left => (target.min[0] + size[0] * 0.25, center[2]),
                Anchor::Right => (target.max[0] - size[0] * 0.25, center[2]),
            };

            // Sit the mover's *bottom* on the surface's top. Using its centre
            // is what puts a monitor half-buried in a desk.
            let half_height = moving.size()[1] * 0.5;
            Ok(vec![SceneOp::SetTransform {
                node: node.clone(),
                pos: Some([x, target.max[1] + half_height, z]),
                rot_euler: None,
                scale: None,
            }])
        }

        PlaceOp::AlignTo {
            nodes,
            axis,
            spacing,
        } => {
            if nodes.is_empty() {
                return Ok(Vec::new());
            }
            let index = axis.index();
            // Centre the run on where the group already is, so aligning does
            // not also teleport it somewhere unrelated.
            let mut sum = 0.0;
            for name in nodes {
                sum += (geometry.bounds_of)(name)
                    .ok_or_else(|| missing(name))?
                    .center()[index];
            }
            #[allow(clippy::cast_precision_loss)]
            let midpoint = sum / nodes.len() as f32;
            #[allow(clippy::cast_precision_loss)]
            let span = spacing * (nodes.len() as f32 - 1.0);
            let start = midpoint - span * 0.5;

            let mut ops = Vec::new();
            for (i, name) in nodes.iter().enumerate() {
                let bounds = (geometry.bounds_of)(name).ok_or_else(|| missing(name))?;
                let mut pos = bounds.center();
                #[allow(clippy::cast_precision_loss)]
                {
                    pos[index] = start + spacing * i as f32;
                }
                ops.push(SceneOp::SetTransform {
                    node: name.clone(),
                    pos: Some(pos),
                    rot_euler: None,
                    scale: None,
                });
            }
            Ok(ops)
        }

        PlaceOp::FaceToward { node, target } => {
            let from = (geometry.bounds_of)(node).ok_or_else(|| missing(node))?.center();
            let to = (geometry.bounds_of)(target)
                .ok_or_else(|| missing(target))?
                .center();

            // Yaw only. Pitching a desk to face something would be wrong, and
            // the agent asking for "facing" always means around up.
            let yaw = (to[0] - from[0]).atan2(to[2] - from[2]).to_degrees();
            Ok(vec![SceneOp::SetTransform {
                node: node.clone(),
                pos: None,
                rot_euler: Some([0.0, yaw, 0.0]),
                scale: None,
            }])
        }

        PlaceOp::GridOn {
            prefix,
            surface,
            mesh,
            rows,
            cols,
            pitch,
            lift,
        } => {
            let target = (geometry.bounds_of)(surface).ok_or_else(|| missing(surface))?;
            let center = target.center();
            #[allow(clippy::cast_precision_loss)]
            let (span_x, span_z) = (
                pitch[0] * (*cols as f32 - 1.0),
                pitch[1] * (*rows as f32 - 1.0),
            );

            let mut ops = Vec::new();
            for row in 0..*rows {
                for col in 0..*cols {
                    let index = row * cols + col;
                    let name = format!("{prefix}{index}");
                    #[allow(clippy::cast_precision_loss)]
                    let pos = [
                        center[0] - span_x * 0.5 + pitch[0] * col as f32,
                        target.max[1] + lift,
                        center[2] - span_z * 0.5 + pitch[1] * row as f32,
                    ];
                    ops.push(SceneOp::SpawnNode {
                        parent: geometry.parent.clone(),
                        name: name.clone(),
                        mesh: Some(mesh.clone()),
                    });
                    ops.push(SceneOp::SetTransform {
                        node: format!("{}/{name}", geometry.parent),
                        pos: Some(pos),
                        rot_euler: None,
                        scale: None,
                    });
                }
            }
            Ok(ops)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(center: [f32; 3], size: [f32; 3]) -> Bounds {
        Bounds {
            min: [
                center[0] - size[0] * 0.5,
                center[1] - size[1] * 0.5,
                center[2] - size[2] * 0.5,
            ],
            max: [
                center[0] + size[0] * 0.5,
                center[1] + size[1] * 0.5,
                center[2] + size[2] * 0.5,
            ],
        }
    }

    fn geometry<'a>(lookup: &'a dyn Fn(&str) -> Option<Bounds>) -> Geometry<'a> {
        Geometry {
            bounds_of: lookup,
            parent: "Room".to_owned(),
        }
    }

    /// **The failure §2.8 exists to prevent**: a monitor floating above — or
    /// buried in — a desk. Its bottom must meet the desk's top exactly.
    #[test]
    fn place_on_sits_the_bottom_on_the_surface_top() {
        let lookup = |name: &str| match name {
            "Desk" => Some(bounds([2.0, 0.4, -1.0], [1.6, 0.8, 0.8])),
            "Monitor" => Some(bounds([0.0, 0.0, 0.0], [0.6, 0.5, 0.1])),
            _ => None,
        };
        let ops = resolve(
            &PlaceOp::PlaceOn {
                node: "Monitor".into(),
                surface: "Desk".into(),
                anchor: Anchor::Center,
            },
            &geometry(&lookup),
        )
        .expect("both exist");

        let SceneOp::SetTransform { pos: Some(pos), .. } = &ops[0] else {
            panic!("expected a transform, got {ops:?}");
        };
        // Desk top is 0.4 + 0.4 = 0.8. Monitor half-height 0.25.
        assert!((pos[1] - 1.05).abs() < 1e-4, "monitor at y={}", pos[1]);
        assert!((pos[0] - 2.0).abs() < 1e-4, "centred over the desk");
    }

    #[test]
    fn align_to_spaces_evenly_and_keeps_the_group_where_it_was() {
        let lookup = |name: &str| match name {
            "A" => Some(bounds([0.0, 0.0, 0.0], [1.0; 3])),
            "B" => Some(bounds([0.3, 0.0, 0.0], [1.0; 3])),
            "C" => Some(bounds([5.7, 0.0, 0.0], [1.0; 3])),
            _ => None,
        };
        let ops = resolve(
            &PlaceOp::AlignTo {
                nodes: vec!["A".into(), "B".into(), "C".into()],
                axis: Axis::X,
                spacing: 2.0,
            },
            &geometry(&lookup),
        )
        .expect("all exist");

        let xs: Vec<f32> = ops
            .iter()
            .map(|op| match op {
                SceneOp::SetTransform { pos: Some(p), .. } => p[0],
                _ => panic!("expected transforms"),
            })
            .collect();
        assert!((xs[1] - xs[0] - 2.0).abs() < 1e-4, "even spacing");
        assert!((xs[2] - xs[1] - 2.0).abs() < 1e-4);
        // Midpoint of 0.0, 0.3, 5.7 is 2.0 — the group stays put.
        assert!((xs[1] - 2.0).abs() < 1e-4, "centred on {xs:?}");
    }

    #[test]
    fn face_toward_yaws_only() {
        let lookup = |name: &str| match name {
            "Teacher" => Some(bounds([0.0, 0.0, -4.0], [1.0; 3])),
            "Class" => Some(bounds([0.0, 3.0, 0.0], [1.0; 3])),
            _ => None,
        };
        let ops = resolve(
            &PlaceOp::FaceToward {
                node: "Teacher".into(),
                target: "Class".into(),
            },
            &geometry(&lookup),
        )
        .unwrap();

        let SceneOp::SetTransform {
            rot_euler: Some(rot),
            pos,
            ..
        } = &ops[0]
        else {
            panic!("expected a rotation");
        };
        assert_eq!(rot[0], 0.0, "no pitch — a desk does not tilt to face you");
        assert_eq!(rot[2], 0.0, "no roll");
        assert!(pos.is_none(), "facing must not move the node");
        // Target is +Z from the node, so yaw 0.
        assert!(rot[1].abs() < 1e-3, "yaw {}", rot[1]);
    }

    /// "Six desks in two rows" — the exact phrase from the M9 exit criterion.
    #[test]
    fn grid_on_lays_out_six_desks_in_two_rows() {
        let lookup = |name: &str| match name {
            "Floor" => Some(bounds([0.0, 0.0, 0.0], [12.0, 0.2, 8.0])),
            _ => None,
        };
        let ops = resolve(
            &PlaceOp::GridOn {
                prefix: "Desk".into(),
                surface: "Floor".into(),
                mesh: "box".into(),
                rows: 2,
                cols: 3,
                pitch: [2.5, 3.0],
                lift: 0.4,
            },
            &geometry(&lookup),
        )
        .expect("floor exists");

        // One spawn plus one transform per desk.
        assert_eq!(ops.len(), 12, "6 desks");
        let names: Vec<String> = ops
            .iter()
            .filter_map(|op| match op {
                SceneOp::SpawnNode { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["Desk0", "Desk1", "Desk2", "Desk3", "Desk4", "Desk5"]);

        let ys: Vec<f32> = ops
            .iter()
            .filter_map(|op| match op {
                SceneOp::SetTransform { pos: Some(p), .. } => Some(p[1]),
                _ => None,
            })
            .collect();
        assert!(ys.iter().all(|y| (*y - 0.5).abs() < 1e-4), "all on the floor: {ys:?}");
    }

    /// Touching is not overlapping — a monitor resting exactly on a desk
    /// shares a plane, and flagging that would fire on correct placements.
    #[test]
    fn touching_boxes_do_not_count_as_overlapping() {
        let desk = bounds([0.0, 0.4, 0.0], [2.0, 0.8, 1.0]);
        let resting = bounds([0.0, 1.05, 0.0], [0.6, 0.5, 0.1]);
        let buried = bounds([0.0, 0.7, 0.0], [0.6, 0.5, 0.1]);

        assert!(!desk.overlaps(&resting), "resting on top is not overlap");
        assert!(desk.overlaps(&buried), "buried IS overlap");
    }

    #[test]
    fn placing_against_a_missing_node_says_what_is_missing() {
        let lookup = |_: &str| None;
        let err = resolve(
            &PlaceOp::PlaceOn {
                node: "Monitor".into(),
                surface: "Ghost".into(),
                anchor: Anchor::Center,
            },
            &geometry(&lookup),
        )
        .expect_err("no geometry");

        assert_eq!(err.error, "no_geometry");
        assert_eq!(err.node, "Ghost");
        assert!(err.hint.contains("Spawn it first"));
    }
}
