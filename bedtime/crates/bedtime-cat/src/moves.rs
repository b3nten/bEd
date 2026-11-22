//! The cat's moves: each a pose for a moment in time.
//!
//! The moves themselves are drawn in Blender (`Model::clips`). Here they are
//! played, and what can't be drawn ahead of time is laid over them: where
//! the cat looks, its eyes, ears and tail, and the paw that swats at the
//! pointer, which goes where the pointer is.
//!
//! Joints are turned about the cat's own axes: x ahead, y up, its left at -z.
use super::rig::{Model, Pose};
use bevy::prelude::*;
use std::f32::consts::{PI, TAU};

/// A pose being worked on.
pub struct Puppet<'a> {
    pub model: &'a Model,
    pub pose: Pose,
}

/// A front leg: the two bones that reach, the wrist they place, and the paw.
#[derive(Clone, Copy)]
pub struct Leg {
    upper: usize,
    mid: usize,
    wrist: usize,
    fingers: usize,
}

fn angle(v: Vec2) -> f32 {
    v.y.atan2(v.x).to_degrees()
}

/// `smooth`, gentler still at its ends.
pub fn smoother(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

pub fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 0 at the ends of 0..1, 1 through the middle: `edge` is how much of the
/// span each rise takes.
pub fn bell(t: f32, edge: f32) -> f32 {
    smooth(t / edge) * smooth((1.0 - t) / edge)
}

impl<'a> Puppet<'a> {
    pub fn new(model: &'a Model, pose: Pose) -> Self {
        Self { model, pose }
    }

    pub fn at(&self, joint: usize) -> Vec3 {
        self.model.global(&self.pose, joint).w_axis.truncate()
    }

    fn flat(&self, joint: usize) -> Vec2 {
        self.at(joint).truncate()
    }

    /// Turn a joint about an axis of the cat's space.
    pub fn rotate(&mut self, joint: usize, axis: Vec3, degrees: f32) {
        if degrees == 0.0 {
            return;
        }
        let parent = self.model.parent_rotation(&self.pose, joint);
        let turn = Quat::from_axis_angle(axis, degrees.to_radians());
        let local = &mut self.pose.0[joint].rotation;
        *local = (parent.inverse() * turn * parent * *local).normalize();
    }

    /// Tip a joint up (what points ahead rises).
    pub fn pitch(&mut self, joint: usize, degrees: f32) {
        self.rotate(joint, Vec3::Z, degrees);
    }

    /// Turn a joint to the cat's left.
    pub fn yaw(&mut self, joint: usize, degrees: f32) {
        self.rotate(joint, Vec3::Y, degrees);
    }

    /// Lean a joint's top to the cat's right.
    pub fn roll(&mut self, joint: usize, degrees: f32) {
        self.rotate(joint, Vec3::X, degrees);
    }

    /// The way a bone points, seen from the side: 0 is ahead, 90 up. A bone
    /// lies along its joint's own y.
    fn points(&self, joint: usize) -> f32 {
        let rotation = self.model.parent_rotation(&self.pose, joint) * self.pose.0[joint].rotation;
        angle((rotation * Vec3::Y).truncate())
    }

    /// Point a bone at an angle, seen from the side.
    fn aim(&mut self, joint: usize, degrees: f32) {
        self.pitch(joint, degrees - self.points(joint));
    }

    /// Move the whole cat.
    pub fn shift(&mut self, by: Vec2) {
        self.slide(self.model.bones.body, by);
    }

    /// Move one joint, and all that hangs from it.
    fn slide(&mut self, joint: usize, by: Vec2) {
        let parent = self.model.parent_rotation(&self.pose, joint);
        self.pose.0[joint].translation += parent.inverse() * by.extend(0.0);
    }

    /// Tip the whole cat about a point on the ground under its middle.
    pub fn tip(&mut self, degrees: f32) {
        let body = self.model.bones.body;
        let before = self.model.global(&self.pose, body);
        let turn = Mat4::from_rotation_z(degrees.to_radians());
        let parent = self.model.nodes[body]
            .parent
            .map_or(Mat4::IDENTITY, |parent| {
                self.model.global(&self.pose, parent)
            });
        self.pose.0[body] = Transform::from_matrix(parent.inverse() * turn * before);
    }

    fn front_leg(&self, right: bool) -> Leg {
        let b = &self.model.bones;
        if right {
            Leg {
                upper: b.r_upper_arm,
                mid: b.r_forearm,
                wrist: b.r_hand,
                fingers: b.r_fingers,
            }
        } else {
            Leg {
                upper: b.l_upper_arm,
                mid: b.l_forearm,
                wrist: b.l_hand,
                fingers: b.l_fingers,
            }
        }
    }

    /// Bend a leg's two bones so its wrist is at `target`, or as near as the
    /// leg reaches. The elbow points back.
    fn reach(&mut self, leg: &Leg, target: Vec2) {
        let from = self.flat(leg.upper);
        let upper = (self.flat(leg.mid) - from).length();
        let lower = (self.flat(leg.wrist) - self.flat(leg.mid)).length();
        let to = target - from;
        let far = to
            .length()
            .clamp((upper - lower).abs() + 0.05, upper + lower - 0.05);
        let inside =
            ((upper * upper + far * far - lower * lower) / (2.0 * upper * far)).clamp(-1.0, 1.0);
        self.aim(leg.upper, angle(to) - inside.acos().to_degrees());
        let to = from + to.normalize_or_zero() * far - self.flat(leg.mid);
        self.aim(leg.mid, angle(to));
    }

    /// Wave the tail side to side, the wave running down it.
    pub fn swish(&mut self, degrees: f32, phase: f32) {
        for (index, joint) in self.model.bones.tail().into_iter().enumerate() {
            let along = index as f32;
            self.yaw(
                joint,
                degrees * (0.4 + along * 0.2) * (phase - along * 0.7).sin(),
            );
        }
    }

    /// Close the eyes: 1 is shut. Each lid turns about the line through its
    /// eye's corners, which is its joint's own x.
    pub fn close_eyes(&mut self, amount: f32) {
        let b = &self.model.bones;
        for (upper, lower) in [
            (b.l_upper_lid, b.l_lower_lid),
            (b.r_upper_lid, b.r_lower_lid),
        ] {
            self.pose.0[upper].rotation *= Quat::from_rotation_x((-52.0 * amount).to_radians());
            self.pose.0[lower].rotation *= Quat::from_rotation_x((30.0 * amount).to_radians());
        }
    }

    /// Lay the ears back (1), or prick them forward (-1).
    pub fn ears(&mut self, back: f32) {
        let b = &self.model.bones;
        self.pitch(b.l_ear, 28.0 * back);
        self.pitch(b.r_ear, 28.0 * back);
        self.roll(b.l_ear, -14.0 * back.max(0.0));
        self.roll(b.r_ear, 14.0 * back.max(0.0));
    }

    /// Flick one ear.
    pub fn flick(&mut self, left: bool, amount: f32) {
        let b = &self.model.bones;
        let ear = if left { b.l_ear } else { b.r_ear };
        self.yaw(ear, if left { 26.0 } else { -26.0 } * amount);
    }

    /// Open the mouth: 1 is a yawn.
    pub fn gape(&mut self, amount: f32) {
        self.pitch(self.model.bones.jaw, -34.0 * amount);
    }

    /// Turn the head by a gaze, shared down the neck.
    pub fn look(&mut self, yaw: f32, pitch: f32) {
        let b = &self.model.bones;
        for (joint, share) in [(b.neck1, 0.2), (b.neck2, 0.25), (b.head, 0.55)] {
            self.yaw(joint, yaw * share);
            self.pitch(joint, pitch * share);
        }
    }

    /// The gaze that would have the head look at `target`.
    pub fn gaze_at(&self, target: Vec3) -> Vec2 {
        let head = self.model.bones.head;
        let ahead = self.model.turned(&self.pose, head) * Vec3::X;
        let to = target - self.at(head);
        let yaw = (-to.z).atan2(to.x) - (-ahead.z).atan2(ahead.x);
        let pitch = to.y.atan2(to.x.hypot(to.z)) - ahead.y.atan2(ahead.x.hypot(ahead.z));
        let yaw = (yaw + PI).rem_euclid(TAU) - PI;
        Vec2::new(
            yaw.to_degrees().clamp(-80.0, 80.0),
            pitch.to_degrees().clamp(-45.0, 50.0),
        )
    }

    /// Fill the chest: 1 is a full breath.
    pub fn breathe(&mut self, amount: f32) {
        let b = &self.model.bones;
        self.pose.0[b.spine2].scale *= 1.0 + 0.025 * amount;
    }
}

/// A move as the viewer sees it: `side` is 1 when the cat's right is what
/// the camera sees, as the moves are drawn, and -1 for its left.
fn seen(model: &Model, pose: Pose, side: f32) -> Pose {
    if side < 0.0 {
        model.mirrored(&pose)
    } else {
        pose
    }
}

/// How the cat walks.
pub struct Gait {
    /// Seconds per cycle.
    pub period: f32,
    /// Model units covered each second.
    pub speed: f32,
}

pub fn gait(model: &Model, trot: bool) -> Gait {
    let clip = if trot {
        &model.clips.trot
    } else {
        &model.clips.walk
    };
    Gait {
        period: clip.length,
        speed: model.ground_speed(clip),
    }
}

/// A step of a walk: `phase` counts cycles.
/// `reach` scales its steps: under 1 as it sets off and pulls up.
pub fn walk(model: &Model, trot: bool, phase: f32, reach: f32) -> Pose {
    let clip = if trot {
        &model.clips.trot
    } else {
        &model.clips.walk
    };
    let mut pose = stand(model);
    pose.blend(
        &model.cycle(clip, phase * clip.length),
        reach.clamp(0.0, 1.0),
    );
    pose
}

/// Standing still.
pub fn stand(model: &Model) -> Pose {
    model.pose(&model.clips.stand, 0.0)
}

/// Sitting up, front paws together, its tail round the side that is seen.
pub fn sit(model: &Model, side: f32) -> Pose {
    seen(model, model.pose(&model.clips.sit, 0.0), side)
}

/// Washing the paw that is seen.
pub fn groom(model: &Model, time: f32, side: f32) -> Pose {
    seen(model, model.cycle(&model.clips.groom, time), side)
}

/// Lying like a sphinx, paws out in front, its tail on the side seen.
pub fn loaf(model: &Model, side: f32) -> Pose {
    seen(model, model.pose(&model.clips.loaf, 0.0), side)
}

/// Asleep: lying with its chin on its paws.
pub fn sleep(model: &Model, side: f32) -> Pose {
    seen(model, model.pose(&model.clips.sleep, 0.0), side)
}

/// A long stretch, front down and rump up: `u` runs 0 to 1 through it.
pub fn stretch(model: &Model, u: f32) -> Pose {
    let clip = &model.clips.stretch;
    model.pose(clip, u * clip.length)
}

/// Low to the ground, ready to spring, its rump wiggling as `time` passes.
/// At 0 it is still.
pub fn crouch(model: &Model, time: f32) -> Pose {
    model.cycle(&model.clips.crouch, time)
}

/// A spring through the air: `u` runs 0 (leaving the ground) to 1 (landing).
pub fn leap(model: &Model, u: f32) -> Pose {
    let clip = &model.clips.leap;
    model.pose(clip, u * clip.length)
}

/// Held up by the scruff: hanging, paddling at the air as `time` (in
/// seconds) passes, and swinging by `swing` degrees.
pub fn dangle(model: &Model, swing: f32, time: f32) -> Pose {
    let mut p = Puppet::new(model, model.cycle(&model.clips.dangle, time));
    p.tip(swing);
    // The tail swings further than the cat.
    p.pitch(model.bones.tail1, swing * 2.0);
    p.pose
}

/// With a paw held high: how far its shoulder rides up, and how far back
/// the chest tips (in degrees, at the two joints of the spine under it).
const SHRUG: Vec2 = Vec2::new(0.5, 1.5);
const REAR: [f32; 2] = [6.0, 15.0];
/// With a paw far out ahead: how far the chest leans after it.
const LEAN: [f32; 2] = [2.0, 3.5];
/// From the wrist to the pads of a paw that is reaching.
const PAD: f32 = 2.6;
/// The shortest and longest a front leg is, shoulder to wrist.
const ARM: [f32; 2] = [3.5, 9.3];

/// How far round from where it rests a paw is, seen from `shoulder`: in
/// degrees, counted round the front of the cat. Past 290 it would be coming
/// up through the cat's own chest, which is 70 the other way.
fn round(shoulder: Vec2, low: f32, paw: Vec2) -> f32 {
    (angle(paw - shoulder) - low + 70.0).rem_euclid(360.0) - 70.0
}

/// Where a sitting cat's front paw can go. A paw's place is told two ways:
/// as a point of the cat's space seen from the side, and as how far it has
/// swung about the shoulder (degrees from where it rests, round the front of
/// the cat) with how far out from the shoulder its pads are. A paw moved by
/// the second swings as a leg does, and never through the cat.
#[derive(Clone, Copy)]
pub struct Reach {
    shoulder: Vec2,
    low: f32,
    /// How far out the pads are with the paw on the ground, as near as they
    /// come, and at full stretch.
    pub rest: f32,
    pub near: f32,
    pub far: f32,
}

impl Reach {
    pub fn new(model: &Model, right: bool) -> Self {
        let p = Puppet::new(model, sit(model, if right { 1.0 } else { -1.0 }));
        let leg = p.front_leg(right);
        let shoulder = p.flat(leg.upper);
        let rest = p.flat(leg.wrist) - shoulder;
        Self {
            shoulder,
            low: angle(rest),
            rest: rest.length() + PAD,
            near: ARM[0] + PAD,
            far: ARM[1].max(rest.length()) + PAD,
        }
    }

    /// The paw on the ground: no swing, and its length at rest.
    pub fn home(&self) -> Vec2 {
        Vec2::new(0.0, self.rest)
    }

    /// The swing and length that put the pads on `point`, or as near as the
    /// leg reaches.
    pub fn swing(&self, point: Vec2) -> Vec2 {
        Vec2::new(
            round(self.shoulder, self.low, point),
            (point - self.shoulder).length().clamp(self.near, self.far),
        )
    }

    /// Where the pads are at a swing and length.
    pub fn point(&self, swing: Vec2) -> Vec2 {
        self.shoulder + Vec2::from_angle((self.low + swing.x).to_radians()) * swing.y
    }
}

/// Sitting, the pads of one front paw at `paw` (in the cat's space, seen
/// from the side; `Reach` says where that may be); `right` picks the paw.
pub fn bat(model: &Model, right: bool, paw: Vec2) -> Pose {
    let mut p = Puppet::new(model, sit(model, if right { 1.0 } else { -1.0 }));
    let b = &model.bones;
    let (leg, other) = (p.front_leg(right), p.front_leg(!right));
    // Where the paw rests, from the shoulder: how far, at what angle, and
    // how the wrist and toes are turned from that.
    let sitting = p.flat(leg.upper);
    let rest = p.flat(leg.wrist) - sitting;
    let low = angle(rest);
    let wrist = p.points(leg.wrist) - low;
    let toe = p.points(leg.fingers) - low;
    // The higher the paw goes, the more the cat rears up after it: its
    // chest tips back and the shoulder rides up with the arm, the other
    // paw staying where it was. With the shoulder left where it sits, the
    // skin from chest to arm is pulled out into a web. A paw far out ahead
    // takes the chest forward with it instead.
    let raise = smooth((low + round(sitting, low, paw) + 30.0) / 120.0);
    let lean = smooth((paw.x - sitting.x - 5.0) / 6.0) * (1.0 - raise);
    let planted = (
        p.flat(other.wrist),
        p.points(other.wrist),
        p.points(other.fingers),
    );
    p.pitch(b.spine3, REAR[0] * raise - LEAN[0] * lean);
    p.pitch(b.chest, REAR[1] * raise - LEAN[1] * lean);
    // The head stays as it was held.
    p.pitch(
        b.neck1,
        -(REAR[0] + REAR[1]) * raise + (LEAN[0] + LEAN[1]) * lean,
    );
    p.reach(&other, planted.0);
    p.aim(other.wrist, planted.1);
    p.aim(other.fingers, planted.2);
    p.slide(leg.upper, SHRUG * raise);
    // The paw's pads are at the point; the wrist stops short of it.
    let from = p.flat(leg.upper);
    let swing = round(from, low, paw);
    let length = ((paw - from).length() - PAD).clamp(ARM[0], ARM[1].max(rest.length()));
    p.reach(
        &leg,
        from + Vec2::from_angle((low + swing).to_radians()) * length,
    );
    // The wrist and toes turn with the arm, and the further the paw is from
    // where it rests the more they curl over what it reaches for.
    let amount = smooth((swing.abs() / 35.0).max((length - rest.length()).abs() / 2.5));
    let bend = |rest: f32, reaching: f32| {
        low + swing + rest + ((reaching - rest + 180.0).rem_euclid(360.0) - 180.0) * amount
    };
    p.aim(leg.wrist, bend(wrist, -18.0));
    p.aim(leg.fingers, bend(toe, -40.0));
    p.pose
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::skinning::SkinnedMeshInverseBindposes;

    fn model() -> Model {
        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<Assets<SkinnedMeshInverseBindposes>>();
        Model::load(&mut world).unwrap()
    }

    #[test]
    fn a_paw_at_rest_is_a_cat_sitting() {
        let model = model();
        let reach = Reach::new(&model, true);
        println!(
            "shoulder {} low {} rest {} near {} far {}",
            reach.shoulder, reach.low, reach.rest, reach.near, reach.far
        );
        let sitting = Puppet::new(&model, sit(&model, 1.0));
        let batting = Puppet::new(&model, bat(&model, true, reach.point(reach.home())));
        for joint in 0..model.nodes.len() {
            let apart = sitting.at(joint).distance(batting.at(joint));
            assert!(
                apart < 0.02,
                "{} is {apart} from where it sits",
                model.nodes[joint].name
            );
        }
    }

    #[test]
    fn a_swat_overhead_swings_round_the_front_of_the_cat() {
        let model = model();
        let reach = Reach::new(&model, true);
        let sitting = Puppet::new(&model, sit(&model, 1.0));
        let leg = sitting.front_leg(true);
        let rest = sitting.flat(leg.wrist);
        for target in [
            Vec2::new(17.0, 9.0),
            Vec2::new(14.0, 24.0),
            Vec2::new(8.0, 30.0),
            Vec2::new(4.0, 34.0),
        ] {
            let to = reach.swing(target);
            println!("{target}: swing {to}");
            let mut last = rest;
            for step in 0..=20 {
                let amount = step as f32 / 20.0;
                let p = Puppet::new(
                    &model,
                    bat(&model, true, reach.point(reach.home().lerp(to, amount))),
                );
                let wrist = p.flat(leg.wrist);
                // Never back through its chest, and never in a jump.
                assert!(
                    wrist.x >= rest.x - 0.01,
                    "reaching for {target}, {amount} of the way: wrist at {wrist}"
                );
                assert!(
                    wrist.distance(last) < 2.5,
                    "reaching for {target}, {amount} of the way: {last} to {wrist}"
                );
                last = wrist;
            }
            // And its pads end up on what it reached for, if that is in reach.
            let p = Puppet::new(&model, bat(&model, true, reach.point(to)));
            let pads = p.flat(leg.wrist) + (p.flat(leg.wrist) - p.flat(leg.mid)).normalize() * PAD;
            println!("  pads {pads} for {}", reach.point(to));
        }
    }
}
