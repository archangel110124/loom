//! The viewport's transform handles, and the projection they share with picking.
//!
//! Drawn by egui rather than by a second Vulkan pass. A gizmo is six lines and
//! three dots; giving it a pipeline, a depth prepass and an ID buffer would be
//! the obsolete style the project rules warn about, for a widget that has to
//! ignore depth anyway.
//!
//! **Picking and the gizmo project the same way.** Not "equivalently" — the
//! same function. A handle that sits a few pixels off the thing it moves is the
//! kind of bug that survives for months because each half looks right alone.

use loom_render::glam::Vec3;
use loom_render::Camera;

/// Which of a node's transform fields the handles edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Move,
    Rotate,
    Scale,
}

impl Mode {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::Rotate => "Rotate",
            Self::Scale => "Scale",
        }
    }
}

/// Handle length on screen, in pixels. Fixed rather than in world units, so a
/// handle stays grabbable whether you are on top of a node or across the map.
pub const HANDLE_PIXELS: f32 = 90.0;
/// How close the cursor must come to a handle to grab it.
pub const GRAB_PIXELS: f32 = 10.0;

/// The camera's basis, computed once per frame and shared by everything that
/// converts between the world and the window.
pub struct View {
    eye: Vec3,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    tan_half_fov: f32,
    /// Where the scene's rectangle starts, in window pixels.
    ///
    /// **The mapping lives here rather than at each consumer**, and that is
    /// the point. `pick_at_cursor`, `drag_gizmo`, `press_in_viewport`, the
    /// handle recomputation and `agent_marks` all speak window pixels, because
    /// that is what winit reports and what egui draws in. Asking each of them
    /// to convert would be five places to get it right and five places for a
    /// later reader to miss one — and a missed one is a click that selects the
    /// wrong object, which reads as a picking bug rather than as a coordinate
    /// bug.
    origin: (f32, f32),
    width: f32,
    height: f32,
}

impl View {
    #[must_use]
    pub fn new(camera: &Camera, width: f32, height: f32) -> Self {
        Self::at(camera, (0.0, 0.0), width, height)
    }

    /// A view onto a scene drawn into a sub-rectangle of the window.
    ///
    /// `origin` is where that rectangle starts, and `width`/`height` are its
    /// size — the same numbers the renderer's `ViewportPlacement` carries. The
    /// aspect ratio comes from the rectangle, matching the projection the
    /// renderer actually used; taking it from the window instead is how the
    /// gizmo ends up slightly off the object it is attached to.
    #[must_use]
    pub fn at(camera: &Camera, origin: (f32, f32), width: f32, height: f32) -> Self {
        let forward = (camera.target - camera.eye).normalize_or_zero();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        Self {
            eye: camera.eye,
            forward,
            right,
            up: right.cross(forward),
            tan_half_fov: (camera.fov_y_degrees.to_radians() * 0.5).tan(),
            origin,
            width,
            height,
        }
    }

    /// A window pixel in viewport space.
    #[must_use]
    pub fn to_viewport(&self, window: (f32, f32)) -> (f32, f32) {
        (window.0 - self.origin.0, window.1 - self.origin.1)
    }

    /// A viewport pixel in window space.
    #[must_use]
    pub fn to_window(&self, viewport: (f32, f32)) -> (f32, f32) {
        (viewport.0 + self.origin.0, viewport.1 + self.origin.1)
    }

    /// A world point in window pixels, or `None` when it is behind the camera.
    #[must_use]
    pub fn project(&self, world: Vec3) -> Option<(f32, f32)> {
        let v = world - self.eye;
        let z = v.dot(self.forward);
        // Not just `z <= 0`: a point on the plane divides by zero, and one a
        // hair in front projects to infinity and draws a handle across the
        // whole screen.
        if z < 0.01 {
            return None;
        }
        let aspect = self.width / self.height;
        let ndc_x = v.dot(self.right) / (z * self.tan_half_fov * aspect);
        let ndc_y = v.dot(self.up) / (z * self.tan_half_fov);
        Some(self.to_window((
            (ndc_x + 1.0) * 0.5 * self.width,
            (1.0 - ndc_y) * 0.5 * self.height,
        )))
    }

    /// A window pixel as a world-space ray direction. The inverse of
    /// [`Self::project`], and the reason picking and the gizmo agree.
    #[must_use]
    pub fn ray(&self, x: f32, y: f32) -> Vec3 {
        let (x, y) = self.to_viewport((x, y));
        let aspect = self.width / self.height;
        let ndc_x = (x / self.width) * 2.0 - 1.0;
        let ndc_y = 1.0 - (y / self.height) * 2.0;
        (self.forward
            + self.right * (ndc_x * self.tan_half_fov * aspect)
            + self.up * (ndc_y * self.tan_half_fov))
            .normalize_or_zero()
    }

    #[must_use]
    pub fn eye(&self) -> Vec3 {
        self.eye
    }
}

/// One handle: where it is drawn and what it edits.
#[derive(Clone)]
pub struct Handle {
    pub axis: usize,
    pub origin: (f32, f32),
    pub tip: (f32, f32),
    /// World units per pixel of drag along this handle, at the node's depth.
    pub scale: f32,
}

/// The three handles for a node at `origin`, in window pixels.
///
/// Empty when the node is behind the camera or so edge-on that a handle would
/// collapse to a point — dragging a zero-length handle would multiply the
/// mouse delta by infinity.
#[must_use]
pub fn handles(view: &View, origin: Vec3) -> Vec<Handle> {
    let Some(screen) = view.project(origin) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for axis in 0..3 {
        let mut direction = Vec3::ZERO;
        direction[axis] = 1.0;
        let Some(end) = view.project(origin + direction) else {
            continue;
        };
        let (dx, dy) = (end.0 - screen.0, end.1 - screen.1);
        let pixels_per_unit = (dx * dx + dy * dy).sqrt();
        if pixels_per_unit < 1.0 {
            continue;
        }
        let unit = (dx / pixels_per_unit, dy / pixels_per_unit);
        out.push(Handle {
            axis,
            origin: screen,
            tip: (
                screen.0 + unit.0 * HANDLE_PIXELS,
                screen.1 + unit.1 * HANDLE_PIXELS,
            ),
            scale: 1.0 / pixels_per_unit,
        });
    }
    out
}

/// How many points a rotation ring is drawn with. Enough that the arc reads as
/// a circle at gizmo size and few enough to hit-test in a loop.
const RING_SEGMENTS: usize = 48;

/// How wide a rotation ring is drawn, in window pixels. A little larger than
/// the axis handles so the caps stay reachable inside it.
const RING_PIXELS: f32 = 78.0;

/// A ring per axis, in screen space — ADR 0099.
///
/// **Rotate needs a different shape from move.** Three straight lines are the
/// right picture for translation and the wrong one for rotation: dragging along
/// a line to turn a thing has no relationship to the motion, and there is
/// nothing on screen saying which plane an axis turns in. A ring is the plane.
///
/// Returned as projected polylines because everything else the gizmo hands out
/// is already in window pixels, and the caller draws rather than computes.
#[must_use]
pub fn rings(view: &View, origin: Vec3) -> Vec<(usize, Vec<(f32, f32)>)> {
    // **A constant size on screen, like the handles.** Sizing the ring to the
    // object put a ring half the boat's diagonal across the window — bigger
    // than the viewport, running off every edge, and impossible to aim at. A
    // gizmo is a control, and a control does not change size when you zoom.
    let Some(screen) = view.project(origin) else {
        return Vec::new();
    };
    let Some(along) = view.project(origin + Vec3::X) else {
        return Vec::new();
    };
    let (dx, dy) = (along.0 - screen.0, along.1 - screen.1);
    let pixels_per_unit = (dx * dx + dy * dy).sqrt();
    if pixels_per_unit < 1.0 {
        return Vec::new();
    }
    let radius = RING_PIXELS / pixels_per_unit;

    let mut out = Vec::new();
    for axis in 0..3 {
        // The two world directions that span the plane this axis turns in.
        let (u, v) = match axis {
            0 => (Vec3::Y, Vec3::Z),
            1 => (Vec3::Z, Vec3::X),
            _ => (Vec3::X, Vec3::Y),
        };
        let mut points = Vec::with_capacity(RING_SEGMENTS + 1);
        for step in 0..=RING_SEGMENTS {
            #[allow(clippy::cast_precision_loss)]
            let angle = step as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
            let at = origin + (u * angle.cos() + v * angle.sin()) * radius;
            // A ring that crosses behind the eye is drawn in the pieces that
            // are in front of it, rather than joined across the whole window.
            match view.project(at) {
                Some(point) => points.push(point),
                None => break,
            }
        }
        if points.len() > 2 {
            out.push((axis, points));
        }
    }
    out
}

/// A quad between two axes, for moving in that plane — ADR 0099.
///
/// **Most placement is two-dimensional.** Sliding a crate along the deck is a
/// move in XZ, and doing it with two separate axis drags is two gestures and
/// two chances to nudge the height by accident.
pub struct Plane {
    /// The two axes this quad spans.
    pub axes: (usize, usize),
    /// Its corners in window pixels, in order round the quad.
    pub corners: [(f32, f32); 4],
}

/// How far along each handle a plane quad sits, and how big it is. Near the
/// origin so it does not swallow the axis caps.
const PLANE_NEAR: f32 = 0.26;
const PLANE_FAR: f32 = 0.62;

/// The three plane quads, built from the axis handles — ADR 0099.
///
/// Derived from the handles rather than re-projected, so a plane and its two
/// axes cannot disagree about where they are or how the drag is geared.
#[must_use]
pub fn planes(handles: &[Handle]) -> Vec<Plane> {
    let mut out = Vec::new();
    for (i, j) in [(0, 1), (1, 2), (2, 0)] {
        let (Some(a), Some(b)) = (
            handles.iter().find(|h| h.axis == i),
            handles.iter().find(|h| h.axis == j),
        ) else {
            continue;
        };
        let origin = a.origin;
        let along = |h: &Handle, t: f32| {
            (
                origin.0 + (h.tip.0 - origin.0) * t,
                origin.1 + (h.tip.1 - origin.1) * t,
            )
        };
        let (an, af) = (along(a, PLANE_NEAR), along(a, PLANE_FAR));
        let (bn, bf) = (along(b, PLANE_NEAR), along(b, PLANE_FAR));
        // The parallelogram those two spans make.
        let corner = |p: (f32, f32), q: (f32, f32)| {
            (p.0 + q.0 - origin.0, p.1 + q.1 - origin.1)
        };
        out.push(Plane {
            axes: (i, j),
            corners: [
                corner(an, bn),
                corner(af, bn),
                corner(af, bf),
                corner(an, bf),
            ],
        });
    }
    out
}

/// Which plane quad the cursor is inside, if any — ADR 0099.
#[must_use]
pub fn grab_plane(planes: &[Plane], cursor: (f32, f32)) -> Option<(usize, usize)> {
    planes
        .iter()
        .find(|plane| inside(&plane.corners, cursor))
        .map(|plane| plane.axes)
}

/// Whether a point is inside a convex quad, by consistent turn direction.
fn inside(corners: &[(f32, f32); 4], point: (f32, f32)) -> bool {
    let mut positive = false;
    let mut negative = false;
    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        let cross = (b.0 - a.0) * (point.1 - a.1) - (b.1 - a.1) * (point.0 - a.0);
        if cross > 0.0 {
            positive = true;
        } else if cross < 0.0 {
            negative = true;
        }
    }
    // Inside a convex quad every edge turns the same way.
    !(positive && negative)
}

/// Which way a ring turns for this viewpoint — ADR 0099.
///
/// **A ring seen from behind runs the other way.** The sweep is measured on
/// screen, and screen angle relates to a rotation about a world axis only up to
/// the side you are looking from: orbit past the plane and the same hand motion
/// turns the object the opposite way, which is the classic rotation-gizmo bug
/// and is invisible from the one viewpoint a fixed test uses.
///
/// `+1.0` when the axis points towards the eye, `-1.0` when it points away.
#[must_use]
pub fn ring_sign(view: &View, origin: Vec3, axis: usize) -> f32 {
    let mut direction = Vec3::ZERO;
    direction[axis] = 1.0;
    let towards_object = (origin - view.eye()).normalize_or_zero();
    // A positive dot means the axis points the same way the eye is looking —
    // that is, away from the viewer.
    if towards_object.dot(direction) > 0.0 { -1.0 } else { 1.0 }
}

/// Which ring the cursor is on, if any — ADR 0099.
///
/// Nearest by distance to the polyline, within a few pixels, so the ring that
/// is edge-on does not swallow clicks meant for the one facing you.
#[must_use]
pub fn grab_ring(rings: &[(usize, Vec<(f32, f32)>)], cursor: (f32, f32)) -> Option<usize> {
    const REACH: f32 = 7.0;
    let mut best: Option<(f32, usize)> = None;
    for (axis, points) in rings {
        for point in points {
            let (dx, dy) = (point.0 - cursor.0, point.1 - cursor.1);
            let distance = (dx * dx + dy * dy).sqrt();
            if distance <= REACH && best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, *axis));
            }
        }
    }
    best.map(|(_, axis)| axis)
}

/// The angle of `cursor` about `centre`, in radians — ADR 0099.
///
/// **This is what makes a ring a ring.** Rotation is the angle swept about the
/// gizmo, not a distance dragged along a line: the hand goes round and the
/// thing goes round with it.
#[must_use]
pub fn angle_about(centre: (f32, f32), cursor: (f32, f32)) -> f32 {
    (cursor.1 - centre.1).atan2(cursor.0 - centre.0)
}

/// The shorter way round between two angles, in radians.
///
/// Without this a drag across the wrap point jumps a full turn: atan2 steps
/// from +pi to -pi and the node spins 360 degrees in one frame.
#[must_use]
pub fn shortest_turn(from: f32, to: f32) -> f32 {
    let mut delta = to - from;
    while delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    }
    while delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    delta
}

/// The handle under the cursor, if any — nearest first, so overlapping handles
/// resolve the way they look.
#[must_use]
pub fn grab(handles: &[Handle], cursor: (f32, f32)) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (index, handle) in handles.iter().enumerate() {
        let d = distance_to_segment(cursor, handle.origin, handle.tip);
        if d <= GRAB_PIXELS && best.is_none_or(|(best, _)| d < best) {
            best = Some((d, index));
        }
    }
    best.map(|(_, index)| index)
}

/// Distance from a point to a line segment, in pixels.
fn distance_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let (apx, apy) = (p.0 - a.0, p.1 - a.1);
    let len_sq = abx * abx + aby * aby;
    let t = if len_sq <= f32::EPSILON {
        0.0
    } else {
        ((apx * abx + apy * aby) / len_sq).clamp(0.0, 1.0)
    };
    let (dx, dy) = (apx - abx * t, apy - aby * t);
    (dx * dx + dy * dy).sqrt()
}

/// How far along a handle the cursor has been dragged, in world units.
#[must_use]
pub fn drag_distance(handle: &Handle, from: (f32, f32), to: (f32, f32)) -> f32 {
    let (dx, dy) = (handle.tip.0 - handle.origin.0, handle.tip.1 - handle.origin.1);
    let length = (dx * dx + dy * dy).sqrt();
    if length < f32::EPSILON {
        return 0.0;
    }
    let (ux, uy) = (dx / length, dy / length);
    ((to.0 - from.0) * ux + (to.1 - from.1) * uy) * handle.scale
}

/// Increment snapping for the gizmo — ADR 0093.
///
/// **The result is snapped, not the movement.** Snapping the drag delta gives
/// increments *from wherever the node happened to be*, so a node at x = 0.37
/// steps to 0.87 and never reaches a round number. Snapping the result is what
/// makes a grid a grid: a deck laid out with this has its planks on whole
/// metres, which is the entire reason a human wants it.
///
/// Applied per component, to the number the *file* stores — the local
/// transform, which is what the inspector shows. Snapping in world space would
/// put a node under a turned parent on values that look arbitrary in every
/// place they are displayed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snap {
    /// Metres. Zero or less disables it.
    pub translate: f32,
    /// Degrees.
    pub rotate: f32,
    /// Scale factor.
    pub scale: f32,
    /// Whether it applies at all. Held modifiers invert this rather than set
    /// it, so a human who works on the grid can drop off it for one drag.
    pub enabled: bool,
}

impl Default for Snap {
    fn default() -> Self {
        // A quarter metre and fifteen degrees: fine enough to place a crate,
        // coarse enough that a deck lines up. Both are what a human would
        // otherwise type into the box first.
        Self { translate: 0.25, rotate: 15.0, scale: 0.25, enabled: false }
    }
}

impl Snap {
    /// The increment for a mode, or `None` when snapping is off.
    #[must_use]
    pub fn step(&self, mode: Mode, inverted: bool) -> Option<f32> {
        if self.enabled == inverted {
            return None;
        }
        let step = match mode {
            Mode::Move => self.translate,
            Mode::Rotate => self.rotate,
            Mode::Scale => self.scale,
        };
        (step > 0.0).then_some(step)
    }
}

/// Round `value` to the nearest multiple of `step`.
///
/// A `step` of zero or less is no snapping, because a step of zero would
/// divide by nothing and a negative one is a typo, not an instruction.
#[must_use]
pub fn snap(value: f32, step: f32) -> f32 {
    if step <= 0.0 || !step.is_finite() {
        return value;
    }
    (value / step).round() * step
}

#[cfg(test)]
mod tests {

    /// **A plane quad has to contain its own middle and not the far side of
    /// the screen.** A convex test that got the winding wrong would either
    /// swallow every click in the viewport or none.
    #[test]
    fn a_plane_contains_its_centre_and_nothing_far_away() {
        let handles = super::handles(&view(), Vec3::ZERO);
        let planes = super::planes(&handles);
        assert!(!planes.is_empty(), "at least one plane from three handles");
        for plane in &planes {
            let centre = (
                plane.corners.iter().map(|c| c.0).sum::<f32>() / 4.0,
                plane.corners.iter().map(|c| c.1).sum::<f32>() / 4.0,
            );
            assert_eq!(
                super::grab_plane(std::slice::from_ref(plane), centre),
                Some(plane.axes),
                "a plane must contain its own centre"
            );
            assert_eq!(
                super::grab_plane(std::slice::from_ref(plane), (-9000.0, -9000.0)),
                None
            );
        }
    }

    /// Each plane spans two different axes, and the three cover all three.
    #[test]
    fn the_planes_span_the_three_axis_pairs() {
        let handles = super::handles(&view(), Vec3::ZERO);
        let planes = super::planes(&handles);
        for plane in &planes {
            assert_ne!(plane.axes.0, plane.axes.1, "a plane needs two axes");
        }
        let mut seen: Vec<(usize, usize)> = planes.iter().map(|p| p.axes).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), planes.len(), "no plane is repeated");
    }


    /// **The bug a single viewpoint cannot see.** Cross the rotation plane and
    /// the same drag must still turn the object the way the hand went; without
    /// the sign it counter-rotates, which is the thing every rotation gizmo
    /// gets wrong once.
    #[test]
    fn a_ring_reverses_when_seen_from_the_other_side() {
        let front = View::new(
            &Camera {
                eye: Vec3::new(0.0, 0.0, 10.0),
                target: Vec3::ZERO,
                fov_y_degrees: 60.0,
            },
            800.0,
            600.0,
        );
        let behind = View::new(
            &Camera {
                eye: Vec3::new(0.0, 0.0, -10.0),
                target: Vec3::ZERO,
                fov_y_degrees: 60.0,
            },
            800.0,
            600.0,
        );
        let z = 2;
        let a = super::ring_sign(&front, Vec3::ZERO, z);
        let b = super::ring_sign(&behind, Vec3::ZERO, z);
        assert!((a * b) < 0.0, "the two sides must disagree: {a} and {b}");
        assert!(a.abs() == 1.0 && b.abs() == 1.0, "a sign, not a scale");
    }

    /// An axis edge-on has no honest answer, and must still return a sign
    /// rather than zero — a zero would freeze the drag instead of turning it.
    #[test]
    fn an_edge_on_axis_still_gives_a_sign() {
        let view = view();
        let sign = super::ring_sign(&view, Vec3::ZERO, 0);
        assert!(sign.abs() == 1.0, "{sign}");
    }


    /// **A drag across the wrap point must not spin the node a full turn.**
    /// atan2 steps from +pi to -pi at due west; taking the raw difference there
    /// is a 360-degree jump in one frame, which is the classic rotation-gizmo
    /// bug and is invisible until somebody drags through that exact spot.
    #[test]
    fn a_turn_across_the_wrap_point_is_the_short_way() {
        let just_under = std::f32::consts::PI - 0.05;
        let just_over = -std::f32::consts::PI + 0.05;
        let delta = super::shortest_turn(just_under, just_over);
        assert!(delta.abs() < 0.2, "expected a small step, got {delta}");
        assert!(delta > 0.0, "and in the direction the hand moved: {delta}");
    }

    #[test]
    fn a_turn_the_other_way_is_also_short() {
        let delta = super::shortest_turn(-std::f32::consts::PI + 0.05, std::f32::consts::PI - 0.05);
        assert!(delta.abs() < 0.2, "{delta}");
        assert!(delta < 0.0, "{delta}");
    }

    /// The angle is measured about the gizmo, so a cursor due right of it is
    /// zero and one below it is a quarter turn — screen y grows downward.
    #[test]
    fn the_angle_is_measured_about_the_centre() {
        let centre = (100.0, 100.0);
        assert!(super::angle_about(centre, (200.0, 100.0)).abs() < 1e-6);
        let down = super::angle_about(centre, (100.0, 200.0));
        assert!((down - std::f32::consts::FRAC_PI_2).abs() < 1e-6, "{down}");
    }

    /// **Three rings, and each spans the plane its axis turns in.** A ring that
    /// collapsed to a line would be unclickable and would say nothing about the
    /// plane, which is the whole reason it is a ring.
    #[test]
    fn every_axis_gets_a_ring_that_is_not_a_point() {
        let rings = super::rings(&view(), Vec3::ZERO);
        assert_eq!(rings.len(), 3, "one per axis");
        for (axis, points) in rings {
            // **Either direction, not both.** A ring seen edge-on is a line and
            // that is correct — the test camera looks down -Z, so the ring in
            // the YZ plane genuinely has no width. What must never happen is a
            // ring with no extent at all.
            let extent = |f: fn(&(f32, f32)) -> f32| {
                points.iter().map(f).fold(f32::MIN, f32::max)
                    - points.iter().map(f).fold(f32::MAX, f32::min)
            };
            let spread = extent(|p| p.0).max(extent(|p| p.1));
            assert!(spread > 1.0, "axis {axis} ring collapsed to a point: {spread} px");
        }
    }

    /// The cursor grabs the ring it is on, and nothing when it is in open space.
    #[test]
    fn a_ring_is_grabbed_only_when_the_cursor_is_on_it() {
        let rings = super::rings(&view(), Vec3::ZERO);
        let (axis, points) = &rings[0];
        assert_eq!(super::grab_ring(&rings, points[0]), Some(*axis));
        assert_eq!(super::grab_ring(&rings, (-9000.0, -9000.0)), None);
    }

    use super::*;

    #[test]
    fn snapping_rounds_to_the_nearest_multiple() {
        assert!((snap(1.13, 0.25) - 1.25).abs() < 1e-6);
        assert!((snap(1.12, 0.25) - 1.0).abs() < 1e-6);
        assert!((snap(-1.13, 0.25) + 1.25).abs() < 1e-6);
        assert!((snap(0.0, 0.25) - 0.0).abs() < 1e-6);
    }

    /// **A step of zero must not divide by nothing**, and a negative one is a
    /// typo rather than an instruction to snap backwards.
    #[test]
    fn a_useless_step_leaves_the_value_alone() {
        for step in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let out = snap(3.7, step);
            assert!((out - 3.7).abs() < 1e-6, "step {step} gave {out}");
        }
    }

    /// The result lands on the grid, which is the whole point — a node at an
    /// arbitrary offset must reach round numbers, not merely move in round
    /// increments away from where it started.
    #[test]
    fn snapping_the_result_reaches_round_numbers_from_anywhere() {
        let start = 0.37_f32;
        let dragged = start + 0.6;
        assert!((snap(dragged, 0.25) - 1.0).abs() < 1e-6, "{}", snap(dragged, 0.25));
    }

    /// A held modifier inverts the toggle, so somebody working on the grid can
    /// step off it for one drag without changing a setting.
    #[test]
    fn a_modifier_inverts_the_toggle_rather_than_setting_it() {
        let on = Snap { enabled: true, ..Snap::default() };
        let off = Snap { enabled: false, ..Snap::default() };
        assert_eq!(on.step(Mode::Move, false), Some(0.25));
        assert_eq!(on.step(Mode::Move, true), None, "held modifier drops off the grid");
        assert_eq!(off.step(Mode::Move, false), None);
        assert_eq!(off.step(Mode::Move, true), Some(0.25), "held modifier snaps for one drag");
        assert_eq!(on.step(Mode::Rotate, false), Some(15.0));
    }

    /// A step that was cleared to zero in the box is not snapping.
    #[test]
    fn a_zeroed_step_is_off_even_when_enabled() {
        let snap = Snap { translate: 0.0, enabled: true, ..Snap::default() };
        assert_eq!(snap.step(Mode::Move, false), None);
    }

    fn view() -> View {
        View::new(
            &Camera {
                eye: Vec3::new(0.0, 0.0, 10.0),
                target: Vec3::ZERO,
                fov_y_degrees: 60.0,
            },
            800.0,
            600.0,
        )
    }

    /// The property the whole module rests on: what `project` puts on a pixel,
    /// `ray` picks up from that pixel.
    #[test]
    fn project_and_ray_are_inverses() {
        let view = view();
        let world = Vec3::new(1.5, -0.8, 2.0);

        let (x, y) = view.project(world).expect("in front of the camera");
        let direction = view.ray(x, y);
        let to_point = (world - view.eye()).normalize();

        assert!(
            direction.dot(to_point) > 0.9999,
            "the ray through a projected point must point back at it"
        );
    }

    /// **The inverse has to hold at a non-zero origin too**, which is the
    /// whole reason the origin exists: the editor draws the scene into the
    /// rectangle the panels leave, and every click arrives in window pixels.
    ///
    /// A `project` that forgot to add the origin and a `ray` that forgot to
    /// subtract it are *still inverses of each other* — so this test only
    /// catches the bug because it also checks where the projection lands.
    #[test]
    fn project_and_ray_are_inverses_at_a_non_zero_origin() {
        let camera = Camera {
            eye: Vec3::new(0.0, 0.0, 10.0),
            target: Vec3::ZERO,
            fov_y_degrees: 60.0,
        };
        let origin = (220.0, 130.0);
        let view = View::at(&camera, origin, 800.0, 600.0);
        let world = Vec3::new(1.5, -0.8, 2.0);

        let (x, y) = view.project(world).expect("in front of the camera");
        let direction = view.ray(x, y);
        let to_point = (world - view.eye()).normalize();
        assert!(
            direction.dot(to_point) > 0.9999,
            "the ray through a projected point must point back at it"
        );

        // And the projection is offset by exactly the origin — the half of
        // this that "inverses of each other" cannot see.
        let centred = View::new(&camera, 800.0, 600.0);
        let (cx, cy) = centred.project(world).expect("visible");
        assert!(
            (x - (cx + origin.0)).abs() < 0.01 && (y - (cy + origin.1)).abs() < 0.01,
            "projection did not move with the viewport: {x},{y} against {cx},{cy}"
        );

        // The round trip both ways, which is what the overlay and the picker
        // each rely on in opposite directions.
        let round = view.to_viewport(view.to_window((17.0, 42.0)));
        assert!((round.0 - 17.0).abs() < 1e-6 && (round.1 - 42.0).abs() < 1e-6);
    }

    #[test]
    fn a_point_behind_the_camera_does_not_project() {
        assert!(view().project(Vec3::new(0.0, 0.0, 20.0)).is_none());
    }

    #[test]
    fn the_origin_projects_to_the_centre_of_the_window() {
        let (x, y) = view().project(Vec3::ZERO).expect("visible");
        assert!((x - 400.0).abs() < 0.01 && (y - 300.0).abs() < 0.01);
    }

    /// Dragging the X handle one handle-length must move the node by the
    /// distance that handle stands for — that is what makes the gizmo feel
    /// attached to the object rather than geared to it.
    #[test]
    fn dragging_a_handle_moves_by_what_it_spans() {
        let view = view();
        let handles = handles(&view, Vec3::ZERO);
        let x = handles.iter().find(|h| h.axis == 0).expect("x handle");

        let moved = drag_distance(x, x.origin, x.tip);

        assert!(
            (moved - HANDLE_PIXELS * x.scale).abs() < 1e-4,
            "dragging to the tip moves by the handle's own length in world units"
        );
    }

    #[test]
    fn dragging_across_a_handle_does_nothing() {
        let view = view();
        let handles = handles(&view, Vec3::ZERO);
        let x = handles.iter().find(|h| h.axis == 0).expect("x handle");

        // Straight up the screen, perpendicular to a handle that runs across it.
        let moved = drag_distance(x, (400.0, 300.0), (400.0, 100.0));

        assert!(moved.abs() < 1e-3, "perpendicular drag, no movement");
    }

    #[test]
    fn the_cursor_grabs_the_handle_it_is_on() {
        let view = view();
        let handles = handles(&view, Vec3::ZERO);
        let x = handles.iter().position(|h| h.axis == 0).expect("x handle");
        let on_it = handles[x].tip;

        assert_eq!(grab(&handles, on_it), Some(x));
        assert_eq!(
            grab(&handles, (on_it.0, on_it.1 + 60.0)),
            None,
            "well clear of every handle"
        );
    }
}
