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
    width: f32,
    height: f32,
}

impl View {
    #[must_use]
    pub fn new(camera: &Camera, width: f32, height: f32) -> Self {
        let forward = (camera.target - camera.eye).normalize_or_zero();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        Self {
            eye: camera.eye,
            forward,
            right,
            up: right.cross(forward),
            tan_half_fov: (camera.fov_y_degrees.to_radians() * 0.5).tan(),
            width,
            height,
        }
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
        Some((
            (ndc_x + 1.0) * 0.5 * self.width,
            (1.0 - ndc_y) * 0.5 * self.height,
        ))
    }

    /// A window pixel as a world-space ray direction. The inverse of
    /// [`Self::project`], and the reason picking and the gizmo agree.
    #[must_use]
    pub fn ray(&self, x: f32, y: f32) -> Vec3 {
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

#[cfg(test)]
mod tests {
    use super::*;

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
