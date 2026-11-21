//! What the cat does: a state machine over its moves.
//!
//! Left alone it wanders: walks somewhere, sits, washes, lies down, sleeps,
//! wakes and stretches. Stroked (the pointer moving over it) it purrs.
//! Clicked, it plays with the pointer until that stops moving or it is
//! clicked again: watches it, trots after it, bats at it when it is in
//! reach, and stalks and pounces on it when it isn't. Dragged, it is carried
//! by the scruff and dropped.
use super::{
    SCALE,
    moves::{self, Puppet, Reach, bell, smooth, smoother},
    paw::Paw,
    rig::{Model, Pose},
};
use bevy::prelude::*;
use std::f32::consts::PI;

/// How near the window's sides the cat's middle comes, in points.
const EDGE: f32 = 20.0 * SCALE;
/// How long the cat's `groom` move is: once through washing a paw.
const GROOM: f32 = 6.0;
/// Points per second per second.
const GRAVITY: f32 = 1100.0 * SCALE;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Act {
    Stand,
    Walk,
    Sit,
    Groom,
    Loaf,
    Sleep,
    Stretch,
    /// Low, rump swinging, about to spring.
    Stalk,
    Leap,
    Land,
    /// Sitting, swatting at the pointer.
    Bat,
    Petted,
    /// Carried by the scruff.
    Held,
    Fall,
}

/// What the cat can tell of the window this frame. Lengths are in points;
/// heights are above the ground the cat walks on.
#[derive(Default, Clone, Copy)]
pub struct Senses {
    pub dt: f32,
    /// The window's width.
    pub width: f32,
    /// The pointer: how far along the window, and how high.
    pub pointer: Option<Vec2>,
    /// The pointer is on the cat.
    pub over: bool,
    pub clicked: bool,
    /// The cat is being dragged.
    pub held: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mark {
    /// Asleep.
    Z,
    Heart,
    /// Noticed something.
    Bang,
}

/// A little sign that drifts up from the cat.
#[derive(Clone, Copy)]
pub struct Sign {
    pub mark: Mark,
    /// Where it is: along the window, and above the ground.
    pub at: Vec2,
    pub age: f32,
    pub life: f32,
    drift: Vec2,
}

pub struct Mind {
    /// Say each act as it starts (`BED_CAT_LOG`).
    log: bool,
    seed: u64,
    time: f32,
    /// Where the cat's middle is along the window, and how far off the
    /// ground it is, in points.
    pub x: f32,
    pub lift: f32,
    /// The way it faces, as a turn about the vertical: 0 is to the right,
    /// -PI/2 out of the screen, -PI to the left.
    pub facing: f32,
    /// The way it means to face, and how fast it is turning.
    heading: f32,
    spin: f32,
    /// Walking: how far into its stride it is, 0 (setting off, pulling up)
    /// to 1.
    pace: f32,
    right: bool,
    pub act: Act,
    /// Seconds into the act, and how long it lasts.
    since: f32,
    length: f32,
    /// The pose the act began from, and how long the change takes.
    from: Option<Pose>,
    blend: f32,
    /// The last pose, before the eyes, ears and gaze were added.
    last: Option<Pose>,
    /// Walking: cycles walked, where to, and whether at a trot.
    phase: f32,
    goal: f32,
    trot: bool,
    /// A leap's start and end along the window, and its height.
    leap: (f32, f32, f32),
    /// Falling: points per second, upwards.
    rise: f32,
    /// Carried: how far it swings, and how fast.
    swing: f32,
    swing_speed: f32,
    /// It was lying down when the stroking began.
    lying: bool,
    /// Playing with the pointer, and for how long without seeing it move.
    pub playing: bool,
    /// Where the pointer was, and how long it has been there.
    pointer: Option<Vec2>,
    still: f32,
    /// How much it has been stroked lately, and how long since.
    stroked: f32,
    unstroked: f32,
    /// Where the head is turned, in degrees left and up, and where it is
    /// glancing when nothing holds its eye.
    gaze: Vec2,
    glance: Vec2,
    next_glance: f32,
    eyes: f32,
    ears: f32,
    next_blink: f32,
    next_flick: f32,
    flick_left: bool,
    /// Seconds into a yawn.
    yawn: Option<f32>,
    /// The paw it swats with.
    paw: Paw,
    /// Swatting: the pointer is still there to reach for.
    reaching: bool,
    /// How lately its paw landed on the pointer: 1 just now, down to 0.
    thrill: f32,
    next_sign: f32,
    pub signs: Vec<Sign>,
    /// From the last frame drawn: the head, in points from the cat's middle
    /// on the ground; the point of the model at its middle; and the height
    /// of the model's lowest point.
    pub head: Vec2,
    pub middle: Vec3,
    pub floor: f32,
}

impl Mind {
    pub fn new(seed: u64) -> Self {
        let mut mind = Self {
            log: std::env::var_os("BED_CAT_LOG").is_some(),
            seed: seed | 1,
            time: 0.0,
            x: -1.0,
            lift: 0.0,
            facing: -0.6,
            heading: -0.6,
            spin: 0.0,
            pace: 0.0,
            right: true,
            act: Act::Sit,
            since: 0.0,
            length: 3.0,
            from: None,
            blend: 0.4,
            last: None,
            phase: 0.0,
            goal: 0.0,
            trot: false,
            leap: (0.0, 0.0, 0.0),
            rise: 0.0,
            swing: 0.0,
            swing_speed: 0.0,
            lying: false,
            playing: false,
            pointer: None,
            still: 0.0,
            stroked: 0.0,
            unstroked: 0.0,
            gaze: Vec2::ZERO,
            glance: Vec2::ZERO,
            next_glance: 1.0,
            eyes: 0.0,
            ears: 0.0,
            next_blink: 2.0,
            next_flick: 5.0,
            flick_left: false,
            yawn: None,
            paw: Paw::new(seed.rotate_left(17)),
            reaching: false,
            thrill: 0.0,
            next_sign: 0.0,
            signs: Vec::new(),
            head: Vec2::new(0.0, 40.0),
            middle: Vec3::ZERO,
            floor: 0.0,
        };
        mind.right = mind.chance(0.5);
        mind.turn(0.6);
        mind.facing = mind.heading;
        // `BED_CAT_ACT` starts it at a known place, doing a known thing, to
        // look at or to script.
        if let Ok(act) = std::env::var("BED_CAT_ACT") {
            mind.x = 220.0;
            mind.right = true;
            mind.act = match act.as_str() {
                "stand" => Act::Stand,
                "groom" => Act::Groom,
                "loaf" => Act::Loaf,
                "sleep" => Act::Sleep,
                "stretch" => Act::Stretch,
                _ => Act::Sit,
            };
            mind.length = 30.0;
            mind.turn(0.6);
            mind.facing = mind.heading;
        }
        mind
    }

    /// A number from 0 up to 1.
    fn random(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed >> 40) as f32 / (1u64 << 24) as f32
    }

    fn between(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.random()
    }

    fn chance(&mut self, odds: f32) -> bool {
        self.random() < odds
    }

    /// 1 when the camera sees the cat's right side, -1 its left.
    fn side(&self) -> f32 {
        if self.facing > -PI / 2.0 { 1.0 } else { -1.0 }
    }

    /// Mean to face the way it is headed, turned `out` towards the screen.
    fn turn(&mut self, out: f32) {
        self.heading = if self.right { -out } else { out - PI };
    }

    fn start(&mut self, act: Act, length: f32, blend: f32) {
        if self.log {
            info!("cat: {act:?} for {length:.1}s at {:.0}", self.x);
        }
        self.from = self.last.clone();
        self.blend = blend;
        if act == Act::Walk && self.act != Act::Walk {
            self.pace = 0.0;
        }
        self.act = act;
        self.since = 0.0;
        self.length = length;
        match act {
            Act::Walk => self.turn(if self.trot { 0.12 } else { 0.2 }),
            Act::Stand | Act::Stretch | Act::Stalk | Act::Leap | Act::Land => self.turn(0.3),
            Act::Bat => self.turn(0.42),
            Act::Sit | Act::Groom | Act::Loaf if !self.playing && self.chance(0.22) => {
                // Now and then it sits facing out of the screen.
                self.heading = -PI / 2.0 + self.between(-0.3, 0.3);
            }
            Act::Sit | Act::Groom | Act::Loaf => self.turn(0.62),
            Act::Sleep | Act::Petted | Act::Held | Act::Fall => {}
        }
    }

    fn sign(&mut self, mark: Mark) {
        let drift = Vec2::new(self.between(-6.0, 10.0), self.between(16.0, 24.0));
        let life = if mark == Mark::Bang { 0.7 } else { 2.2 };
        self.signs.push(Sign {
            mark,
            at: Vec2::new(
                self.x + self.head.x + 6.0 * self.side(),
                self.lift + self.head.y + 10.0,
            ),
            age: 0.0,
            life,
            drift,
        });
    }

    /// The pointer, in the model's units from the cat's middle: across the
    /// window and up.
    fn pointer_from_cat(&self) -> Option<Vec2> {
        self.pointer
            .map(|pointer| Vec2::new(pointer.x - self.x, pointer.y - self.lift) / SCALE)
    }

    /// A point of the window (as `pointer_from_cat` gives it), `depth` in
    /// front of the screen, in the model's space.
    fn in_model(&self, point: Vec2, depth: f32) -> Vec3 {
        Quat::from_rotation_y(-self.facing) * Vec3::new(point.x, point.y + self.floor, depth)
            + Vec3::new(self.middle.x, 0.0, self.middle.z)
    }

    /// Go somewhere else along the window, taking `blend` seconds to set off.
    fn wander(&mut self, width: f32, blend: f32) {
        let (low, high) = (EDGE, (width - EDGE).max(EDGE));
        if high - low < 90.0 {
            self.start(Act::Sit, 6.0, 0.6);
            return;
        }
        for _ in 0..8 {
            self.goal = self.between(low, high);
            if (self.goal - self.x).abs() > 70.0 {
                break;
            }
        }
        self.right = self.goal > self.x;
        self.trot = false;
        self.start(Act::Walk, 60.0, blend);
    }

    fn stop_playing(&mut self) {
        self.playing = false;
        if matches!(self.act, Act::Walk | Act::Bat | Act::Stalk | Act::Stand) {
            let length = self.between(3.0, 7.0);
            self.start(Act::Sit, length, 0.5);
        }
    }

    /// What to do next, left alone.
    fn idle(&mut self, senses: &Senses) {
        let done = self.since >= self.length;
        match self.act {
            Act::Stand if done => {
                let pick = self.random();
                if pick < 0.55 {
                    self.wander(senses.width, 0.3);
                } else if pick < 0.85 {
                    let length = self.between(4.0, 10.0);
                    self.start(Act::Sit, length, 0.7);
                } else if pick < 0.93 {
                    self.start(Act::Stretch, 2.8, 0.3);
                } else {
                    let length = self.between(8.0, 20.0);
                    self.start(Act::Loaf, length, 0.9);
                }
            }
            Act::Walk => {
                let ahead = if self.right {
                    self.goal - self.x
                } else {
                    self.x - self.goal
                };
                if ahead <= 0.0 || done {
                    if self.chance(0.5) {
                        let length = self.between(0.8, 2.5);
                        self.start(Act::Stand, length, 0.35);
                    } else {
                        let length = self.between(4.0, 10.0);
                        self.start(Act::Sit, length, 0.7);
                    }
                }
            }
            Act::Sit if done => {
                let pick = self.random();
                if pick < 0.3 {
                    let loops = if self.chance(0.4) { 2.0 } else { 1.0 };
                    self.start(Act::Groom, GROOM * loops, 0.5);
                } else if pick < 0.6 {
                    self.wander(senses.width, 0.6);
                } else if pick < 0.8 {
                    let length = self.between(8.0, 20.0);
                    self.start(Act::Loaf, length, 0.9);
                } else if pick < 0.9 {
                    self.yawn = Some(0.0);
                    self.length += 3.0;
                } else {
                    self.length += self.between(3.0, 6.0);
                }
            }
            Act::Groom if done => {
                let length = self.between(2.0, 6.0);
                self.start(Act::Sit, length, 0.6);
            }
            Act::Loaf if done => {
                let pick = self.random();
                if pick < 0.5 {
                    let length = self.between(25.0, 70.0);
                    self.start(Act::Sleep, length, 1.2);
                } else if pick < 0.8 {
                    let length = self.between(3.0, 6.0);
                    self.start(Act::Sit, length, 0.9);
                } else {
                    self.yawn = Some(0.0);
                    self.length += 4.0;
                }
            }
            Act::Sleep if done => {
                self.yawn = Some(0.0);
                self.start(Act::Loaf, 3.2, 1.0);
                // Up it gets, and stretches.
                self.lying = true;
            }
            Act::Stretch if done => {
                let length = self.between(0.5, 1.5);
                self.start(Act::Stand, length, 0.3);
            }
            Act::Stalk | Act::Bat => {
                let length = self.between(2.0, 5.0);
                self.start(Act::Sit, length, 0.4);
            }
            Act::Land if done => {
                let length = self.between(0.4, 1.0);
                self.start(Act::Stand, length, 0.3);
            }
            _ => {}
        }
        // Woken, it gets up to stretch.
        if self.act == Act::Loaf && self.lying && self.since >= self.length - 0.1 {
            self.lying = false;
            self.start(Act::Stretch, 2.8, 0.9);
        }
    }

    /// What to do next, playing with the pointer.
    fn play(&mut self, model: &Model, senses: &Senses) {
        let Some(to) = self.pointer_from_cat() else {
            // The pointer left the window.
            if self.still > 3.0 {
                self.stop_playing();
            }
            return;
        };
        if self.still > 9.0 {
            self.yawn = Some(0.0);
            self.stop_playing();
            return;
        }
        let ahead = to.x.abs();
        let toward = to.x > 0.0;
        // How far in front of it the pointer is, the way it is sitting: less
        // than nothing is behind it.
        let before = if self.right { to.x } else { -to.x };
        // What it swats at is out in front of it, or over its head (a
        // pointer passing over is swatted straight up). A pointer on the cat
        // itself is too near for its paw to get to: that it only watches.
        let overhead = to.y >= 22.0 && (-5.0..19.0).contains(&before);
        let reachable = (1.0..28.0).contains(&to.y) && ((6.5..19.0).contains(&before) || overhead);
        // Too far to swat, near enough to spring at.
        let far = ahead >= 19.0 || to.y >= 28.0;
        match self.act {
            Act::Sit | Act::Stand => {
                if toward != self.right && ahead > 9.0 {
                    self.right = toward;
                    self.turn(0.5);
                }
                let facing = (self.facing - self.heading).abs() < 0.5;
                if ahead > 36.0 {
                    self.trot = true;
                    self.start(Act::Walk, 30.0, 0.3);
                } else if reachable && facing && self.since > 0.35 {
                    // Sitting, its paw is where a swat starts from: up it
                    // gets, and the swat is on its way as it settles.
                    let blend = if self.act == Act::Sit { 0.1 } else { 0.3 };
                    self.paw
                        .rest(&Reach::new(model, self.side() > 0.0), blend * 0.6);
                    self.reaching = true;
                    self.start(Act::Bat, 30.0, blend);
                } else if far && facing && to.y < 62.0 && self.since > 0.9 && self.still > 0.15 {
                    let length = self.between(0.7, 1.2);
                    self.start(Act::Stalk, length, 0.3);
                }
            }
            Act::Walk => {
                self.right = toward;
                self.turn(0.12);
                self.goal = senses.pointer.map_or(self.x, |pointer| pointer.x);
                let edge = if toward {
                    senses.width - EDGE - self.x
                } else {
                    self.x - EDGE
                };
                if ahead < 15.0 || edge < 2.0 {
                    self.start(Act::Sit, 30.0, 0.4);
                }
            }
            Act::Bat => {
                // Out of reach, it puts its paw down before it sits back.
                self.reaching = reachable;
                if !self.reaching && self.since > 0.5 && self.paw.home {
                    self.start(Act::Sit, 30.0, 0.35);
                }
            }
            Act::Stalk if self.since >= self.length => {
                if toward != self.right {
                    self.start(Act::Sit, 30.0, 0.3);
                    return;
                }
                // Spring at where the pointer is now, to land just short of
                // it; a pointer high up is jumped at.
                let sign = if toward { 1.0 } else { -1.0 };
                let end = (self.x + (to.x - sign * 9.0) * SCALE).clamp(
                    EDGE.min(senses.width / 2.0),
                    (senses.width - EDGE).max(senses.width / 2.0),
                );
                let height = (to.y - 7.0).clamp(7.0, 58.0) * SCALE;
                self.leap = (self.x, end, height);
                let length = 0.42 + (height / 260.0 + (end - self.x).abs() / 390.0) / SCALE;
                self.start(Act::Leap, length, 0.12);
            }
            Act::Land if self.since >= self.length => self.start(Act::Sit, 30.0, 0.35),
            Act::Loaf | Act::Sleep | Act::Groom | Act::Stretch => self.start(Act::Sit, 30.0, 0.45),
            _ => {}
        }
    }

    /// Live through `senses.dt` seconds, and say how the cat is posed.
    pub fn think(&mut self, model: &Model, senses: &Senses) -> Pose {
        let dt = senses.dt;
        self.time += dt;
        self.since += dt;
        if self.x < 0.0 {
            self.x = self.between(EDGE, (senses.width - EDGE).max(EDGE));
        }
        let moved = match (senses.pointer, self.pointer) {
            (Some(now), Some(before)) => (now - before).length(),
            (None, None) => 0.0,
            _ => 100.0,
        };
        self.pointer = senses.pointer;
        self.still = if moved > 0.5 { 0.0 } else { self.still + dt };

        if senses.held {
            if self.act != Act::Held {
                self.swing = 0.0;
                self.swing_speed = 0.0;
                self.sign(Mark::Bang);
                self.start(Act::Held, 0.0, 0.25);
            }
        } else if self.act == Act::Held {
            self.rise = 0.0;
            self.start(Act::Fall, 0.0, 0.3);
        }
        match self.act {
            Act::Held => {
                if let Some(pointer) = senses.pointer {
                    // It hangs from the pointer by its scruff, and swings
                    // behind as it is carried.
                    let to = pointer.x.clamp(8.0, senses.width - 8.0);
                    let speed = (to - self.x) / dt.max(0.001);
                    self.x = to;
                    self.lift = (pointer.y - 30.0 * SCALE).max(0.0);
                    self.swing_speed += (-speed * 2.0 / SCALE * self.side()
                        - self.swing * 60.0
                        - self.swing_speed * 5.0)
                        * dt;
                    self.swing = (self.swing + self.swing_speed * dt).clamp(-38.0, 38.0);
                }
            }
            Act::Fall => {
                self.rise -= GRAVITY * dt;
                self.lift += self.rise * dt;
                if self.lift <= 0.0 {
                    self.lift = 0.0;
                    self.x = self.x.clamp(
                        EDGE.min(senses.width / 2.0),
                        (senses.width - EDGE).max(senses.width / 2.0),
                    );
                    self.start(Act::Land, 0.45, 0.14);
                }
            }
            _ => {
                if senses.clicked {
                    if self.playing {
                        self.stop_playing();
                    } else {
                        self.playing = true;
                        self.still = 0.0;
                        self.sign(Mark::Bang);
                        if !matches!(self.act, Act::Leap | Act::Land | Act::Walk) {
                            self.start(Act::Sit, 30.0, 0.4);
                        }
                    }
                }
                // Stroking: the pointer moving over it.
                if senses.over && !self.playing {
                    self.stroked = (self.stroked + moved.min(30.0)).min(160.0);
                    if moved > 0.5 {
                        self.unstroked = 0.0;
                    }
                }
                self.unstroked += dt;
                if !senses.over || self.unstroked > 0.4 {
                    self.stroked = (self.stroked - 60.0 * dt).max(0.0);
                }
                let strokable = matches!(
                    self.act,
                    Act::Sit | Act::Stand | Act::Groom | Act::Loaf | Act::Sleep
                );
                if self.act == Act::Petted {
                    if self.unstroked > if senses.over { 2.5 } else { 0.9 } {
                        self.stroked = 0.0;
                        let (act, length) = if self.lying {
                            (Act::Loaf, 10.0)
                        } else {
                            (Act::Sit, 5.0)
                        };
                        self.lying = false;
                        self.start(act, length, 0.6);
                    }
                } else if strokable && !self.playing && self.stroked > 55.0 {
                    self.lying = matches!(self.act, Act::Loaf | Act::Sleep);
                    self.next_sign = 0.3;
                    self.start(Act::Petted, 0.0, 0.5);
                } else if self.playing {
                    self.play(model, senses);
                } else {
                    self.idle(senses);
                }
            }
        }

        // Turn towards the way it means to face.
        // Turn towards the way it means to face: into the turn and out of it
        // gently, as a damped spring would.
        let steps = (dt / 0.004).ceil().max(1.0);
        for _ in 0..steps as usize {
            let step = dt / steps;
            self.spin += ((self.heading - self.facing) * 70.0 - self.spin * 16.0) * step;
            self.facing += self.spin * step;
        }

        let side = self.side();
        let mut pose = match self.act {
            Act::Stand => moves::stand(model),
            Act::Walk => {
                let gait = moves::gait(model, self.trot);
                // It steps on the spot while it turns round.
                let ahead = self.facing.cos();
                // It eases into its stride, and out of it as it nears where
                // it is going: shorter, slower steps.
                let left = (self.goal - self.x).abs() / SCALE;
                let want = if self.playing {
                    1.0
                } else {
                    (left / 14.0).clamp(0.3, 1.0)
                };
                self.pace += (want - self.pace) * (1.0 - (-dt * 4.0).exp());
                let rate = 0.55 + 0.45 * self.pace;
                self.phase += dt / gait.period * rate;
                self.x += gait.speed * self.pace * SCALE * ahead * dt;
                self.x = self.x.clamp(
                    EDGE.min(senses.width / 2.0),
                    (senses.width - EDGE).max(senses.width / 2.0),
                );
                moves::walk(model, self.trot, self.phase, self.pace / rate)
            }
            Act::Sit => moves::sit(model, side),
            Act::Groom => moves::groom(model, self.since, side),
            Act::Loaf => moves::loaf(model, side),
            Act::Sleep => moves::sleep(model, side),
            Act::Stretch => moves::stretch(model, self.since / self.length),
            Act::Stalk => moves::crouch(model, self.since),
            Act::Leap => {
                let u = (self.since / self.length).min(1.0);
                let (from, to, height) = self.leap;
                self.x = from + (to - from) * u;
                self.lift = 4.0 * height * u * (1.0 - u);
                if u >= 1.0 {
                    self.lift = 0.0;
                    self.start(Act::Land, 0.4, 0.16);
                }
                moves::leap(model, u)
            }
            Act::Land => moves::crouch(model, 0.0),
            Act::Bat => {
                // What it swats at, in its own space: across the window is
                // further ahead to a cat turned part way out of the screen.
                // Never behind its own shoulder: there the paw goes straight
                // up.
                let target = self.pointer_from_cat().filter(|_| self.reaching).map(|to| {
                    let before = if self.right { to.x } else { -to.x };
                    let ahead = (before / self.facing.cos().abs().max(0.5)).clamp(2.5, 22.0);
                    Vec2::new(self.middle.x + ahead, to.y + self.floor)
                });
                let right = side > 0.0;
                let pads = self
                    .paw
                    .step(&Reach::new(model, right), target, self.still, dt);
                if self.paw.hit {
                    self.thrill = 1.0;
                }
                moves::bat(model, right, pads)
            }
            Act::Petted => {
                if self.lying {
                    moves::loaf(model, side)
                } else {
                    moves::sit(model, side)
                }
            }
            Act::Held => moves::dangle(model, self.swing, self.time),
            Act::Fall => {
                let mut pose = moves::dangle(model, self.swing, self.time);
                pose.blend(&moves::leap(model, 0.85), smooth(self.since / 0.25));
                pose
            }
        };
        if let Some(from) = &self.from {
            let amount = smoother(self.since / self.blend.max(0.01));
            if amount < 1.0 {
                let mut blended = from.clone();
                blended.blend(&pose, amount);
                pose = blended;
            } else {
                self.from = None;
            }
        }
        self.last = Some(pose.clone());
        self.dress(model, pose, senses)
    }

    /// Add what lives on top of a move: breath, eyes, ears, tail, and where
    /// the cat is looking.
    fn dress(&mut self, model: &Model, pose: Pose, senses: &Senses) -> Pose {
        let dt = senses.dt;
        let mut p = Puppet::new(model, pose);
        let b = &model.bones;
        let asleep = self.act == Act::Sleep && self.since > 1.0;

        // Breath: slow and deep asleep.
        let rate = if asleep { 1.6 } else { 2.6 };
        p.breathe(0.5 + 0.5 * (self.time * rate).sin() * if asleep { 1.6 } else { 0.8 });

        // Even still, it is never quite still: its weight shifts.
        if matches!(
            self.act,
            Act::Stand | Act::Sit | Act::Loaf | Act::Bat | Act::Petted
        ) {
            let t = self.time;
            p.yaw(
                b.spine2,
                1.3 * (t * 0.7).sin() + 0.6 * (t * 1.9 + 1.0).sin(),
            );
            p.roll(b.chest, 0.9 * (t * 0.5 + 2.0).sin());
            p.roll(b.head, 1.2 * (t * 0.9 + 0.5).sin());
            p.pitch(b.neck2, 0.8 * (t * 1.3).sin());
        }

        // The tail's own life.
        let (sway, speed) = match self.act {
            Act::Stand | Act::Sit | Act::Groom => (7.0, 1.6),
            Act::Loaf => (4.0, 1.0),
            Act::Sleep => (1.0, 0.5),
            Act::Stalk => (9.0, 9.0),
            // Its tail lashes when it has caught something.
            Act::Bat => (13.0 + 12.0 * self.thrill, 5.0),
            Act::Petted => (8.0, 2.4),
            Act::Held | Act::Fall => (4.0, 3.0),
            _ => (0.0, 1.0),
        };
        p.swish(sway, self.time * speed);
        self.thrill *= (-dt * 4.0).exp();

        // A yawn.
        let mut gape = 0.0;
        if let Some(yawn) = &mut self.yawn {
            *yawn += dt;
            gape = bell(*yawn / 2.2, 0.35);
            if *yawn > 2.2 {
                self.yawn = None;
            }
            p.pitch(b.head, 12.0 * gape);
            p.gape(gape);
        }

        // Eyes and ears ease to what the act asks of them.
        let (eyes, ears) = match self.act {
            Act::Sleep => (1.0, 0.25),
            Act::Petted => (1.0, 0.15),
            Act::Loaf => (0.3, 0.0),
            Act::Groom => (0.55, 0.1),
            Act::Stretch => (0.85 * bell(self.since / self.length, 0.3), 0.5),
            Act::Stalk | Act::Bat | Act::Leap => (0.0, -0.8),
            Act::Held | Act::Fall => (0.0, 0.9),
            _ if self.playing => (0.0, -0.5),
            _ => (0.0, 0.0),
        };
        let ease = 1.0 - (-dt * 7.0).exp();
        self.eyes += (eyes - self.eyes) * ease;
        self.ears += (ears - self.ears) * ease;
        self.next_blink -= dt;
        if self.next_blink < -0.16 {
            self.next_blink = self.between(2.0, 6.5);
        }
        let blink = if self.next_blink < 0.0 { 1.0 } else { 0.0 };
        p.close_eyes(self.eyes.max(blink).max(gape));
        p.ears(self.ears + 0.6 * gape);
        self.next_flick -= dt;
        if self.next_flick < -0.24 {
            self.next_flick = self.between(3.0, 11.0);
            self.flick_left = self.chance(0.5);
        }
        if self.next_flick < 0.0 {
            p.flick(self.flick_left, (-self.next_flick / 0.24 * PI).sin());
        }

        // Where it looks: at the pointer when that has its interest, and
        // about itself otherwise.
        self.next_glance -= dt;
        if self.next_glance < 0.0 {
            self.next_glance = self.between(1.2, 4.5);
            self.glance = if self.chance(0.3) {
                Vec2::ZERO
            } else {
                Vec2::new(self.between(-55.0, 55.0), self.between(-8.0, 16.0))
            };
        }
        let near = self.pointer_from_cat().filter(|to| {
            self.playing || self.act == Act::Petted || (self.still < 1.5 && to.length() < 110.0)
        });
        let looks = matches!(
            self.act,
            Act::Stand
                | Act::Sit
                | Act::Loaf
                | Act::Bat
                | Act::Stalk
                | Act::Petted
                | Act::Walk
                | Act::Land
        );
        let want = match near {
            Some(to) if looks => {
                let mut want = p.gaze_at(self.in_model(to, 16.0));
                if self.act == Act::Petted {
                    // It leans its head into the hand.
                    want = Vec2::new(want.x * 0.5, want.y * 0.4 + 10.0);
                }
                if self.act == Act::Stalk {
                    // Low to the ground, it only peers up.
                    want.y = want.y.min(22.0);
                }
                want
            }
            _ if matches!(self.act, Act::Stand | Act::Sit | Act::Loaf) && self.yawn.is_none() => {
                self.glance
            }
            _ => Vec2::ZERO,
        };
        let quick = if near.is_some() { 9.0 } else { 3.5 };
        self.gaze += (want - self.gaze) * (1.0 - (-dt * quick).exp());
        p.look(self.gaze.x, self.gaze.y);

        // Purring: it trembles, and hearts rise.
        if self.act == Act::Petted {
            p.shift(Vec2::new(0.0, 0.05 * (self.time * 95.0).sin()));
            p.roll(b.head, 7.0 * (self.time * 1.3).sin());
        }
        self.next_sign -= dt;
        if self.next_sign < 0.0 {
            match self.act {
                Act::Petted => {
                    self.next_sign = self.between(0.5, 0.9);
                    self.sign(Mark::Heart);
                }
                Act::Sleep if asleep => {
                    self.next_sign = 1.7;
                    self.sign(Mark::Z);
                }
                _ => {}
            }
        }
        for sign in &mut self.signs {
            sign.age += dt;
            sign.at += sign.drift * dt;
        }
        self.signs.retain(|sign| sign.age < sign.life);
        p.pose
    }

    /// How many times a second the cat needs drawing.
    pub fn frame_rate(&self) -> f32 {
        match self.act {
            Act::Sleep => 20.0,
            Act::Sit | Act::Loaf if !self.playing => 40.0,
            Act::Leap | Act::Fall | Act::Held | Act::Bat => 60.0,
            _ => 60.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
    use std::collections::HashSet;

    const WIDTH: f32 = 1400.0;
    const STEP: f32 = 1.0 / 20.0;

    fn model() -> Model {
        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<Assets<SkinnedMeshInverseBindposes>>();
        Model::load(&mut world).unwrap()
    }

    /// A cat sitting in the middle of the window, facing right.
    fn sitting() -> Mind {
        let mut mind = Mind::new(7);
        mind.log = false;
        mind.x = WIDTH / 2.0;
        mind.right = true;
        mind.act = Act::Sit;
        mind.length = 30.0;
        mind.turn(0.6);
        mind.facing = mind.heading;
        mind
    }

    fn senses(pointer: Option<Vec2>) -> Senses {
        Senses {
            dt: STEP,
            width: WIDTH,
            pointer,
            ..default()
        }
    }

    /// Live a step, and check the cat is whole and where it may be.
    fn step(mind: &mut Mind, model: &Model, senses: Senses) -> Act {
        let pose = mind.think(model, &senses);
        for (node, local) in pose.0.iter().enumerate() {
            assert!(
                local.translation.is_finite()
                    && local.rotation.is_finite()
                    && local.scale.is_finite(),
                "{:?} bends {} out of shape",
                mind.act,
                model.nodes[node].name
            );
        }
        assert!(
            mind.lift >= 0.0 && mind.lift.is_finite(),
            "{:?} at {}",
            mind.act,
            mind.lift
        );
        if mind.act != Act::Held {
            assert!(
                (EDGE..=WIDTH - EDGE).contains(&mind.x),
                "{:?} at {}",
                mind.act,
                mind.x
            );
        }
        mind.act
    }

    #[test]
    fn left_alone_it_wanders_the_window_and_does_everything_a_cat_does() {
        let model = model();
        let mut mind = sitting();
        let mut acts = HashSet::new();
        let mut places = HashSet::new();
        for _ in 0..(30.0 * 60.0 / STEP) as usize {
            acts.insert(step(&mut mind, &model, senses(None)) as u8);
            places.insert((mind.x / 100.0) as i32);
            assert_eq!(mind.lift, 0.0);
        }
        for act in [
            Act::Stand,
            Act::Walk,
            Act::Sit,
            Act::Groom,
            Act::Loaf,
            Act::Sleep,
            Act::Stretch,
        ] {
            assert!(acts.contains(&(act as u8)), "it never did {act:?}");
        }
        assert!(places.len() >= 6, "it kept to {places:?}");
    }

    #[test]
    fn clicked_it_plays_with_the_pointer_until_the_pointer_stops() {
        let model = model();
        let mut mind = sitting();
        step(
            &mut mind,
            &model,
            Senses {
                clicked: true,
                ..senses(None)
            },
        );
        assert!(mind.playing);
        let mut acts = HashSet::new();
        for tick in 0..(90.0 / STEP) as usize {
            // The pointer goes from place to place: near and low, far away,
            // and overhead.
            let time = tick as f32 * STEP;
            let wobble = Vec2::new((time * 5.0).sin(), (time * 3.0).cos()) * 3.0;
            let place = match (time / 6.0) as usize % 3 {
                0 => Vec2::new(mind.x + 13.0 * SCALE, 17.0 * SCALE),
                1 => Vec2::new(
                    if (time / 18.0) as usize % 2 == 0 {
                        120.0
                    } else {
                        WIDTH - 120.0
                    },
                    13.0 * SCALE,
                ),
                _ => Vec2::new(mind.x + 22.0 * SCALE, 46.0 * SCALE),
            };
            acts.insert(step(&mut mind, &model, senses(Some(place + wobble))) as u8);
            assert!(mind.playing);
        }
        for act in [Act::Bat, Act::Walk, Act::Stalk, Act::Leap, Act::Land] {
            assert!(acts.contains(&(act as u8)), "it never did {act:?}");
        }
        // A pointer that stays put is no fun.
        for _ in 0..(12.0 / STEP) as usize {
            step(&mut mind, &model, senses(Some(Vec2::new(300.0, 200.0))));
        }
        assert!(!mind.playing);
    }

    #[test]
    fn a_pointer_passing_overhead_is_still_swatted_at() {
        let model = model();
        let mut mind = sitting();
        step(
            &mut mind,
            &model,
            Senses {
                clicked: true,
                ..senses(None)
            },
        );
        // In front of it, in reach: it starts to swat.
        let at = |ahead: f32| Vec2::new(WIDTH / 2.0 + ahead * SCALE, 24.0 * SCALE);
        for _ in 0..40 {
            step(&mut mind, &model, senses(Some(at(12.0))));
        }
        assert_eq!(mind.act, Act::Bat);
        // Then over its head to a little behind it, and back.
        for tick in 0..120 {
            let ahead = 12.0 - 16.0 * (tick as f32 / 120.0 * PI).sin();
            let act = step(&mut mind, &model, senses(Some(at(ahead))));
            assert_eq!(act, Act::Bat, "with the pointer {ahead} ahead");
        }
    }

    #[test]
    fn a_pointer_on_the_cat_itself_is_watched_not_swatted() {
        let model = model();
        let mut mind = sitting();
        step(
            &mut mind,
            &model,
            Senses {
                clicked: true,
                ..senses(None)
            },
        );
        // On its chest, then its head, then its rump.
        for (ahead, up) in [(3.0, 10.0), (4.0, 18.0), (-6.0, 6.0)] {
            let on = Vec2::new(WIDTH / 2.0 + ahead * SCALE, up * SCALE);
            for tick in 0..80 {
                let wobble = Vec2::new((tick as f32 * 0.4).sin(), 0.0);
                let act = step(
                    &mut mind,
                    &model,
                    Senses {
                        over: true,
                        ..senses(Some(on + wobble))
                    },
                );
                assert_eq!(act, Act::Sit, "with the pointer {ahead} ahead, {up} up");
            }
        }
    }

    #[test]
    fn a_second_click_ends_the_game() {
        let model = model();
        let mut mind = sitting();
        step(
            &mut mind,
            &model,
            Senses {
                clicked: true,
                ..senses(None)
            },
        );
        step(
            &mut mind,
            &model,
            Senses {
                clicked: true,
                ..senses(None)
            },
        );
        assert!(!mind.playing);
    }

    #[test]
    fn stroked_it_purrs_until_the_hand_goes() {
        let model = model();
        let mut mind = sitting();
        let over = |tick: usize| Senses {
            over: true,
            ..senses(Some(Vec2::new(
                WIDTH / 2.0 + (tick % 2) as f32 * 12.0,
                20.0,
            )))
        };
        for tick in 0..40 {
            step(&mut mind, &model, over(tick));
        }
        assert_eq!(mind.act, Act::Petted);
        assert!(mind.signs.iter().any(|sign| sign.mark == Mark::Heart));
        for _ in 0..40 {
            step(&mut mind, &model, senses(Some(Vec2::new(40.0, 300.0))));
        }
        assert_eq!(mind.act, Act::Sit);
    }

    #[test]
    fn dragged_it_is_carried_and_dropped_it_lands_on_its_feet() {
        let model = model();
        let mut mind = sitting();
        for tick in 0..30 {
            let pointer = Vec2::new(WIDTH / 2.0 + tick as f32 * 8.0, 20.0 + tick as f32 * 9.0);
            step(
                &mut mind,
                &model,
                Senses {
                    held: true,
                    ..senses(Some(pointer))
                },
            );
        }
        assert_eq!(mind.act, Act::Held);
        assert!(mind.lift > 100.0);
        assert!((mind.x - (WIDTH / 2.0 + 29.0 * 8.0)).abs() < 1.0);
        let mut acts = Vec::new();
        for _ in 0..60 {
            let act = step(&mut mind, &model, senses(None));
            if acts.last() != Some(&act) {
                acts.push(act);
            }
        }
        assert_eq!(acts[..3], [Act::Fall, Act::Land, Act::Stand]);
        assert_eq!(mind.lift, 0.0);
    }
}
