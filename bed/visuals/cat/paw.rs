//! The paw a sitting cat swats with.
//!
//! Nothing here is a drawn-out animation: the paw is a weight on a spring,
//! and a swat is the cat changing where the spring pulls and how hard. It
//! draws the paw back, lets it go at the pointer, holds it there a moment
//! and takes it back. How the paw gets from one to the next (how fast, how
//! far past, how it settles) is the spring's doing, so a swat at a pointer
//! that moves follows it, and no two are quite alike.
use super::moves::Reach;
use bevy::prelude::*;

/// A spring: how quick it is (as a turning rate, per second) and how much
/// of the damping that would stop it dead without overshooting.
#[derive(Clone, Copy)]
struct Pull {
    quick: f32,
    damping: f32,
}

/// Easing the paw down, or up to where it waits.
const SOFT: Pull = Pull {
    quick: 13.0,
    damping: 0.9,
};
/// Drawing it back.
const DRAW: Pull = Pull {
    quick: 26.0,
    damping: 0.8,
};
/// Keeping it on what it has caught.
const PIN: Pull = Pull {
    quick: 24.0,
    damping: 0.7,
};

/// How far a paw may swing, in degrees from where it rests: a little behind
/// that, and up over the cat's head.
const SWING: [f32; 2] = [-8.0, 215.0];
/// The fastest a leg swings, in degrees a second.
const FASTEST: f32 = 1000.0;
/// How near (in degrees of swing, and length) counts as on the pointer.
const ON: Vec2 = Vec2::new(7.0, 0.9);
/// How far off it (the same) the pointer has got away from a paw holding it.
const AWAY: Vec2 = Vec2::new(25.0, 2.5);
/// A pointer moving faster than this (the cat's units a second) is swatted
/// at in a hurry.
const BRISK: f32 = 25.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Stage {
    /// Down, or hovering near the pointer, until the next swat.
    Wait,
    /// Drawn back.
    Wind,
    /// Let go at the pointer.
    Strike,
    /// On the pointer.
    Hold,
}

pub struct Paw {
    seed: u64,
    /// Where the pads are, as `Reach` tells it (degrees of swing, and how
    /// far out), and how fast that is changing.
    at: Vec2,
    speed: Vec2,
    pub stage: Stage,
    /// Seconds of the stage left.
    left: f32,
    /// This swat: how far back from the pointer the paw is drawn (degrees),
    /// how hard it is let go, and how far short of the pointer it is aimed.
    draw: f32,
    strike: Pull,
    short: f32,
    /// Where the pointer was, and how fast it is going.
    target: Option<Vec2>,
    chase: Vec2,
    /// The paw landed on the pointer this step.
    pub hit: bool,
    /// The paw is down, and still.
    pub home: bool,
}

impl Paw {
    pub fn new(seed: u64) -> Self {
        Self {
            seed: seed | 1,
            at: Vec2::ZERO,
            speed: Vec2::ZERO,
            stage: Stage::Wait,
            left: 0.0,
            draw: 0.0,
            strike: SOFT,
            short: 0.0,
            target: None,
            chase: Vec2::ZERO,
            hit: false,
            home: true,
        }
    }

    /// Put the paw down, to swat in `wait` seconds.
    pub fn rest(&mut self, reach: &Reach, wait: f32) {
        self.at = reach.home();
        self.speed = Vec2::ZERO;
        self.stage = Stage::Wait;
        self.left = wait;
        self.target = None;
        self.chase = Vec2::ZERO;
        self.hit = false;
        self.home = true;
    }

    fn between(&mut self, low: f32, high: f32) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        low + (high - low) * ((self.seed >> 40) as f32 / (1u64 << 24) as f32)
    }

    /// Draw the paw back for a swat at `want`. `pat` is a second, smaller
    /// one, from on the pointer; `idle` is how long the pointer has been
    /// still.
    fn wind(&mut self, want: Vec2, pat: bool, idle: f32) {
        let brisk = self.chase.length() > BRISK;
        // Back means up, for a pointer out in front; one overhead is swung
        // up at from in front.
        let back = if want.x < 130.0 { 1.0 } else { -1.0 };
        self.draw = back * self.between(20.0, 36.0) * if pat { 0.45 } else { 1.0 };
        self.left = self.between(0.09, 0.17) * if pat || brisk { 0.6 } else { 1.0 };
        // A pointer that has stopped is prodded at, to see if it will move.
        if !pat && !brisk && idle > 0.5 && self.between(0.0, 1.0) < 0.5 {
            self.strike = Pull {
                quick: self.between(17.0, 23.0),
                damping: 0.9,
            };
            self.short = self.between(0.4, 1.4);
            self.left *= 1.6;
        } else {
            self.strike = Pull {
                quick: self.between(34.0, 46.0),
                damping: self.between(0.5, 0.65),
            };
            self.short = 0.0;
        }
        self.stage = Stage::Wind;
    }

    /// Live through `dt` seconds with the pointer at `target` (in the cat's
    /// space, seen from the side; none when there is nothing to swat at),
    /// still for `idle` seconds. Says where the pads are, as `moves::bat`
    /// takes them.
    pub fn step(&mut self, reach: &Reach, target: Option<Vec2>, idle: f32, dt: f32) -> Vec2 {
        self.hit = false;
        let chase = match (target, self.target) {
            (Some(now), Some(before)) if dt > 0.0 => (now - before) / dt,
            _ => Vec2::ZERO,
        };
        self.chase += (chase - self.chase) * (1.0 - (-dt * 12.0).exp());
        self.target = target;
        self.left -= dt;
        // It swats at where the pointer is going.
        let want = target.map(|target| {
            let want = reach.swing(target + self.chase * 0.04);
            Vec2::new(want.x.clamp(SWING[0], SWING[1]), want.y)
        });
        let off = want.map_or(Vec2::ZERO, |want| (want - self.at).abs());
        match (self.stage, want) {
            (_, None) => {
                // Nothing to swat at: down it goes, and not straight back up.
                self.stage = Stage::Wait;
                self.left = self.left.max(0.1);
            }
            (Stage::Wait, Some(want)) if self.left <= 0.0 => self.wind(want, false, idle),
            (Stage::Wind, Some(_)) if self.left <= 0.0 => {
                self.stage = Stage::Strike;
                self.left = 0.3;
            }
            (Stage::Strike, Some(_)) if off.x < ON.x && off.y < ON.y + self.short => {
                self.hit = self.short == 0.0;
                self.stage = Stage::Hold;
                self.left = self.between(0.06, 0.4);
            }
            (Stage::Strike, Some(_)) if self.left <= 0.0 => {
                // Missed.
                self.stage = Stage::Wait;
                self.left = self.between(0.1, 0.35);
            }
            // It got away: after it.
            (Stage::Hold, Some(want)) if off.x > AWAY.x || off.y > AWAY.y => {
                self.wind(want, true, 0.0)
            }
            (Stage::Hold, Some(want)) if self.left <= 0.0 => {
                if self.between(0.0, 1.0) < 0.4 {
                    self.wind(want, true, 0.0);
                } else {
                    self.stage = Stage::Wait;
                    let hurry = if self.chase.length() > BRISK {
                        0.4
                    } else {
                        1.0
                    };
                    self.left = self.between(0.15, 0.7) * hurry;
                }
            }
            _ => {}
        }
        let (goal, pull) = match (self.stage, want) {
            (_, None) => (reach.home(), SOFT),
            // Waiting, it holds the paw up off the ground, part of the way
            // to the pointer and bent at the elbow.
            (Stage::Wait, Some(want)) => (Vec2::new(want.x * 0.35, reach.rest - 1.9), SOFT),
            (Stage::Wind, Some(want)) => {
                let length = reach.near + (want.y - reach.near) * 0.45;
                (Vec2::new(want.x + self.draw, length), DRAW)
            }
            (Stage::Strike, Some(want)) => (Vec2::new(want.x, want.y - self.short), self.strike),
            (Stage::Hold, Some(want)) => (want, PIN),
        };
        let goal = Vec2::new(
            goal.x.clamp(SWING[0], SWING[1]),
            goal.y.clamp(reach.near, reach.far),
        );
        // In small steps: a stiff spring taken a frame at a time flies apart.
        let steps = (dt / 0.004).ceil().max(1.0);
        let step = dt / steps;
        for _ in 0..steps as usize {
            self.speed += ((goal - self.at) * pull.quick * pull.quick
                - self.speed * 2.0 * pull.damping * pull.quick)
                * step;
            self.speed.x = self.speed.x.clamp(-FASTEST, FASTEST);
            self.at += self.speed * step;
            // The leg is only so long, and only swings so far.
            for (axis, low, high) in [(0, SWING[0], SWING[1]), (1, reach.near, reach.far)] {
                if self.at[axis] < low || self.at[axis] > high {
                    self.at[axis] = self.at[axis].clamp(low, high);
                    self.speed[axis] = 0.0;
                }
            }
        }
        let from_home = (self.at - reach.home()).abs();
        self.home =
            want.is_none() && from_home.x < 5.0 && from_home.y < 0.5 && self.speed.x.abs() < 40.0;
        reach.point(self.at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visuals::cat::rig::Model;
    use bevy::mesh::skinning::SkinnedMeshInverseBindposes;

    const STEP: f32 = 1.0 / 60.0;

    fn model() -> Model {
        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<StandardMaterial>>();
        world.init_resource::<Assets<SkinnedMeshInverseBindposes>>();
        Model::load(&mut world).unwrap()
    }

    /// Swat at `target` for `seconds`: each frame's stage and where the pads
    /// were.
    fn swat(
        paw: &mut Paw,
        reach: &Reach,
        target: impl Fn(f32) -> Option<Vec2>,
        seconds: f32,
    ) -> Vec<(Stage, Vec2)> {
        (0..(seconds / STEP) as usize)
            .map(|frame| {
                let at = paw.step(reach, target(frame as f32 * STEP), 0.0, STEP);
                assert!(at.is_finite());
                (paw.stage, at)
            })
            .collect()
    }

    #[test]
    fn a_swat_lands_on_the_pointer_and_comes_back() {
        let model = model();
        let reach = Reach::new(&model, true);
        // Out in front and low, out in front and high, and overhead.
        for target in [
            Vec2::new(15.0, 5.0),
            Vec2::new(14.0, 18.0),
            Vec2::new(8.0, 23.0),
            Vec2::new(2.0, 22.0),
        ] {
            for seed in 1..40 {
                let mut paw = Paw::new(seed);
                paw.rest(&reach, 0.0);
                let path = swat(&mut paw, &reach, |_| Some(target), 3.0);
                let landed = path.iter().position(|(stage, _)| *stage == Stage::Hold);
                let landed = landed.unwrap_or_else(|| panic!("never landed on {target}"));
                assert!(
                    landed as f32 * STEP < 0.6,
                    "took {landed} frames to land on {target}"
                );
                let near = path[landed].1.distance(reach.point(reach.swing(target)));
                assert!(near < 2.5, "landed {near} from {target}");
                // No frame is a jump, and the paw stays in front of the cat.
                for pair in path.windows(2) {
                    let moved = pair[0].1.distance(pair[1].1);
                    assert!(moved < 4.5, "{moved} in a frame, swatting at {target}");
                    assert!(
                        pair[1].1.x > reach.point(reach.home()).x - 3.5,
                        "at {} swatting at {target}",
                        pair[1].1
                    );
                }
                // The pointer gone, it puts its paw down.
                swat(&mut paw, &reach, |_| None, 1.2);
                assert!(paw.home, "still up after {target}");
            }
        }
    }

    #[test]
    fn a_swat_follows_a_pointer_that_moves() {
        let model = model();
        let reach = Reach::new(&model, true);
        let mut paw = Paw::new(3);
        paw.rest(&reach, 0.0);
        // The pointer goes up and down in front of the cat.
        let target = |time: f32| Some(Vec2::new(15.0, 12.0 + 8.0 * (time * 3.0).sin()));
        let path = swat(&mut paw, &reach, target, 6.0);
        let held: Vec<f32> = path
            .iter()
            .enumerate()
            .filter(|(_, (stage, _))| *stage == Stage::Hold)
            .map(|(frame, (_, at))| {
                at.distance(reach.point(reach.swing(target(frame as f32 * STEP).unwrap())))
            })
            .collect();
        assert!(
            held.len() > 20,
            "it was on the pointer for {} frames",
            held.len()
        );
        let far = held.iter().copied().fold(0.0, f32::max);
        assert!(far < 4.0, "holding it, the paw was {far} from the pointer");
    }

    /// `cargo test -p bed paw::tests::trace -- --nocapture --ignored` prints
    /// a swat frame by frame.
    #[test]
    #[ignore]
    fn trace() {
        let model = model();
        let reach = Reach::new(&model, true);
        let mut paw = Paw::new(5);
        paw.rest(&reach, 0.0);
        let target = Vec2::new(15.0, 14.0);
        println!("target {target}: swing {}", reach.swing(target));
        for (frame, (stage, at)) in swat(&mut paw, &reach, |_| Some(target), 1.6)
            .iter()
            .enumerate()
        {
            println!(
                "{:5.2}s {stage:?} swing {:6.1} length {:5.2} at {:5.1} {:5.1}",
                frame as f32 * STEP,
                paw_at(&reach, *at).x,
                paw_at(&reach, *at).y,
                at.x,
                at.y
            );
        }
    }

    fn paw_at(reach: &Reach, point: Vec2) -> Vec2 {
        reach.swing(point)
    }
}
