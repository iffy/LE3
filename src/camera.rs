//! Orbit camera and 3D→2D projection for the viewport.
//!
//! Until the wgpu/OCCT 3D pipeline lands (SPEC §1, §10), the viewport is drawn
//! by projecting world-space geometry to screen with egui's 2D painter. This
//! module owns the camera state and the project/unproject math.
//!
//! World convention: **Z is up, the ground plane is XY** (z = 0).

use egui::{Pos2, Rect};
use glam::{Mat4, Quat, Vec3, Vec4};

/// Named orthographic-style views for the Z-up ground plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardView {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
}

impl StandardView {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "front" | "f" => Some(Self::Front),
            "back" | "b" => Some(Self::Back),
            "left" | "l" => Some(Self::Left),
            "right" | "r" => Some(Self::Right),
            "top" | "t" => Some(Self::Top),
            "bottom" | "bot" => Some(Self::Bottom),
            _ => None,
        }
    }

    /// Spherical camera parameters that place the eye on this side of `target`.
    pub fn yaw_pitch(self) -> (f32, f32) {
        use std::f32::consts::{FRAC_PI_2, PI};
        match self {
            Self::Front => (-FRAC_PI_2, 0.0),
            Self::Back => (FRAC_PI_2, 0.0),
            Self::Right => (0.0, 0.0),
            Self::Left => (PI, 0.0),
            Self::Top => (0.0, PITCH_LIMIT),
            Self::Bottom => (0.0, -PITCH_LIMIT),
        }
    }
}

/// Default duration for animated view changes (seconds).
pub const VIEW_TRANSITION_DURATION: f32 = 0.35;

/// Startup orbit angles (matches [`Camera::default`]).
pub const ISOMETRIC_YAW: f32 = 0.8;
pub const ISOMETRIC_PITCH: f32 = 0.6;

/// Default look-at target and distance (matches [`Camera::default`]).
pub const HOME_TARGET: Vec3 = Vec3::ZERO;
pub const HOME_DISTANCE: f32 = 400.0;

/// Stored "home" camera pose used by the home-view button.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HomeView {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    /// `None` restores world Z as look-at up when returning home.
    pub view_up: Option<Vec3>,
}

impl Default for HomeView {
    fn default() -> Self {
        Self {
            target: HOME_TARGET,
            yaw: ISOMETRIC_YAW,
            pitch: ISOMETRIC_PITCH,
            distance: HOME_DISTANCE,
            view_up: None,
        }
    }
}

/// Viewport projection: parallel (orthographic) or perspective (natural).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionMode {
    Orthographic,
    Natural,
}

impl ProjectionMode {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "orthographic" | "ortho" => Some(Self::Orthographic),
            "natural" | "perspective" | "persp" => Some(Self::Natural),
            _ => None,
        }
    }

    pub fn opposite(self) -> Self {
        match self {
            Self::Orthographic => Self::Natural,
            Self::Natural => Self::Orthographic,
        }
    }
}

#[derive(Clone, Debug)]
struct ViewTransition {
    from_yaw: f32,
    from_pitch: f32,
    delta_yaw: f32,
    to_pitch: f32,
    from_target: Vec3,
    to_target: Vec3,
    from_distance: f32,
    to_distance: f32,
    from_view_up: Vec3,
    to_view_up: Vec3,
    animate_target: bool,
    animate_distance: bool,
    animate_view_up: bool,
    clear_view_up_on_complete: bool,
    elapsed: f32,
    duration: f32,
}

/// Screen-space padding when framing existing sketch contents in the viewport.
pub const SKETCH_EDIT_FRAME_PADDING_PX: f32 = 15.0;

/// A look-at orbit camera parameterised in spherical coordinates around a
/// `target` point.
#[derive(Clone, Debug)]
pub struct Camera {
    /// Point the camera orbits and looks at, in world space.
    pub target: Vec3,
    /// Azimuth around the up (Z) axis, radians.
    pub yaw: f32,
    /// Elevation above the ground plane, radians. Clamped away from straight
    /// down/up so the look-at `up` vector never degenerates.
    pub pitch: f32,
    /// Distance from `target` to the eye.
    pub distance: f32,
    /// Vertical field of view, radians (used for perspective and ortho framing).
    pub fov_y: f32,
    pub projection: ProjectionMode,
    /// When set, overrides world Z as the look-at up vector (sketch mode).
    view_up: Option<Vec3>,
    /// Accumulated trackball rotation relative to [`orbit_base_offset`].
    orbit_quat: Option<Quat>,
    /// Eye offset when [`orbit_quat`] is identity (captured at first trackball drag).
    orbit_base_offset: Option<Vec3>,
    /// Look-at up when [`orbit_quat`] is identity (preserves roll through poles).
    orbit_base_up: Option<Vec3>,
    /// Camera-right when [`orbit_quat`] is identity (stable pitch axis at poles).
    orbit_base_right: Option<Vec3>,
    home: HomeView,
    transition: Option<ViewTransition>,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            target: HOME_TARGET,
            yaw: ISOMETRIC_YAW,
            pitch: ISOMETRIC_PITCH,
            distance: HOME_DISTANCE,
            fov_y: 45f32.to_radians(),
            projection: ProjectionMode::Natural,
            view_up: None,
            orbit_quat: None,
            orbit_base_offset: None,
            orbit_base_up: None,
            orbit_base_right: None,
            home: HomeView::default(),
            transition: None,
        }
    }
}

fn shortest_yaw_delta(from: f32, to: f32) -> f32 {
    let mut delta = to - from;
    while delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    }
    while delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    delta
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn slerp_direction(from: Vec3, to: Vec3, t: f32) -> Vec3 {
    let from = from.normalize_or_zero();
    let to = to.normalize_or_zero();
    if from.length_squared() < 1e-8 {
        return to;
    }
    if to.length_squared() < 1e-8 {
        return from;
    }
    let dot = from.dot(to).clamp(-1.0, 1.0);
    if dot > 0.9995 {
        return from.lerp(to, t).normalize_or_zero();
    }
    let omega = dot.acos();
    let sin_omega = omega.sin();
    if sin_omega < 1e-6 {
        return from.lerp(to, t).normalize_or_zero();
    }
    let a = ((1.0 - t) * omega).sin() / sin_omega;
    let b = (t * omega).sin() / sin_omega;
    (from * a + to * b).normalize_or_zero()
}

const PITCH_LIMIT: f32 = 1.54; // ~88°, just shy of the singularity at 90°.

impl Camera {
    fn spherical_offset(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.distance * Vec3::new(cp * cy, cp * sy, sp)
    }

    /// Eye position in world space.
    pub fn eye(&self) -> Vec3 {
        if let (Some(q), Some(base)) = (&self.orbit_quat, &self.orbit_base_offset) {
            return self.target + *q * *base;
        }
        self.target + self.spherical_offset()
    }

    /// How head-on the ground plane (XY) is to the view. 1 = plan view, 0 = edge-on.
    #[cfg(test)]
    pub fn ground_plane_head_on(&self) -> f32 {
        (self.target - self.eye()).normalize().z.abs()
    }

    pub fn is_transitioning(&self) -> bool {
        self.transition.is_some()
    }

    pub fn cancel_transition(&mut self) {
        self.transition = None;
    }

    /// Override the look-at up vector (`None` restores world Z).
    pub fn set_view_up(&mut self, up: Option<Vec3>) {
        self.view_up = up.filter(|u| u.length_squared() >= 1e-8);
    }

    pub fn view_up_hint(&self) -> Vec3 {
        if let Some(transition) = &self.transition {
            if transition.animate_view_up {
                let u = smoothstep((transition.elapsed / transition.duration).min(1.0));
                return slerp_direction(transition.from_view_up, transition.to_view_up, u);
            }
        }
        // Active trackball drag must update roll; a fixed sketch `view_up` would
        // fight eye rotation and make orbit feel wrong.
        if let (Some(q), Some(base_up)) = (&self.orbit_quat, &self.orbit_base_up) {
            let up = *q * *base_up;
            if up.length_squared() >= 1e-8 {
                return up.normalize();
            }
        }
        if let Some(up) = self.view_up.filter(|u| u.length_squared() >= 1e-8) {
            return up;
        }
        Self::orbit_look_up(self.target - self.eye())
    }

    /// Leave sketch edit view: bake trackball motion and restore world-Z orbit.
    pub fn leave_sketch_mode(&mut self) {
        self.resolve_orbit_state();
        self.set_view_up(None);
    }

    #[cfg(test)]
    pub fn has_orbit_trackball_state(&self) -> bool {
        self.orbit_quat.is_some()
    }

    #[cfg(test)]
    pub fn has_custom_view_up(&self) -> bool {
        self.view_up.is_some()
    }

    /// World-up for orbit look-at. Flip when the eye is below the target so
    /// `look_at_rh` stays stable through the south pole.
    fn orbit_look_up(forward: Vec3) -> Vec3 {
        let forward = forward.normalize_or_zero();
        if forward.dot(Vec3::Z) > 0.95 {
            -Vec3::Z
        } else {
            Vec3::Z
        }
    }

    fn camera_axes(&self, forward: Vec3) -> (Vec3, Vec3) {
        let forward = forward.normalize_or_zero();
        let up_hint = self.view_up_hint();
        // Match `Mat4::look_at_rh`: right = cross(forward, up_hint), up = cross(right, forward).
        let mut right = forward.cross(up_hint);
        if right.length_squared() < 1e-8 {
            right = forward.cross(Vec3::X);
        }
        if right.length_squared() < 1e-8 {
            right = Vec3::Y;
        }
        right = right.normalize();
        let up = right.cross(forward).normalize();
        (right, up)
    }

    /// Camera screen-right and screen-up in world space (for planar annotation layout).
    pub fn screen_axes(&self, forward: Vec3) -> (Vec3, Vec3) {
        self.camera_axes(forward)
    }

    /// Animate to a standard orthographic view over `duration` seconds.
    pub fn start_view_transition(&mut self, view: StandardView, duration: f32) {
        let (yaw, pitch) = view.yaw_pitch();
        self.start_transition_to_yaw_pitch(yaw, pitch, duration);
    }

    pub fn projection_mode(&self) -> ProjectionMode {
        self.projection
    }

    pub fn set_projection_mode(&mut self, mode: ProjectionMode) {
        self.projection = mode;
    }

    pub fn toggle_projection_mode(&mut self) {
        self.projection = self.projection.opposite();
    }

    /// Half-width/height of the view frustum at the look-at target (world units).
    pub fn viewport_half_extents(&self, aspect: f32) -> (f32, f32) {
        let half_h = self.distance * (self.fov_y * 0.5).tan();
        (half_h * aspect, half_h)
    }

    /// Outward view direction (from `face_point` toward the eye) that keeps the camera on
    /// the side of `face_normal` it already occupies — never flips to the opposite face.
    pub fn visible_face_view_direction(&self, face_point: Vec3, face_normal: Vec3) -> Vec3 {
        let n = face_normal.normalize_or_zero();
        if n.length_squared() < 1e-8 {
            return Vec3::Z;
        }
        let toward_eye = self.eye() - face_point;
        if toward_eye.length_squared() < 1e-8 {
            return n;
        }
        if toward_eye.dot(n) >= 0.0 {
            n
        } else {
            -n
        }
    }

    /// Convert an outward view direction (from `target` toward the eye) to yaw/pitch.
    pub fn view_direction_to_yaw_pitch(direction: Vec3) -> (f32, f32) {
        let dir = direction.normalize_or_zero();
        if dir.length_squared() < 1e-8 {
            return (0.0, 0.0);
        }
        let pitch = dir.z.asin().clamp(-PITCH_LIMIT, PITCH_LIMIT);
        let yaw = if pitch.cos().abs() < 1e-6 {
            0.0
        } else {
            dir.y.atan2(dir.x)
        };
        (yaw, pitch)
    }

    /// Animate to a view that looks from `direction` (outward from `target`).
    pub fn start_view_transition_to_direction(&mut self, direction: Vec3, duration: f32) {
        let (yaw, pitch) = Self::view_direction_to_yaw_pitch(direction);
        self.start_transition_to_yaw_pitch(yaw, pitch, duration);
    }

    pub fn start_transition_to_yaw_pitch(&mut self, to_yaw: f32, to_pitch: f32, duration: f32) {
        self.resolve_orbit_state();
        let to_pitch = to_pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);
        let view_up = self.view_up_hint();
        self.transition = Some(ViewTransition {
            from_yaw: self.yaw,
            from_pitch: self.pitch,
            delta_yaw: shortest_yaw_delta(self.yaw, to_yaw),
            to_pitch,
            from_target: self.target,
            to_target: self.target,
            from_distance: self.distance,
            to_distance: self.distance,
            from_view_up: view_up,
            to_view_up: view_up,
            animate_target: false,
            animate_distance: false,
            animate_view_up: false,
            clear_view_up_on_complete: false,
            elapsed: 0.0,
            duration: duration.max(0.01),
        });
    }

    #[cfg(test)]
    pub fn home_view(&self) -> HomeView {
        self.home
    }

    /// Capture the current camera pose as the home view.
    pub fn set_home_from_current(&mut self) {
        self.resolve_orbit_state();
        self.home = HomeView {
            target: self.target,
            yaw: self.yaw,
            pitch: self.pitch,
            distance: self.distance,
            view_up: self.view_up,
        };
    }

    /// Animate to the stored home camera pose.
    pub fn start_home_transition(&mut self, duration: f32) {
        self.resolve_orbit_state();
        let home = self.home;
        let from_view_up = self.view_up_hint();
        let to_view_up = home.view_up.unwrap_or(Vec3::Z);
        let had_custom_up = self.view_up.is_some();
        let animate_view_up =
            had_custom_up || home.view_up.is_some() || (from_view_up - to_view_up).length() > 1e-3;
        self.transition = Some(ViewTransition {
            from_yaw: self.yaw,
            from_pitch: self.pitch,
            delta_yaw: shortest_yaw_delta(self.yaw, home.yaw),
            to_pitch: home.pitch,
            from_target: self.target,
            to_target: home.target,
            from_distance: self.distance,
            to_distance: home.distance,
            from_view_up,
            to_view_up,
            animate_target: true,
            animate_distance: true,
            animate_view_up,
            clear_view_up_on_complete: home.view_up.is_none() && had_custom_up,
            elapsed: 0.0,
            duration: duration.max(0.01),
        });
    }

    /// Animate to a face-normal view, optionally reframing target and zoom.
    /// `face_normal` is the face's outward normal; the camera stays on the side it
    /// already occupies relative to that face.
    pub fn start_sketch_view_transition(
        &mut self,
        target: Vec3,
        face_normal: Vec3,
        zoom_distance: Option<f32>,
        duration: f32,
        sketch_view_up: Vec3,
    ) {
        self.resolve_orbit_state();
        let view_direction = self.visible_face_view_direction(target, face_normal);
        let current_view = (self.eye() - target).normalize_or_zero();
        let desired_view = view_direction.normalize_or_zero();
        let (yaw, pitch) = if current_view.dot(desired_view) > 0.999 {
            (self.yaw, self.pitch)
        } else {
            Self::view_direction_to_yaw_pitch(view_direction)
        };
        let to_distance = zoom_distance.unwrap_or(self.distance).clamp(2.0, 50_000.0);
        let from_view_up = self.view_up_hint();
        self.transition = Some(ViewTransition {
            from_yaw: self.yaw,
            from_pitch: self.pitch,
            delta_yaw: shortest_yaw_delta(self.yaw, yaw),
            to_pitch: pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT),
            from_target: self.target,
            to_target: target,
            from_distance: self.distance,
            to_distance,
            from_view_up,
            to_view_up: sketch_view_up,
            animate_target: true,
            animate_distance: zoom_distance.is_some(),
            animate_view_up: true,
            clear_view_up_on_complete: false,
            elapsed: 0.0,
            duration: duration.max(0.01),
        });
    }

    /// Camera distance so `corners` around `center` fit in `viewport` when looking along `view_direction`.
    pub fn distance_to_fit_corners(
        &self,
        center: Vec3,
        view_direction: Vec3,
        corners: &[Vec3],
        padding_px: f32,
        viewport: Rect,
    ) -> f32 {
        let dir = view_direction.normalize_or_zero();
        if dir.length_squared() < 1e-8 || corners.is_empty() {
            return self.distance;
        }

        let aspect = (viewport.width() / viewport.height().max(1.0)).max(0.01);
        let forward = -dir;
        let (right, up) = self.camera_axes(forward);

        let mut distance = self.distance.max(10.0);
        for _ in 0..2 {
            let half_h = distance * (self.fov_y * 0.5).tan();
            let pad_world = padding_px * (2.0 * half_h) / viewport.height().max(1.0);

            let mut max_right = pad_world;
            let mut max_up = pad_world;
            for corner in corners {
                let offset = *corner - center;
                max_right = max_right.max(offset.dot(right).abs());
                max_up = max_up.max(offset.dot(up).abs());
            }

            let required_half_h = max_up.max(max_right / aspect);
            distance = (required_half_h / (self.fov_y * 0.5).tan()).clamp(2.0, 50_000.0);
        }
        distance
    }

    /// Advance an in-flight view transition. Returns `true` while animating.
    pub fn tick_transition(&mut self, dt: f32) -> bool {
        let Some(transition) = self.transition.take() else {
            return false;
        };
        let mut t = transition;
        t.elapsed += dt;
        let u = smoothstep((t.elapsed / t.duration).min(1.0));
        self.yaw = t.from_yaw + t.delta_yaw * u;
        self.pitch = t.from_pitch + (t.to_pitch - t.from_pitch) * u;
        if t.animate_target {
            self.target = t.from_target.lerp(t.to_target, u);
        }
        if t.animate_distance {
            self.distance = t.from_distance + (t.to_distance - t.from_distance) * u;
        }
        if t.elapsed < t.duration {
            self.transition = Some(t);
            true
        } else {
            self.yaw = t.from_yaw + t.delta_yaw;
            self.pitch = t.to_pitch;
            if t.animate_target {
                self.target = t.to_target;
            }
            if t.animate_distance {
                self.distance = t.to_distance;
            }
            if t.animate_view_up {
                if t.clear_view_up_on_complete {
                    self.view_up = None;
                } else {
                    self.view_up = Some(t.to_view_up);
                }
            }
            false
        }
    }

    const ORBIT_SENSITIVITY: f32 = 0.01;

    /// Orbit by a screen-space drag delta (in points).
    pub fn orbit(&mut self, delta: egui::Vec2) {
        self.orbit_trackball(delta);
    }

    fn capture_orbit_base_right(
        forward: Vec3,
        up: Vec3,
        yaw: f32,
        sketch_pitch_at_pole: bool,
    ) -> Vec3 {
        let forward = forward.normalize_or_zero();
        // World top/bottom: `forward × world_up` is non-zero but is the wrong pitch
        // axis near the pole; keep the yaw-aligned fallback. Sketch views pass a
        // plane-axis `up` that matches screen-right even at the pole.
        if !sketch_pitch_at_pole && forward.z.abs() > 0.95 {
            return Vec3::new(yaw.cos(), yaw.sin(), 0.0).normalize_or_zero();
        }
        let mut right = forward.cross(up);
        if right.length_squared() < 1e-8 {
            if forward.z.abs() > 0.95 {
                return Vec3::new(yaw.cos(), yaw.sin(), 0.0).normalize_or_zero();
            }
            right = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
        }
        if right.length_squared() < 1e-8 {
            right = Vec3::X;
        }
        right.normalize()
    }

    /// Up hint for the pitch (vertical-drag) axis. Prefer sketch plane-axis up when
    /// it yields a stable screen-right at the pole; otherwise fall back to world up.
    fn orbit_pitch_up_hint(&self, forward: Vec3) -> Vec3 {
        if let Some(sketch_up) = self.view_up.filter(|u| u.length_squared() >= 1e-8) {
            let sketch_up = sketch_up.normalize();
            if forward.cross(sketch_up).length_squared() >= 1e-8 {
                return sketch_up;
            }
        }
        if let (Some(_), Some(base_up)) = (&self.orbit_quat, &self.orbit_base_up) {
            return *base_up;
        }
        self.view_up_hint()
    }

    fn ensure_orbit_trackball_state(&mut self) {
        if self.orbit_quat.is_none() {
            let forward = (self.target - self.eye()).normalize_or_zero();
            let up = self.view_up_hint();
            let sketch_pitch = self.view_up.is_some();
            let pitch_up = self.orbit_pitch_up_hint(forward);
            let right =
                Self::capture_orbit_base_right(forward, pitch_up, self.yaw, sketch_pitch);
            self.orbit_base_offset = Some(self.spherical_offset());
            self.orbit_base_up = Some(up);
            self.orbit_base_right = Some(right);
            self.orbit_quat = Some(Quat::IDENTITY);
        }
    }

    fn trackball_right_axis(&self) -> Vec3 {
        if let (Some(q), Some(base_right)) = (&self.orbit_quat, &self.orbit_base_right) {
            let right = *q * *base_right;
            if right.length_squared() >= 1e-8 {
                return right.normalize();
            }
        }
        let forward = (self.target - self.eye()).normalize_or_zero();
        let sketch_pitch = self.view_up.is_some();
        let pitch_up = self.orbit_pitch_up_hint(forward);
        Self::capture_orbit_base_right(forward, pitch_up, self.yaw, sketch_pitch)
    }

    /// Trackball-style orbit: rotates the eye around `target` using camera-local
    /// axes so dragging still works at the poles (e.g. top view).
    pub fn orbit_trackball(&mut self, delta: egui::Vec2) {
        self.cancel_transition();
        self.ensure_orbit_trackball_state();
        let sens = Self::ORBIT_SENSITIVITY;
        let mut delta_yaw = Quat::IDENTITY;
        let mut delta_pitch = Quat::IDENTITY;

        if delta.x.abs() > f32::EPSILON {
            delta_yaw = Quat::from_axis_angle(Vec3::Z, -delta.x * sens);
        }

        if delta.y.abs() > f32::EPSILON {
            let right = self.trackball_right_axis();
            delta_pitch = Quat::from_axis_angle(right, -delta.y * sens);
        }

        let quat = self.orbit_quat.as_mut().expect("orbit trackball state");
        *quat = delta_pitch * delta_yaw * *quat;
        self.apply_offset_yaw_pitch(self.eye() - self.target);
    }

    fn resolve_orbit_state(&mut self) {
        if self.orbit_quat.is_some() {
            self.apply_offset_yaw_pitch(self.eye() - self.target);
        }
        self.orbit_quat = None;
        self.orbit_base_offset = None;
        self.orbit_base_up = None;
        self.orbit_base_right = None;
    }

    fn apply_offset_yaw_pitch(&mut self, offset: Vec3) {
        let len = offset.length();
        if len < 1e-6 {
            return;
        }
        self.distance = len;
        let dir = offset / len;
        let pitch = dir.z.asin();
        self.pitch = pitch;
        let horizontal_len_sq = offset.x * offset.x + offset.y * offset.y;
        if pitch.cos().abs() < 1e-4 || horizontal_len_sq < 1e-6 {
            // Keep yaw through the pole; atan2 branches differ by π and reverse orbit.
        } else {
            let new_yaw = offset.y.atan2(offset.x);
            self.yaw += shortest_yaw_delta(self.yaw, new_yaw);
        }
    }

    /// Pan: slide the look-at `target` (and therefore the eye) in the camera's
    /// view plane by a screen-space drag delta. Scaled so the point under the
    /// cursor tracks it regardless of zoom level.
    pub fn pan(&mut self, delta: egui::Vec2, viewport_height: f32) {
        let forward = (self.target - self.eye()).normalize();
        let (right, up) = self.camera_axes(forward);
        let half_h = self.viewport_half_extents(1.0).1;
        let world_per_px = 2.0 * half_h / viewport_height.max(1.0);
        self.target += (-right * delta.x + up * delta.y) * world_per_px;
    }

    /// Dolly in/out from a scroll amount (positive = zoom in), keeping the point
    /// under `focal_screen` fixed on screen when possible.
    pub fn zoom(&mut self, scroll: f32, focal_screen: Pos2, viewport: Rect) {
        let old_distance = self.distance;
        let new_distance =
            (old_distance * (1.0 - scroll * 0.001)).clamp(2.0, 50_000.0);
        if (new_distance - old_distance).abs() < f32::EPSILON {
            return;
        }

        let vp = self.view_proj(viewport);
        if let Some(pivot) = self.view_plane_point(focal_screen, viewport, &vp) {
            let ratio = new_distance / old_distance;
            self.target += (pivot - self.target) * (1.0 - ratio);
        }

        self.distance = new_distance;
        if let Some(base) = &mut self.orbit_base_offset {
            *base = base.normalize_or_zero() * new_distance;
        }
    }

    /// World point on the view plane (through `target`, facing the camera) under
    /// `focal_screen`.
    fn view_plane_point(&self, focal_screen: Pos2, viewport: Rect, vp: &Mat4) -> Option<Vec3> {
        let inv = vp.inverse();
        let ndc_x = ((focal_screen.x - viewport.min.x) / viewport.width()) * 2.0 - 1.0;
        let ndc_y = (1.0 - (focal_screen.y - viewport.min.y) / viewport.height()) * 2.0 - 1.0;

        let near = inv * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        let far = inv * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
        let near_w = near.truncate() / near.w;
        let far_w = far.truncate() / far.w;
        let ray_dir = far_w - near_w;
        if ray_dir.length_squared() < 1e-12 {
            return None;
        }
        let ray_dir = ray_dir.normalize();
        let eye = self.eye();
        let forward = (self.target - eye).normalize();
        let denom = ray_dir.dot(forward);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = (self.target - eye).dot(forward) / denom;
        if t < 0.0 {
            return None;
        }
        Some(eye + ray_dir * t)
    }

    /// Combined view-projection matrix for the given viewport rectangle.
    pub fn view_proj(&self, viewport: Rect) -> Mat4 {
        let aspect = (viewport.width() / viewport.height().max(1.0)).max(0.01);
        let (half_w, half_h) = self.viewport_half_extents(aspect);
        let proj = match self.projection {
            ProjectionMode::Natural => Mat4::perspective_rh(self.fov_y, aspect, 0.1, 100_000.0),
            ProjectionMode::Orthographic => {
                Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, 0.1, 100_000.0)
            }
        };
        let view = Mat4::look_at_rh(self.eye(), self.target, self.view_up_hint());
        proj * view
    }

    /// Project a world point to a screen position, or `None` if it is behind
    /// the camera.
    pub fn project(&self, world: Vec3, viewport: Rect, vp: &Mat4) -> Option<Pos2> {
        let clip = *vp * world.extend(1.0);
        if clip.w <= 1e-4 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        let x = viewport.min.x + (ndc.x * 0.5 + 0.5) * viewport.width();
        let y = viewport.min.y + (1.0 - (ndc.y * 0.5 + 0.5)) * viewport.height();
        Some(Pos2::new(x, y))
    }

    /// Cast a ray from the screen pixel onto an arbitrary plane and return the hit.
    pub fn ray_plane_hit(
        &self,
        screen: Pos2,
        viewport: Rect,
        vp: &Mat4,
        plane_origin: Vec3,
        plane_normal: Vec3,
    ) -> Option<Vec3> {
        let inv = vp.inverse();
        let ndc_x = ((screen.x - viewport.min.x) / viewport.width()) * 2.0 - 1.0;
        let ndc_y = (1.0 - (screen.y - viewport.min.y) / viewport.height()) * 2.0 - 1.0;

        let near = inv * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        let far = inv * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
        let near_w = near.truncate() / near.w;
        let far_w = far.truncate() / far.w;
        let ray_dir = far_w - near_w;
        if ray_dir.length_squared() < 1e-12 {
            return None;
        }
        let ray_dir = ray_dir.normalize();
        let n = plane_normal.normalize_or_zero();
        let denom = ray_dir.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = (plane_origin - near_w).dot(n) / denom;
        if t < 0.0 {
            return None;
        }
        Some(near_w + ray_dir * t)
    }

    /// Cast a ray from the screen pixel onto the ground plane (z = 0) and return
    /// the hit point, or `None` if the ray misses (points at/above the horizon).
    pub fn ground_point(&self, screen: Pos2, viewport: Rect, vp: &Mat4) -> Option<Vec3> {
        let inv = vp.inverse();
        let ndc_x = ((screen.x - viewport.min.x) / viewport.width()) * 2.0 - 1.0;
        let ndc_y = (1.0 - (screen.y - viewport.min.y) / viewport.height()) * 2.0 - 1.0;

        let near = inv * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        let far = inv * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
        let near = near.truncate() / near.w;
        let far = far.truncate() / far.w;

        let dir = far - near;
        if dir.z.abs() < 1e-6 {
            return None; // Ray parallel to the ground.
        }
        let t = -near.z / dir.z;
        if t < 0.0 {
            return None; // Ground is behind the camera.
        }
        Some(near + dir * t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_viewport() -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 80.0), egui::vec2(800.0, 600.0))
    }

    /// How much `axis` aligns with camera screen-right (+ = points right).
    fn axis_along_screen_right(cam: &Camera, axis: Vec3) -> f32 {
        let forward = (cam.target - cam.eye()).normalize_or_zero();
        let up = cam.view_up_hint();
        let right = forward.cross(up).normalize_or_zero();
        axis.normalize_or_zero().dot(right)
    }

    /// How much `axis` aligns with camera screen-up (+ = points up).
    fn axis_along_screen_up(cam: &Camera, axis: Vec3) -> f32 {
        let forward = (cam.target - cam.eye()).normalize_or_zero();
        let up_hint = cam.view_up_hint();
        let mut right = forward.cross(up_hint);
        if right.length_squared() < 1e-8 {
            right = forward.cross(Vec3::X);
        }
        if right.length_squared() < 1e-8 {
            right = Vec3::Y;
        }
        let screen_up = right.cross(forward).normalize_or_zero();
        axis.normalize_or_zero().dot(screen_up)
    }

    fn cam_looking_from_positive_x() -> Camera {
        let mut cam = Camera::default();
        cam.yaw = 0.0;
        cam.pitch = 0.0;
        cam
    }

    /// Egui pointer delta for a screen-space drag upward (y decreases).
    fn screen_drag_up(cam: &mut Camera, amount: f32) {
        cam.orbit_trackball(egui::vec2(0.0, -amount));
    }

    #[test]
    fn top_view_large_drag_tilts_toward_back() {
        let (yaw, pitch) = StandardView::Top.yaw_pitch();
        let mut cam = Camera::default();
        cam.yaw = yaw;
        cam.pitch = pitch;
        cam.orbit_trackball(egui::vec2(0.0, 90.0));
        let offset = (cam.eye() - cam.target).normalize();
        assert!(
            offset.y > 0.4,
            "dragging top view down should move the eye toward +Y/back, offset={offset:?}"
        );
    }

    #[test]
    fn looking_down_positive_x_has_y_pointing_right() {
        let cam = cam_looking_from_positive_x();
        assert!(
            axis_along_screen_right(&cam, Vec3::Y) > 0.9,
            "Y should point right when looking down +X"
        );
        assert!(
            axis_along_screen_up(&cam, Vec3::Z) > 0.9,
            "Z should point up when looking down +X"
        );
    }

    /// Screen drag-up from the +X equator pitches toward the south pole (-Z).
    fn drag_orbit_toward_south_pole(cam: &mut Camera, amount: f32) {
        screen_drag_up(cam, amount);
    }

    #[test]
    fn orbit_past_south_pole_keeps_y_pointing_right_from_x_view() {
        let mut cam = cam_looking_from_positive_x();
        // π/2 rad of pitch at 0.06 rad/step ≈ 26 steps to the south-pole meridian.
        let steps_to_z_away = (std::f32::consts::FRAC_PI_2 / 0.06).round() as usize;
        for _ in 0..steps_to_z_away {
            drag_orbit_toward_south_pole(&mut cam, 6.0);
        }
        // First pole crossing: +Z away, +X up on screen, +Y right.
        assert!(
            axis_along_screen_right(&cam, Vec3::Y) > 0.8,
            "Y should still point right after crossing the south pole"
        );
        assert!(
            axis_along_screen_up(&cam, Vec3::X) > 0.8,
            "+X should point up after crossing the south pole"
        );
        let forward = (cam.target - cam.eye()).normalize_or_zero();
        assert!(
            forward.dot(Vec3::Z) > 0.8,
            "+Z should point away after crossing the south pole"
        );

        // Keep orbiting through the next quadrant — Y must not flip left.
        let extra_steps = (std::f32::consts::FRAC_PI_2 / 0.06).round() as usize;
        for _ in 0..extra_steps {
            drag_orbit_toward_south_pole(&mut cam, 6.0);
        }
        assert!(
            axis_along_screen_right(&cam, Vec3::Y) > 0.8,
            "Y should remain on the right after continuing past the south pole"
        );
    }

    #[test]
    fn continuous_orbit_past_south_pole_twice_preserves_y_on_screen_right() {
        let mut cam = cam_looking_from_positive_x();
        let steps_per_half_turn = (std::f32::consts::PI / 0.06).round() as usize;
        let total_steps = steps_per_half_turn * 4 + 4;
        for step in 0..total_steps {
            drag_orbit_toward_south_pole(&mut cam, 6.0);
            if step % 8 == 0 {
                assert!(
                    axis_along_screen_right(&cam, Vec3::Y) > 0.7,
                    "Y should stay on the right through continuous pole orbits (step {step})"
                );
            }
        }
    }

    #[test]
    fn orbit_with_any_axis_on_right_can_stay_right_through_pole() {
        let mut cam = Camera::default();
        cam.yaw = std::f32::consts::FRAC_PI_2;
        cam.pitch = 0.0;
        let screen_right_axis = -Vec3::X;
        assert!(
            axis_along_screen_right(&cam, screen_right_axis) > 0.9,
            "precondition: -X should start on screen right from +Y view"
        );
        let steps = (std::f32::consts::PI / 0.06).round() as usize + 2;
        for step in 0..steps {
            screen_drag_up(&mut cam, 6.0);
            if step % 6 == 0 {
                assert!(
                    axis_along_screen_right(&cam, screen_right_axis) > 0.7,
                    "the axis on screen right should stay right through the south pole (step {step})"
                );
            }
        }
    }

    #[test]
    fn zoom_at_cursor_preserves_screen_position() {
        let mut cam = Camera::default();
        let viewport = test_viewport();
        let focal = Pos2::new(520.0, 380.0);
        let vp = cam.view_proj(viewport);
        let pivot = cam
            .view_plane_point(focal, viewport, &vp)
            .expect("cursor ray should hit the view plane");
        let screen_before = cam.project(pivot, viewport, &vp).expect("pivot should be visible");

        cam.zoom(120.0, focal, viewport);

        let vp2 = cam.view_proj(viewport);
        let screen_after = cam.project(pivot, viewport, &vp2).expect("pivot should stay visible");
        assert!(
            (screen_before - screen_after).length() < 0.5,
            "pivot should stay under the cursor: before={screen_before:?} after={screen_after:?}"
        );
    }

    #[test]
    fn standard_view_orientations() {
        let mut cam = Camera::default();
        for (view, expected) in [
            (StandardView::Right, Vec3::new(1.0, 0.0, 0.0)),
            (StandardView::Front, Vec3::new(0.0, -1.0, 0.0)),
            (StandardView::Back, Vec3::new(0.0, 1.0, 0.0)),
            (StandardView::Left, Vec3::new(-1.0, 0.0, 0.0)),
        ] {
            let (yaw, pitch) = view.yaw_pitch();
            cam.yaw = yaw;
            cam.pitch = pitch;
            let offset = (cam.eye() - cam.target).normalize();
            assert!(
                (offset - expected).length() < 0.05,
                "{view:?}: expected {expected:?}, got {offset:?}"
            );
        }
        cam.yaw = 0.0;
        cam.pitch = PITCH_LIMIT;
        let top = (cam.eye() - cam.target).normalize();
        assert!(top.z > 0.95, "top view should look from +Z, got {top:?}");
        cam.pitch = -PITCH_LIMIT;
        let bottom = (cam.eye() - cam.target).normalize();
        assert!(bottom.z < -0.95, "bottom view should look from -Z, got {bottom:?}");
    }

    #[test]
    fn view_transition_reaches_target() {
        let mut cam = Camera::default();
        cam.start_view_transition(StandardView::Right, 0.5);
        assert!(cam.is_transitioning());
        while cam.tick_transition(0.05) {}
        let (yaw, pitch) = StandardView::Right.yaw_pitch();
        assert!((cam.yaw - yaw).abs() < 0.01);
        assert!((cam.pitch - pitch).abs() < 0.01);
        assert!(!cam.is_transitioning());
    }

    #[test]
    fn orbit_cancels_view_transition() {
        let mut cam = Camera::default();
        cam.start_view_transition(StandardView::Top, 0.5);
        cam.orbit(egui::vec2(4.0, 2.0));
        assert!(!cam.is_transitioning());
    }

    #[test]
    fn trackball_from_top_drag_down_tilts_toward_back() {
        let (yaw, pitch) = StandardView::Top.yaw_pitch();
        let mut cam = Camera::default();
        cam.yaw = yaw;
        cam.pitch = pitch;
        let before = cam.eye() - cam.target;

        // Pulling down on the cube tilts the top face away (positive screen Y).
        cam.orbit_trackball(egui::vec2(0.0, 30.0));

        let after_offset = cam.eye() - cam.target;
        assert!(
            cam.pitch < pitch - 0.05,
            "pitch should decrease from top view, got {}",
            cam.pitch
        );
        assert!(
            before.z - after_offset.z > 5.0,
            "eye should descend from the top pole, before={before:?} after={after_offset:?}"
        );
        assert!(
            after_offset.x * after_offset.x + after_offset.y * after_offset.y > 25.0,
            "top-pole orbit should gain horizontal offset, after={after_offset:?}"
        );
    }

    #[test]
    fn trackball_horizontal_drag_changes_yaw_off_pole() {
        let mut cam = Camera::default();
        let yaw_before = cam.yaw;
        cam.orbit_trackball(egui::vec2(25.0, 0.0));
        assert!(
            (cam.yaw - yaw_before).abs() > 0.05,
            "horizontal drag should change yaw away from the poles"
        );
    }

    #[test]
    fn ground_plane_head_on_is_zero_when_viewing_edge_on() {
        let mut cam = Camera::default();
        let (yaw, pitch) = StandardView::Front.yaw_pitch();
        cam.yaw = yaw;
        cam.pitch = pitch;
        assert!(cam.ground_plane_head_on() < 0.05);
    }

    #[test]
    fn ground_plane_head_on_is_one_from_top_view() {
        let mut cam = Camera::default();
        let (yaw, pitch) = StandardView::Top.yaw_pitch();
        cam.yaw = yaw;
        cam.pitch = pitch;
        assert!(cam.ground_plane_head_on() > 0.95);
    }

    #[test]
    fn default_projection_is_natural() {
        assert_eq!(Camera::default().projection_mode(), ProjectionMode::Natural);
    }

    #[test]
    fn toggle_projection_mode_swaps() {
        let mut cam = Camera::default();
        cam.toggle_projection_mode();
        assert_eq!(cam.projection_mode(), ProjectionMode::Orthographic);
        cam.toggle_projection_mode();
        assert_eq!(cam.projection_mode(), ProjectionMode::Natural);
    }

    #[test]
    fn orthographic_projection_preserves_parallel_xy_spacing() {
        let mut cam = Camera::default();
        cam.set_projection_mode(ProjectionMode::Orthographic);
        let viewport = test_viewport();
        let vp = cam.view_proj(viewport);
        let a0 = cam.project(Vec3::new(0.0, 0.0, 0.0), viewport, &vp).unwrap();
        let a1 = cam.project(Vec3::new(100.0, 0.0, 0.0), viewport, &vp).unwrap();
        let b0 = cam.project(Vec3::new(0.0, 0.0, 80.0), viewport, &vp).unwrap();
        let b1 = cam.project(Vec3::new(100.0, 0.0, 80.0), viewport, &vp).unwrap();
        let dx_near = a1.x - a0.x;
        let dx_far = b1.x - b0.x;
        assert!(
            (dx_near - dx_far).abs() < 0.5,
            "ortho spacing should not shrink with depth: near={dx_near} far={dx_far}"
        );
    }

    #[test]
    fn view_direction_to_yaw_pitch_front_top_edge() {
        let dir = Vec3::new(0.0, -1.0, 1.0).normalize();
        let (yaw, pitch) = Camera::view_direction_to_yaw_pitch(dir);
        assert!(pitch > 0.2);
        // Between front (-π/2) and top (0): x=0, y<0 ⇒ yaw = -π/2.
        assert!((yaw - (-std::f32::consts::FRAC_PI_2)).abs() < 0.01);
    }

    #[test]
    fn trackball_cancels_view_transition() {
        let mut cam = Camera::default();
        cam.start_view_transition(StandardView::Top, 0.5);
        cam.orbit_trackball(egui::vec2(4.0, 2.0));
        assert!(!cam.is_transitioning());
    }

    #[test]
    fn trackball_past_bottom_continues_orbit() {
        let mut cam = Camera::default();
        cam.pitch = -std::f32::consts::FRAC_PI_2;
        cam.yaw = 0.4;
        let z_start = cam.eye().z;

        // π rad of pitch drag (6 px × 0.01 sens) is needed to traverse the south pole.
        let steps = (std::f32::consts::PI / 0.06).ceil() as usize + 2;
        for _ in 0..steps {
            cam.orbit_trackball(egui::vec2(0.0, 6.0));
        }

        let offset = cam.eye() - cam.target;
        assert!(
            offset.z > 0.0,
            "dragging past the bottom should cross the pole, offset={offset:?}"
        );
        assert!(
            cam.eye().z > z_start + cam.distance * 0.5,
            "eye should rise well above the start, z={} start={z_start}",
            cam.eye().z
        );
        let viewport = test_viewport();
        let vp = cam.view_proj(viewport);
        assert!(
            cam.project(Vec3::ZERO, viewport, &vp).is_some(),
            "view should stay projectable after crossing the bottom pole"
        );
    }

    #[test]
    fn orbit_look_up_flips_when_viewing_from_below() {
        let mut cam = Camera::default();
        cam.pitch = -PITCH_LIMIT;
        assert!(
            cam.view_up_hint().dot(Vec3::Z) < -0.9,
            "look-at up should flip below the target"
        );
    }

    #[test]
    fn zoom_at_cursor_moves_target_toward_pivot_when_zooming_in() {
        let mut cam = Camera::default();
        let viewport = test_viewport();
        let focal = Pos2::new(520.0, 380.0);
        let vp = cam.view_proj(viewport);
        let pivot = cam
            .view_plane_point(focal, viewport, &vp)
            .expect("cursor ray should hit the view plane");
        let target_before = cam.target;

        cam.zoom(200.0, focal, viewport);

        assert!(cam.distance < 400.0);
        let toward = (pivot - target_before).normalize_or_zero();
        let motion = cam.target - target_before;
        assert!(
            motion.dot(toward) > 0.0,
            "target should move toward the point under the cursor"
        );
    }

    #[test]
    fn visible_face_view_direction_stays_on_current_side() {
        let mut cam = Camera::default();
        cam.target = Vec3::ZERO;
        cam.distance = 400.0;
        cam.yaw = 0.0;
        cam.pitch = 1.2;
        let from_above = cam.visible_face_view_direction(Vec3::ZERO, Vec3::Z);
        assert!(from_above.z > 0.0, "camera above should keep +Z, got {from_above:?}");

        cam.pitch = -1.2;
        let from_below = cam.visible_face_view_direction(Vec3::ZERO, Vec3::Z);
        assert!(from_below.z < 0.0, "camera below should keep -Z, got {from_below:?}");
    }

    #[test]
    fn sketch_orbit_uses_trackball_roll_while_dragging() {
        let mut cam = Camera::default();
        cam.set_view_up(Some(Vec3::Y));
        let up_before_drag = cam.view_up_hint();
        cam.orbit_trackball(egui::vec2(12.0, 0.0));
        let up_after_drag = cam.view_up_hint();
        assert!(
            (up_before_drag - up_after_drag).length() > 1e-4,
            "sketch orbit should rotate roll with the trackball"
        );
    }

    #[test]
    fn leave_sketch_mode_clears_view_up_and_trackball() {
        let mut cam = Camera::default();
        cam.set_view_up(Some(Vec3::Y));
        cam.orbit_trackball(egui::vec2(8.0, 4.0));
        assert!(cam.has_orbit_trackball_state());
        cam.leave_sketch_mode();
        assert!(cam.view_up.is_none());
        assert!(!cam.has_orbit_trackball_state());
    }

    #[test]
    fn set_home_from_current_updates_home_view() {
        let mut cam = Camera::default();
        cam.target = Vec3::new(12.0, -8.0, 3.0);
        cam.yaw = 1.2;
        cam.pitch = -0.35;
        cam.distance = 220.0;
        cam.set_home_from_current();
        let home = cam.home_view();
        assert!((home.target - cam.target).length() < 1e-4);
        assert!((home.yaw - cam.yaw).abs() < 1e-4);
        assert!((home.pitch - cam.pitch).abs() < 1e-4);
        assert!((home.distance - cam.distance).abs() < 1e-4);
    }

    #[test]
    fn home_transition_reaches_custom_home_view() {
        let mut cam = Camera::default();
        cam.target = Vec3::new(30.0, 10.0, 5.0);
        cam.yaw = 1.7;
        cam.pitch = 0.25;
        cam.distance = 150.0;
        cam.set_home_from_current();

        cam.target = Vec3::ZERO;
        cam.yaw = 0.0;
        cam.pitch = 0.0;
        cam.distance = 400.0;
        cam.start_home_transition(0.5);
        while cam.tick_transition(0.05) {}

        let home = cam.home_view();
        assert!((cam.target - home.target).length() < 0.01);
        assert!((cam.yaw - home.yaw).abs() < 0.01);
        assert!((cam.pitch - home.pitch).abs() < 0.01);
        assert!((cam.distance - home.distance).abs() < 0.5);
    }

    #[test]
    fn home_transition_reaches_startup_pose() {
        let mut cam = Camera::default();
        cam.target = Vec3::new(40.0, -20.0, 10.0);
        cam.yaw = 2.1;
        cam.pitch = -0.4;
        cam.distance = 120.0;
        cam.set_view_up(Some(Vec3::Y));
        cam.start_home_transition(0.5);
        while cam.tick_transition(0.05) {}
        assert!((cam.target - HOME_TARGET).length() < 0.01);
        assert!((cam.yaw - ISOMETRIC_YAW).abs() < 0.01);
        assert!((cam.pitch - ISOMETRIC_PITCH).abs() < 0.01);
        assert!((cam.distance - HOME_DISTANCE).abs() < 0.5);
        assert!(cam.view_up_hint().dot(Vec3::Z).abs() > 0.99);
    }

    #[test]
    fn sketch_view_transition_animates_target_and_distance() {
        let mut cam = Camera::default();
        cam.start_sketch_view_transition(
            Vec3::new(10.0, 20.0, 0.0),
            Vec3::Z,
            Some(120.0),
            0.5,
            Vec3::Y,
        );
        while cam.tick_transition(0.05) {}
        assert!((cam.target.x - 10.0).abs() < 0.01);
        assert!((cam.target.y - 20.0).abs() < 0.01);
        assert!((cam.distance - 120.0).abs() < 0.5);
        let view = (cam.eye() - cam.target).normalize();
        assert!(view.z > 0.95, "should look along +Z normal, got {view:?}");
    }

    #[test]
    fn distance_to_fit_corners_scales_with_bounds() {
        let cam = Camera::default();
        let viewport = test_viewport();
        let center = Vec3::ZERO;
        let small = [
            Vec3::new(-10.0, -10.0, 0.0),
            Vec3::new(10.0, -10.0, 0.0),
            Vec3::new(10.0, 10.0, 0.0),
            Vec3::new(-10.0, 10.0, 0.0),
        ];
        let large = [
            Vec3::new(-100.0, -100.0, 0.0),
            Vec3::new(100.0, -100.0, 0.0),
            Vec3::new(100.0, 100.0, 0.0),
            Vec3::new(-100.0, 100.0, 0.0),
        ];
        let near = cam.distance_to_fit_corners(center, Vec3::Z, &small, 8.0, viewport);
        let far = cam.distance_to_fit_corners(center, Vec3::Z, &large, 8.0, viewport);
        assert!(far > near * 5.0);
    }

    #[test]
    fn sketch_edit_padding_is_in_requested_range() {
        assert!(
            (10.0..=20.0).contains(&SKETCH_EDIT_FRAME_PADDING_PX),
            "padding should leave 10-20px margin around framed sketch contents"
        );
    }

    /// Sketch entry from isometric with v (Y) on screen-right: vertical drag should
    /// tilt the view, not slide the Y axis horizontally across the screen.
    #[test]
    fn sketch_orbit_vertical_drag_with_y_on_screen_right() {
        use crate::actions::{Action, AppState};
        use crate::model::FaceId;

        let viewport = test_viewport();
        let mut state = AppState::default();
        state.apply(Action::BeginSketch {
            face: FaceId::ConstructionPlane(0),
            viewport: None,
        });
        while state.cam.tick_transition(0.05) {}

        assert!(
            axis_along_screen_right(&state.cam, Vec3::Y) > 0.7,
            "precondition: Y should be on screen right in sketch"
        );
        assert!(state.cam.has_custom_view_up());

        let probe = Vec3::new(50.0, 0.0, 30.0);
        let vp = state.cam.view_proj(viewport);
        let before = state
            .cam
            .project(probe, viewport, &vp)
            .expect("probe visible");
        state.cam.orbit_trackball(egui::vec2(0.0, -30.0));
        let vp = state.cam.view_proj(viewport);
        let after = state
            .cam
            .project(probe, viewport, &vp)
            .expect("probe visible after drag");

        let delta = after - before;
        assert!(
            delta.y.abs() >= delta.x.abs(),
            "vertical drag should tilt the view (screen Y), not pan sideways (screen X); \
             delta={delta:?}, before={before:?}, after={after:?}"
        );
        assert!(
            axis_along_screen_right(&state.cam, Vec3::Y) > 0.7,
            "Y should stay on screen right after vertical orbit"
        );
    }

}
