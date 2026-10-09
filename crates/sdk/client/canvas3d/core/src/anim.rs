//! Animation: a model's node hierarchy ([`Node`], [`Transform`]), skeletal
//! skins ([`Skin`], [`SkinWeights`]), clips ([`Animation`]) and the [`Pose`]
//! a clip is sampled into.
//!
//! A pose is plain data: the local transform of every node of one model.
//! Sampling a clip, blending two poses, or setting a node by hand all produce
//! a pose, and drawing a model with `s.model(&m, xf).pose(pose)` shows it.
//! Time is the author's input (usually an [`AnimationClock`](crate::AnimationClock)),
//! so playback, pausing, scrubbing and crossfades are the same operation.

use crate::model::{Aabb, MeshData};
use glam::{Mat4, Quat, Vec3, Vec4};
use std::sync::Arc;

/// A local transform: scale, then rotate, then translate (glTF's TRS order).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Transform::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Transform = Transform { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE };

    pub fn new(translation: Vec3, rotation: Quat, scale: Vec3) -> Transform {
        Transform { translation, rotation, scale }
    }

    pub fn from_translation(translation: Vec3) -> Transform {
        Transform { translation, ..Transform::IDENTITY }
    }

    pub fn from_rotation(rotation: Quat) -> Transform {
        Transform { rotation, ..Transform::IDENTITY }
    }

    /// Decompose an affine matrix (shear is lost; glTF node matrices have none).
    pub fn from_mat4(m: Mat4) -> Transform {
        let (scale, rotation, translation) = m.to_scale_rotation_translation();
        Transform { translation, rotation, scale }
    }

    pub fn to_mat4(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    /// `self` at `w = 0`, `other` at `w = 1`: translation and scale
    /// interpolate linearly, rotation along the shorter arc.
    pub fn lerp(&self, other: &Transform, w: f32) -> Transform {
        Transform {
            translation: self.translation.lerp(other.translation, w),
            rotation: self.rotation.slerp(other.rotation, w),
            scale: self.scale.lerp(other.scale, w),
        }
    }
}

/// One node of a model's hierarchy.
#[derive(Clone, Debug)]
pub struct Node {
    pub name: Option<String>,
    /// Index of the parent node; `None` for a root.
    pub parent: Option<usize>,
    /// The node's local transform when nothing animates it.
    pub rest: Transform,
}

/// A skeleton: the nodes acting as joints, and for each the inverse of its
/// model-space transform at bind time.
///
/// A skinned vertex lands at `Σ wᵢ · global(joints[jᵢ]) · inverse_bind[jᵢ] · p`
/// in model space. The skinned mesh's own node transform does not apply
/// (glTF's rule): the joints place it.
#[derive(Clone, Debug)]
pub struct Skin {
    pub joints: Vec<usize>,
    pub inverse_bind: Vec<Mat4>,
}

/// Per-vertex skin influences: up to four joints (indices into the skin's
/// `joints`) and their weights. Weights are normalised to sum to 1 when the
/// mesh is built.
#[derive(Clone, Debug, PartialEq)]
pub struct SkinWeights {
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
}

/// How a channel's values change between keyframes (glTF `interpolation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    /// Hold each keyframe's value until the next.
    Step,
    /// Straight-line between keyframes; rotations along the shorter arc.
    Linear,
    /// Hermite spline. Values come in triples per keyframe:
    /// (in-tangent, value, out-tangent).
    CubicSpline,
}

/// The keyframe values of one channel.
#[derive(Clone, Debug)]
pub enum ChannelValues {
    Translation(Vec<Vec3>),
    Rotation(Vec<Quat>),
    Scale(Vec<Vec3>),
}

impl ChannelValues {
    fn len(&self) -> usize {
        match self {
            ChannelValues::Translation(v) | ChannelValues::Scale(v) => v.len(),
            ChannelValues::Rotation(v) => v.len(),
        }
    }
}

/// One animated property of one node.
#[derive(Clone, Debug)]
pub struct Channel {
    pub node: usize,
    pub interpolation: Interpolation,
    /// Keyframe times in seconds, non-decreasing.
    pub times: Vec<f32>,
    pub values: ChannelValues,
}

impl Channel {
    /// Why this channel can't be sampled, if it can't.
    fn problem(&self) -> Option<&'static str> {
        if self.times.is_empty() {
            return Some("an animation channel has no keyframes");
        }
        if self.times.iter().any(|t| !t.is_finite()) || self.times.windows(2).any(|w| w[1] < w[0]) {
            return Some("animation keyframe times are not finite and non-decreasing");
        }
        let per_key = if self.interpolation == Interpolation::CubicSpline { 3 } else { 1 };
        if self.values.len() != self.times.len() * per_key {
            return Some("animation channel value count does not match its keyframes");
        }
        None
    }
}

#[derive(Debug)]
struct AnimationData {
    name: Option<String>,
    duration: f32,
    channels: Vec<Channel>,
}

/// A clip: channels that move nodes over time. Cheap to clone (shared).
///
/// A clip addresses nodes by index, so it applies to the model it came from
/// (or to another model with the same hierarchy). Channels naming a node the
/// pose doesn't have are skipped.
#[derive(Clone, Debug)]
pub struct Animation {
    data: Arc<AnimationData>,
}

impl Animation {
    /// A clip from channels. Its duration is the last keyframe time of any
    /// channel.
    ///
    /// Panics on a malformed channel (no keyframes, times out of order, or a
    /// value count that doesn't match the keyframes) — sampling it would read
    /// out of range. Loaded clips are checked by the loader instead and
    /// reported as [`ModelError::BadAnimation`](crate::ModelError::BadAnimation).
    pub fn new(name: Option<String>, channels: Vec<Channel>) -> Animation {
        if let Some(problem) = Animation::problem(&channels) {
            panic!("{problem}");
        }
        Animation::new_unchecked(name, channels)
    }

    pub(crate) fn problem(channels: &[Channel]) -> Option<&'static str> {
        channels.iter().find_map(Channel::problem)
    }

    pub(crate) fn new_unchecked(name: Option<String>, channels: Vec<Channel>) -> Animation {
        let duration = channels.iter().filter_map(|c| c.times.last().copied()).fold(0.0, f32::max);
        Animation { data: Arc::new(AnimationData { name, duration, channels }) }
    }

    pub fn name(&self) -> Option<&str> {
        self.data.name.as_deref()
    }

    /// Seconds from the clip's start to its last keyframe.
    pub fn duration(&self) -> f32 {
        self.data.duration
    }

    pub fn channels(&self) -> &[Channel] {
        &self.data.channels
    }

    /// `time` wrapped into `[0, duration)`, for a clip that repeats.
    pub fn looped(&self, time: f32) -> f32 {
        if self.data.duration > 0.0 {
            time.rem_euclid(self.data.duration)
        } else {
            0.0
        }
    }

    /// Overwrite the nodes this clip drives with their values at `time`
    /// (clamped to the clip: before the start holds the first keyframe, after
    /// the end the last).
    fn apply(&self, time: f32, locals: &mut [Transform]) {
        for ch in &self.data.channels {
            let Some(local) = locals.get_mut(ch.node) else { continue };
            let at = Cursor::find(&ch.times, time);
            match &ch.values {
                ChannelValues::Translation(v) => local.translation = sample_vec3(v, ch.interpolation, at),
                ChannelValues::Scale(v) => local.scale = sample_vec3(v, ch.interpolation, at),
                ChannelValues::Rotation(v) => local.rotation = sample_quat(v, ch.interpolation, at),
            }
        }
    }
}

/// Where a time falls among a channel's keyframes.
#[derive(Clone, Copy, Debug)]
enum Cursor {
    /// On (or clamped to) keyframe `k`.
    Key(usize),
    /// Between keyframes `k` and `k + 1`, `u` of the way (0..1), the gap
    /// being `dt` seconds.
    Between { k: usize, u: f32, dt: f32 },
}

impl Cursor {
    fn find(times: &[f32], t: f32) -> Cursor {
        let last = times.len() - 1;
        if t <= times[0] {
            return Cursor::Key(0);
        }
        if t >= times[last] {
            return Cursor::Key(last);
        }
        // First keyframe strictly after `t`, minus one: the segment start.
        let k = times.partition_point(|&x| x <= t) - 1;
        let dt = times[k + 1] - times[k];
        if dt <= 0.0 {
            return Cursor::Key(k + 1);
        }
        Cursor::Between { k, u: (t - times[k]) / dt, dt }
    }
}

/// Cubic Hermite basis: the value between `v0` (out-tangent `b0`) and `v1`
/// (in-tangent `a1`) at `u`, tangents scaled by the segment length `dt`
/// (glTF Appendix C).
fn hermite(v0: Vec4, b0: Vec4, a1: Vec4, v1: Vec4, u: f32, dt: f32) -> Vec4 {
    let (u2, u3) = (u * u, u * u * u);
    v0 * (2.0 * u3 - 3.0 * u2 + 1.0)
        + b0 * (dt * (u3 - 2.0 * u2 + u))
        + v1 * (-2.0 * u3 + 3.0 * u2)
        + a1 * (dt * (u3 - u2))
}

fn sample_vec3(v: &[Vec3], interp: Interpolation, at: Cursor) -> Vec3 {
    match (interp, at) {
        (Interpolation::CubicSpline, Cursor::Key(k)) => v[3 * k + 1],
        (_, Cursor::Key(k)) => v[k],
        (Interpolation::Step, Cursor::Between { k, .. }) => v[k],
        (Interpolation::Linear, Cursor::Between { k, u, .. }) => v[k].lerp(v[k + 1], u),
        (Interpolation::CubicSpline, Cursor::Between { k, u, dt }) => {
            let (b0, v0) = (v[3 * k + 2], v[3 * k + 1]);
            let (a1, v1) = (v[3 * k + 3], v[3 * k + 4]);
            hermite(v0.extend(0.0), b0.extend(0.0), a1.extend(0.0), v1.extend(0.0), u, dt).truncate()
        }
    }
}

fn sample_quat(v: &[Quat], interp: Interpolation, at: Cursor) -> Quat {
    match (interp, at) {
        (Interpolation::CubicSpline, Cursor::Key(k)) => v[3 * k + 1].normalize(),
        (_, Cursor::Key(k)) => v[k].normalize(),
        (Interpolation::Step, Cursor::Between { k, .. }) => v[k].normalize(),
        (Interpolation::Linear, Cursor::Between { k, u, .. }) => v[k].normalize().slerp(v[k + 1].normalize(), u),
        (Interpolation::CubicSpline, Cursor::Between { k, u, dt }) => {
            let q = |i: usize| Vec4::from(v[i]);
            Quat::from_vec4(hermite(q(3 * k + 1), q(3 * k + 2), q(3 * k + 3), q(3 * k + 4), u, dt)).normalize()
        }
    }
}

/// The local transform of every node of one model: what a clip is sampled
/// into and what a posed model is drawn with.
#[derive(Clone, Debug, PartialEq)]
pub struct Pose {
    model: u64,
    locals: Vec<Transform>,
}

impl Pose {
    /// Every node at its rest transform.
    pub fn rest(model: &crate::Model) -> Pose {
        Pose { model: model.id(), locals: model.nodes().iter().map(|n| n.rest).collect() }
    }

    /// This pose with `anim` sampled at `time` seconds over it: the nodes the
    /// clip drives take its values, the rest keep theirs.
    pub fn sampled(mut self, anim: &Animation, time: f32) -> Pose {
        self.sample(anim, time);
        self
    }

    /// In-place [`sampled`](Pose::sampled).
    pub fn sample(&mut self, anim: &Animation, time: f32) {
        anim.apply(time, &mut self.locals);
    }

    /// `self` at `w = 0`, `other` at `w = 1`, node by node — a crossfade.
    ///
    /// Panics if the two poses belong to different models.
    pub fn blend(&self, other: &Pose, w: f32) -> Pose {
        assert_eq!(self.model, other.model, "blending poses of two different models");
        let w = w.clamp(0.0, 1.0);
        Pose {
            model: self.model,
            locals: self.locals.iter().zip(&other.locals).map(|(a, b)| a.lerp(b, w)).collect(),
        }
    }

    /// Replace one node's local transform (procedural motion: aiming a head,
    /// an IK result). Out-of-range nodes are ignored.
    pub fn set(&mut self, node: usize, transform: Transform) -> &mut Self {
        if let Some(l) = self.locals.get_mut(node) {
            *l = transform;
        }
        self
    }

    pub fn get(&self, node: usize) -> Option<Transform> {
        self.locals.get(node).copied()
    }

    pub fn locals(&self) -> &[Transform] {
        &self.locals
    }

    /// The id of the model this pose belongs to.
    pub fn model_id(&self) -> u64 {
        self.model
    }
}

/// Model-space transform of every node: each node's local transform under
/// its parent's. `order` lists parents before their children.
pub(crate) fn globals(nodes: &[Node], order: &[usize], locals: &[Transform]) -> Vec<Mat4> {
    let mut out = vec![Mat4::IDENTITY; nodes.len()];
    for &n in order {
        let local = locals[n].to_mat4();
        out[n] = match nodes[n].parent {
            Some(p) => out[p] * local,
            None => local,
        };
    }
    out
}

/// Parents-first visiting order, or `None` if the parent links form a cycle
/// or point out of range.
pub(crate) fn hierarchy_order(nodes: &[Node]) -> Option<Vec<usize>> {
    let mut order = Vec::with_capacity(nodes.len());
    // 0 = unvisited, 1 = on the current path, 2 = placed.
    let mut state = vec![0u8; nodes.len()];
    for start in 0..nodes.len() {
        let mut path = Vec::new();
        let mut n = start;
        loop {
            match state[n] {
                2 => break,
                1 => return None, // cycle
                _ => {}
            }
            state[n] = 1;
            path.push(n);
            match nodes[n].parent {
                Some(p) if p < nodes.len() => n = p,
                Some(_) => return None,
                None => break,
            }
        }
        // `path` runs child → ancestor; place ancestors first.
        for &p in path.iter().rev() {
            state[p] = 2;
            order.push(p);
        }
    }
    Some(order)
}

/// Each joint's skinning matrix for `skin` under `globals`.
pub(crate) fn joint_matrices(skin: &Skin, globals: &[Mat4]) -> Vec<Mat4> {
    skin.joints.iter().zip(&skin.inverse_bind).map(|(&j, ibm)| globals[j] * *ibm).collect()
}

/// `mesh`'s positions skinned by `joints` (model space).
pub fn skinned_positions(mesh: &MeshData, joints: &[Mat4]) -> Vec<Vec3> {
    let Some(skin) = &mesh.skin else {
        return mesh.positions.iter().map(|p| Vec3::from(*p)).collect();
    };
    mesh.positions
        .iter()
        .zip(skin.joints.iter().zip(&skin.weights))
        .map(|(p, (j, w))| {
            let m = (0..4).fold(Mat4::ZERO, |acc, i| acc + joints[j[i] as usize] * w[i]);
            m.transform_point3(Vec3::from(*p))
        })
        .collect()
}

/// Bounds of skinned `positions`.
pub(crate) fn bounds_of(points: &[Vec3]) -> Aabb {
    Aabb::from_points(points.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    fn approx(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-4
    }

    fn channel(interpolation: Interpolation, times: Vec<f32>, values: ChannelValues) -> Channel {
        Channel { node: 0, interpolation, times, values }
    }

    fn sample_translation(ch: Channel, t: f32) -> Vec3 {
        let anim = Animation::new(None, vec![ch]);
        let mut locals = vec![Transform::IDENTITY];
        anim.apply(t, &mut locals);
        locals[0].translation
    }

    #[test]
    fn linear_translation_interpolates_and_clamps() {
        let ch = || {
            channel(
                Interpolation::Linear,
                vec![1.0, 3.0],
                ChannelValues::Translation(vec![Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)]),
            )
        };
        assert!(approx(sample_translation(ch(), 2.0), Vec3::new(2.0, 0.0, 0.0)));
        assert!(approx(sample_translation(ch(), 0.0), Vec3::ZERO), "before the start holds the first key");
        assert!(approx(sample_translation(ch(), 9.0), Vec3::new(4.0, 0.0, 0.0)), "after the end holds the last");
    }

    #[test]
    fn step_holds_the_previous_keyframe() {
        let ch = channel(
            Interpolation::Step,
            vec![0.0, 1.0],
            ChannelValues::Translation(vec![Vec3::X, Vec3::Y]),
        );
        assert!(approx(sample_translation(ch, 0.99), Vec3::X));
    }

    #[test]
    fn cubic_spline_passes_through_keys_and_follows_tangents() {
        // 0 → 1 over one second, both tangents 0: the smoothstep curve.
        let v = vec![
            Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, // in, value, out @ t=0
            Vec3::ZERO, Vec3::X, Vec3::ZERO, // @ t=1
        ];
        let ch = || channel(Interpolation::CubicSpline, vec![0.0, 1.0], ChannelValues::Translation(v.clone()));
        assert!(approx(sample_translation(ch(), 0.0), Vec3::ZERO));
        assert!(approx(sample_translation(ch(), 1.0), Vec3::X));
        assert!(approx(sample_translation(ch(), 0.5), Vec3::new(0.5, 0.0, 0.0)));
        // smoothstep(0.25) = 3u² − 2u³ = 0.15625
        assert!(approx(sample_translation(ch(), 0.25), Vec3::new(0.15625, 0.0, 0.0)));
    }

    #[test]
    fn linear_rotation_takes_the_shorter_arc() {
        // q and −q are the same rotation; interpolating toward −q must not
        // swing the long way round.
        let a = Quat::from_rotation_y(0.0);
        let b = -Quat::from_rotation_y(FRAC_PI_2);
        let anim = Animation::new(None, vec![channel(Interpolation::Linear, vec![0.0, 1.0], ChannelValues::Rotation(vec![a, b]))]);
        let mut locals = vec![Transform::IDENTITY];
        anim.apply(0.5, &mut locals);
        let angle = locals[0].rotation.angle_between(Quat::from_rotation_y(FRAC_PI_2 / 2.0));
        assert!(angle < 1e-3, "halfway is 45°, off by {angle}");
    }

    #[test]
    fn looped_wraps_and_duration_is_the_last_key() {
        let anim = Animation::new(
            Some("spin".into()),
            vec![channel(Interpolation::Linear, vec![0.0, 2.0], ChannelValues::Scale(vec![Vec3::ONE, Vec3::ONE]))],
        );
        assert_eq!(anim.duration(), 2.0);
        assert_eq!(anim.looped(5.0), 1.0);
        assert_eq!(anim.looped(-0.5), 1.5);
        assert_eq!(anim.name(), Some("spin"));
    }

    #[test]
    #[should_panic(expected = "value count")]
    fn a_channel_with_mismatched_values_is_rejected() {
        Animation::new(
            None,
            vec![channel(Interpolation::Linear, vec![0.0, 1.0], ChannelValues::Translation(vec![Vec3::ZERO]))],
        );
    }

    #[test]
    fn globals_compose_down_the_hierarchy_whatever_the_index_order() {
        // Child listed before its parent.
        let nodes = vec![
            Node { name: None, parent: Some(1), rest: Transform::from_translation(Vec3::X) },
            Node { name: None, parent: None, rest: Transform::from_rotation(Quat::from_rotation_z(FRAC_PI_2)) },
        ];
        let order = hierarchy_order(&nodes).unwrap();
        assert_eq!(order, vec![1, 0]);
        let locals: Vec<_> = nodes.iter().map(|n| n.rest).collect();
        let g = globals(&nodes, &order, &locals);
        // +X rotated 90° about Z is +Y.
        assert!(approx(g[0].transform_point3(Vec3::ZERO), Vec3::Y));
    }

    #[test]
    fn a_parent_cycle_is_rejected() {
        let nodes = vec![
            Node { name: None, parent: Some(1), rest: Transform::IDENTITY },
            Node { name: None, parent: Some(0), rest: Transform::IDENTITY },
        ];
        assert!(hierarchy_order(&nodes).is_none());
    }

    #[test]
    fn skinning_blends_joint_matrices_by_weight() {
        let mesh = MeshData::new(vec![[0.0, 1.0, 0.0]; 3], Some(vec![[0.0, 0.0, 1.0]; 3]), None, Some(vec![0, 1, 2]))
            .with_skin(SkinWeights {
                joints: vec![[0, 0, 0, 0], [1, 0, 0, 0], [0, 1, 0, 0]],
                weights: vec![[1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.5, 0.5, 0.0, 0.0]],
            });
        let joints = [Mat4::IDENTITY, Mat4::from_translation(Vec3::new(2.0, 0.0, 0.0))];
        let p = skinned_positions(&mesh, &joints);
        assert!(approx(p[0], Vec3::new(0.0, 1.0, 0.0)));
        assert!(approx(p[1], Vec3::new(2.0, 1.0, 0.0)));
        assert!(approx(p[2], Vec3::new(1.0, 1.0, 0.0)), "half each: halfway");
    }

    #[test]
    fn transform_lerp_ends_match_and_mat4_round_trips() {
        let a = Transform::new(Vec3::ZERO, Quat::IDENTITY, Vec3::ONE);
        let b = Transform::new(Vec3::new(2.0, 0.0, 0.0), Quat::from_rotation_x(1.0), Vec3::splat(3.0));
        assert_eq!(a.lerp(&b, 0.0), a);
        assert!(approx(a.lerp(&b, 1.0).translation, b.translation));
        let back = Transform::from_mat4(b.to_mat4());
        assert!(approx(back.translation, b.translation) && approx(back.scale, b.scale));
        assert!(back.rotation.angle_between(b.rotation) < 1e-4);
    }
}
