//! Dog behavior: decides what to do next and updates position and animation.
//! Platform-independent; coordinates are physical screen pixels and `y` is the line the feet stand on.

use crate::render::{FRAMES, WALK_STRIDE};

/// Walking speed, in sprite pixels per second.
const WALK_SPEED: f32 = 56.0;
const GRAVITY: f32 = 2500.0;
const MIN_WALK: f32 = 76.0;

/// Animations in the sprite atlas; the order matches the rows of `assets/dog.png`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anim {
    Idle,
    Walk,
    Sit,
    Bark,
    Happy,
    Sleep,
    Held,
}

impl Anim {
    pub const ALL: [Anim; 7] = [
        Anim::Idle,
        Anim::Walk,
        Anim::Sit,
        Anim::Bark,
        Anim::Happy,
        Anim::Sleep,
        Anim::Held,
    ];

    pub fn row(self) -> u32 {
        self as u32
    }

    /// Duration of each frame (seconds). `Walk` advances by distance travelled, not by time.
    fn durations(self) -> &'static [f32] {
        match self {
            Anim::Idle => &[0.9, 0.3, 0.3, 0.15],
            Anim::Walk => &[0.0; 8],
            Anim::Sit => &[1.4, 0.35, 0.35, 0.15],
            Anim::Bark => &[0.2, 0.12, 0.22, 0.14],
            Anim::Happy => &[0.14, 0.11, 0.13, 0.14],
            Anim::Sleep => &[0.8, 0.7, 0.8, 0.7],
            Anim::Held => &[0.28, 0.2, 0.28, 0.2],
        }
    }

    pub fn frames(self) -> usize {
        FRAMES[self as usize]
    }

    /// The frame at second `t` of the loop.
    pub fn frame_at(self, t: f32) -> usize {
        let d = self.durations();
        let total: f32 = d.iter().sum();
        if total <= 0.0 {
            return 0;
        }
        let mut t = t.rem_euclid(total);
        for (i, len) in d.iter().enumerate() {
            if t < *len {
                return i;
            }
            t -= len;
        }
        d.len() - 1
    }
}

/// Snapshot of the state needed for drawing.
#[derive(Clone, Copy, Debug)]
pub struct DogFrame {
    pub anim: Anim,
    pub frame: usize,
    /// 1 = facing right, -1 = facing left.
    pub facing: f32,
    pub time: f32,
}

impl DogFrame {
    pub fn still(anim: Anim, frame: usize) -> Self {
        DogFrame {
            anim,
            frame,
            facing: 1.0,
            time: 0.0,
        }
    }
}

/// The area the dog can roam in (physical pixels).
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub ground: f32,
    pub left: f32,
    pub right: f32,
    /// Physical pixels per sprite pixel.
    pub scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Activity {
    Stand,
    /// `origin`: the x where the walk started. The dog advances exactly one stride (`WALK_STRIDE`)
    /// per frame, so the planted paw stays fixed on screen and doesn't slide.
    Walk {
        origin: f32,
        target: f32,
    },
    Sit,
    Sleep,
    Alert,
    Happy,
    Held,
    Falling,
}

pub struct Dog {
    pub x: f32,
    pub y: f32,
    pub facing: f32,
    /// While a notification is open the dog sits in place.
    pub hold: bool,
    /// Walking speed multiplier (settings).
    pub speed: f32,
    /// false: the dog doesn't roam; it stands, sits or sleeps in place (settings).
    pub wander: bool,
    activity: Activity,
    timer: f32,
    /// Time since the current activity started.
    anim_time: f32,
    walk_dist: f32,
    time: f32,
    vy: f32,
    forced_sleep: bool,
    rng: fastrand::Rng,
}

impl Dog {
    pub fn new(x: f32, y: f32) -> Self {
        Dog {
            x,
            y,
            facing: 1.0,
            hold: false,
            speed: 1.0,
            wander: true,
            activity: Activity::Stand,
            timer: 1.5,
            anim_time: 0.0,
            walk_dist: 0.0,
            time: 0.0,
            vy: 0.0,
            forced_sleep: false,
            rng: fastrand::Rng::new(),
        }
    }

    pub fn update(&mut self, dt: f32, b: &Bounds) {
        self.time += dt;
        self.anim_time += dt;

        match self.activity {
            Activity::Held => {}
            Activity::Falling => {
                self.vy += GRAVITY * b.scale * dt;
                self.y += self.vy * dt;
                self.y = self.y.round();
                if self.y >= b.ground {
                    self.y = b.ground;
                    self.vy = 0.0;
                    self.set(Activity::Stand, 0.8);
                }
            }
            Activity::Walk { origin, target } => {
                self.walk_dist += WALK_SPEED * self.speed * dt;
                let travelled = self.steps() as f32 * WALK_STRIDE * b.scale;
                let total = (target - origin).abs();
                self.x = origin + self.facing * travelled.min(total);
                if travelled >= total {
                    self.next(b);
                }
            }
            _ => {
                self.timer -= dt;
                if self.timer <= 0.0 {
                    self.next(b);
                }
            }
        }

        if !matches!(self.activity, Activity::Held | Activity::Falling) {
            if self.y < b.ground - 1.0 {
                // The ground moved down (settings or taskbar changed): fall.
                self.vy = 0.0;
                self.set(Activity::Falling, 0.0);
                return;
            }
            self.y = b.ground;
            let x = self.x.clamp(b.left, b.right.max(b.left));
            if x != self.x {
                // The screen changed (resolution, taskbar): stop walking.
                self.x = x.round();
                if matches!(self.activity, Activity::Walk { .. }) {
                    self.set(Activity::Stand, 1.0);
                }
            }
        }
    }

    pub fn frame(&self) -> DogFrame {
        let anim = match self.activity {
            Activity::Stand => Anim::Idle,
            Activity::Walk { .. } => Anim::Walk,
            Activity::Sit => Anim::Sit,
            Activity::Sleep => Anim::Sleep,
            Activity::Alert => Anim::Bark,
            Activity::Happy => Anim::Happy,
            Activity::Held | Activity::Falling => Anim::Held,
        };
        let frame = match self.activity {
            Activity::Walk { .. } => self.steps() % anim.frames(),
            Activity::Falling => 0,
            _ => anim.frame_at(self.anim_time),
        };
        DogFrame {
            anim,
            frame,
            facing: self.facing,
            time: self.time,
        }
    }

    /// A new notification arrived: wake up, sit, bark.
    pub fn notify(&mut self) {
        if self.is_busy() {
            return;
        }
        self.forced_sleep = false;
        self.set(Activity::Alert, 2.4);
    }

    /// The dog was clicked.
    pub fn pet(&mut self) {
        if self.is_busy() {
            return;
        }
        self.forced_sleep = false;
        self.set(Activity::Happy, 2.0);
    }

    pub fn grab(&mut self) {
        self.set(Activity::Held, 0.0);
    }

    pub fn drag_to(&mut self, x: f32, y: f32) {
        if self.activity == Activity::Held {
            self.x = x.round();
            self.y = y.round();
        }
    }

    pub fn release(&mut self) {
        if self.activity == Activity::Held {
            self.vy = 0.0;
            self.set(Activity::Falling, 0.0);
        }
    }

    pub fn is_held(&self) -> bool {
        self.activity == Activity::Held
    }

    pub fn is_sleeping(&self) -> bool {
        self.forced_sleep || self.activity == Activity::Sleep
    }

    pub fn toggle_sleep(&mut self) {
        if self.is_busy() {
            return;
        }
        if self.is_sleeping() {
            self.forced_sleep = false;
            self.set(Activity::Stand, 1.5);
        } else {
            self.forced_sleep = true;
            self.set(Activity::Sleep, f32::INFINITY);
        }
    }

    fn is_busy(&self) -> bool {
        matches!(self.activity, Activity::Held | Activity::Falling)
    }

    fn steps(&self) -> usize {
        (self.walk_dist / WALK_STRIDE) as usize
    }

    fn set(&mut self, activity: Activity, timer: f32) {
        if self.activity != activity {
            self.anim_time = 0.0;
        }
        if let Activity::Walk { origin, target } = activity {
            self.walk_dist = 0.0;
            self.facing = (target - origin).signum();
        }
        self.activity = activity;
        self.timer = timer;
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + self.rng.f32() * (b - a)
    }

    fn next(&mut self, b: &Bounds) {
        if self.forced_sleep {
            return self.set(Activity::Sleep, f32::INFINITY);
        }
        if self.hold {
            return self.set(Activity::Sit, 1.0);
        }
        match self.activity {
            Activity::Sit if self.rng.f32() < 0.35 => {
                let t = self.range(30.0, 90.0);
                return self.set(Activity::Sleep, t);
            }
            Activity::Sleep => return self.set(Activity::Stand, 1.5),
            _ => {}
        }

        let r = self.rng.f32();
        if r < 0.5 && self.wander {
            for _ in 0..8 {
                let target = self.range(b.left, b.right).round();
                if (target - self.x).abs() >= MIN_WALK * b.scale {
                    let origin = self.x.round();
                    return self.set(Activity::Walk { origin, target }, 0.0);
                }
            }
            let t = self.range(2.0, 5.0);
            self.set(Activity::Stand, t);
        } else if r < 0.75 {
            let t = self.range(6.0, 18.0);
            self.set(Activity::Sit, t);
        } else {
            let t = self.range(2.0, 5.0);
            self.set(Activity::Stand, t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: Bounds = Bounds {
        ground: 1040.0,
        left: 40.0,
        right: 1880.0,
        scale: 2.0,
    };

    #[test]
    fn stays_on_ground_and_in_bounds() {
        let mut dog = Dog::new(500.0, B.ground);
        for _ in 0..(60 * 60 * 10) {
            dog.update(1.0 / 60.0, &B);
            assert_eq!(dog.y, B.ground);
            assert!(dog.x >= B.left && dog.x <= B.right);
            let f = dog.frame();
            assert!(f.frame < f.anim.frames());
        }
    }

    #[test]
    fn dropped_dog_falls_to_ground() {
        let mut dog = Dog::new(500.0, B.ground);
        dog.grab();
        dog.drag_to(900.0, 300.0);
        dog.update(1.0 / 60.0, &B);
        assert_eq!((dog.x, dog.y), (900.0, 300.0));
        assert_eq!(dog.frame().anim, Anim::Held);
        dog.release();
        for _ in 0..120 {
            dog.update(1.0 / 60.0, &B);
        }
        assert_eq!(dog.y, B.ground);
        assert!(!dog.is_held());
    }

    #[test]
    fn hold_keeps_dog_sitting() {
        let mut dog = Dog::new(500.0, B.ground);
        dog.notify();
        assert_eq!(dog.frame().anim, Anim::Bark);
        dog.hold = true;
        for _ in 0..(60 * 30) {
            dog.update(1.0 / 60.0, &B);
        }
        assert_eq!(dog.activity, Activity::Sit);
        assert_eq!(dog.x, 500.0);
    }

    #[test]
    fn forced_sleep_until_woken() {
        let mut dog = Dog::new(500.0, B.ground);
        dog.toggle_sleep();
        for _ in 0..(60 * 120) {
            dog.update(1.0 / 60.0, &B);
        }
        assert!(dog.is_sleeping());
        assert_eq!(dog.frame().anim, Anim::Sleep);
        dog.pet();
        assert!(!dog.is_sleeping());
    }

    #[test]
    fn walk_moves_in_whole_strides() {
        let mut dog = Dog::new(500.0, B.ground);
        // On reaching the target, sit instead of starting a new random walk.
        dog.hold = true;
        dog.set(
            Activity::Walk {
                origin: 500.0,
                target: 700.0,
            },
            0.0,
        );
        let mut last = (dog.x, dog.steps());
        for _ in 0..(60 * 10) {
            dog.update(1.0 / 60.0, &B);
            if !matches!(dog.activity, Activity::Walk { .. }) {
                break;
            }
            let now = (dog.x, dog.steps());
            // Position changes only when the frame changes, by exactly one stride.
            assert_eq!(now.0 != last.0, now.1 != last.1, "{last:?} -> {now:?}");
            if now.0 != last.0 {
                assert_eq!(now.0 - last.0, WALK_STRIDE * B.scale);
            }
            last = now;
        }
        assert_eq!(dog.x, 700.0);
    }

    #[test]
    fn no_wander_stays_put() {
        let mut dog = Dog::new(500.0, B.ground);
        dog.wander = false;
        for _ in 0..(60 * 600) {
            dog.update(1.0 / 60.0, &B);
            assert_eq!(dog.x, 500.0);
        }
    }

    #[test]
    fn falls_when_ground_drops_and_snaps_up() {
        let mut dog = Dog::new(500.0, 900.0);
        dog.update(1.0 / 60.0, &Bounds { ground: 900.0, ..B });
        dog.update(1.0 / 60.0, &B);
        assert!(dog.y < B.ground && dog.frame().anim == Anim::Held);
        for _ in 0..120 {
            dog.update(1.0 / 60.0, &B);
        }
        assert_eq!(dog.y, B.ground);
        dog.update(1.0 / 60.0, &Bounds { ground: 800.0, ..B });
        assert_eq!(dog.y, 800.0);
    }

    #[test]
    fn frame_at_cycles() {
        assert_eq!(Anim::Idle.frame_at(0.0), 0);
        assert_eq!(Anim::Idle.frame_at(0.95), 1);
        assert_eq!(Anim::Idle.frame_at(1.65 + 0.1), 0);
    }
}
