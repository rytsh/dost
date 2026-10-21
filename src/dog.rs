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
    Sad,
}

impl Anim {
    pub const ALL: [Anim; 8] = [
        Anim::Idle,
        Anim::Walk,
        Anim::Sit,
        Anim::Bark,
        Anim::Happy,
        Anim::Sleep,
        Anim::Held,
        Anim::Sad,
    ];

    pub fn row(self) -> u32 {
        self as u32
    }

    /// Index of the frame's cell, counting every frame of the rows above (atlas order).
    pub fn cell(self, frame: usize) -> usize {
        FRAMES[..self as usize].iter().sum::<usize>() + frame
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
            Anim::Sad => &[1.2, 0.9, 1.1, 0.2],
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
    /// The toy is in its mouth.
    pub carry: bool,
}

impl DogFrame {
    pub fn still(anim: Anim, frame: usize) -> Self {
        DogFrame {
            anim,
            frame,
            facing: 1.0,
            time: 0.0,
            carry: false,
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
    Sad,
}

/// Playing fetch: running to the toy, then bringing it back.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fetch {
    None,
    /// Running to `target` (where the mouth meets the toy); then back to `home`.
    Chasing {
        target: f32,
        home: f32,
    },
    /// Carrying the toy back to `home`.
    Returning {
        home: f32,
    },
}

/// How far in front of the feet point the mouth is (sprite pixels).
pub const MOUTH: f32 = 44.0;

/// Walking off screen (Hide) and back in (Show), past the right edge.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Visit {
    None,
    /// Walking in from the right edge to `target`.
    Entering {
        target: f32,
    },
    /// Walking off past the right edge.
    Leaving {
        target: f32,
    },
}

/// How far past the walkable area the dog goes off screen (sprite pixels).
const OFFSTAGE: f32 = 200.0;
/// Where the dog stops after walking back in, left of the walkable area's right end
/// (sprite pixels).
const VISIT_SPOT: f32 = 240.0;

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
    /// Runs left in the current zoomies.
    zoomies: u8,
    visit: Visit,
    /// Off screen past the right edge (hidden); nothing happens until it comes back.
    away: bool,
    /// Set when a walk off screen finishes; read once by [`Dog::take_left`].
    just_left: bool,
    fetch: Fetch,
    /// Physical pixels per sprite pixel at the last update.
    scale_hint: f32,
    /// The toy is in the mouth.
    carrying: bool,
    /// Picked the toy up; read once by [`Dog::take_picked`].
    picked: bool,
    /// Put the toy down at this x; read once by [`Dog::take_dropped`].
    dropped: Option<f32>,
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
            zoomies: 0,
            visit: Visit::None,
            away: false,
            just_left: false,
            fetch: Fetch::None,
            scale_hint: 1.0,
            carrying: false,
            picked: false,
            dropped: None,
            rng: fastrand::Rng::new(),
        }
    }

    pub fn update(&mut self, dt: f32, b: &Bounds) {
        self.scale_hint = b.scale;
        if self.away {
            return;
        }
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
                let boost = if self.zoomies > 0 {
                    3.5
                } else if let Fetch::Chasing { .. } = self.fetch {
                    3.0
                } else if matches!(self.visit, Visit::Entering { .. } | Visit::Leaving { .. })
                    || self.fetch != Fetch::None
                {
                    1.8
                } else {
                    1.0
                };
                self.walk_dist += WALK_SPEED * self.speed * boost * dt;
                let travelled = self.steps() as f32 * WALK_STRIDE * b.scale;
                let total = (target - origin).abs();
                self.x = origin + self.facing * travelled.min(total);
                if travelled >= total {
                    match self.visit {
                        Visit::Entering { .. } => self.visit = Visit::None,
                        Visit::Leaving { .. } => {
                            self.visit = Visit::None;
                            self.away = true;
                            self.just_left = true;
                            self.set(Activity::Stand, 1.0);
                            return;
                        }
                        _ => {}
                    }
                    if self.fetch_arrived(b) {
                        return;
                    }
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
            // Walking in or out passes the screen edge on purpose.
            if !matches!(self.visit, Visit::Entering { .. } | Visit::Leaving { .. }) {
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
    }

    /// Comes back on screen: walks in from the right edge if away, or turns back if leaving.
    pub fn enter(&mut self, b: &Bounds) {
        if matches!(self.activity, Activity::Held | Activity::Falling) {
            return;
        }
        match self.visit {
            Visit::None if !self.away => return,
            Visit::Entering { .. } => return,
            _ => {}
        }
        if self.away {
            self.away = false;
            self.x = (b.right + OFFSTAGE * b.scale).round();
            self.y = b.ground;
        }
        let target = (b.right - VISIT_SPOT * b.scale).max(b.left).round();
        self.visit = Visit::Entering { target };
        let origin = self.x.round();
        self.set(Activity::Walk { origin, target }, 0.0);
    }

    /// Walks off past the right edge; [`Dog::take_left`] reports when it is gone.
    pub fn leave(&mut self, b: &Bounds) {
        if self.away || matches!(self.visit, Visit::Leaving { .. }) {
            return;
        }
        self.zoomies = 0;
        self.drop_toy();
        let target = (b.right + OFFSTAGE * b.scale).round();
        self.visit = Visit::Leaving { target };
        if !matches!(self.activity, Activity::Held | Activity::Falling) {
            let origin = self.x.round();
            self.set(Activity::Walk { origin, target }, 0.0);
        }
    }

    /// True once after the dog has walked off screen.
    pub fn take_left(&mut self) -> bool {
        std::mem::take(&mut self.just_left)
    }

    /// Off screen (hidden and not visiting).
    /// Starts off screen, as if it had walked off (hidden at startup).
    pub fn start_away(&mut self) {
        self.away = true;
        self.visit = Visit::None;
        self.zoomies = 0;
    }

    pub fn is_away(&self) -> bool {
        self.away
    }

    pub fn is_leaving(&self) -> bool {
        matches!(self.visit, Visit::Leaving { .. })
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
            Activity::Sad => Anim::Sad,
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
            carry: self.carrying,
        }
    }

    /// Something failed: the dog sits and looks sad for a while.
    pub fn sad(&mut self) {
        if self.is_busy() || self.forced_sleep || self.is_performing() {
            return;
        }
        self.set(Activity::Sad, 4.5);
    }

    /// The toy landed at `x` (physical): the dog runs to it, picks it up and brings it back
    /// to where it stood. Does nothing while held, asleep or hidden.
    pub fn fetch(&mut self, x: f32, b: &Bounds) -> bool {
        if self.is_busy() || self.forced_sleep || self.zoomies > 0 {
            return false;
        }
        let home = match self.fetch {
            Fetch::Chasing { home, .. } | Fetch::Returning { home } => home,
            Fetch::None => self.x.clamp(b.left, b.right).round(),
        };
        // Stop where the mouth is over the toy, on whichever side the dog comes from.
        let dir = if x >= self.x { 1.0 } else { -1.0 };
        let target = (x - dir * MOUTH * b.scale).round();
        self.carrying = false;
        self.fetch = Fetch::Chasing { target, home };
        if (target - self.x).abs() < WALK_STRIDE * b.scale {
            self.facing = dir;
            self.fetch_arrived(b);
        } else {
            let origin = self.x.round();
            self.set(Activity::Walk { origin, target }, 0.0);
        }
        true
    }

    /// Playing fetch (running to the toy or bringing it back).
    pub fn is_fetching(&self) -> bool {
        self.fetch != Fetch::None
    }

    /// Stops playing (the toy was grabbed, or the dog was picked up).
    pub fn stop_fetch(&mut self) -> Option<f32> {
        self.fetch = Fetch::None;
        std::mem::take(&mut self.carrying).then(|| self.mouth_x())
    }

    /// The toy's x while carried: in front of the mouth (physical).
    pub fn mouth_x(&self) -> f32 {
        self.x + self.facing * MOUTH * self.scale_hint
    }

    pub fn is_carrying(&self) -> bool {
        self.carrying
    }

    /// True once after the toy was picked up.
    pub fn take_picked(&mut self) -> bool {
        std::mem::take(&mut self.picked)
    }

    /// Once, after the toy was put down: where (physical x).
    pub fn take_dropped(&mut self) -> Option<f32> {
        self.dropped.take()
    }

    /// A walk ended: the next step of fetch, if playing. Returns true if it took over.
    fn fetch_arrived(&mut self, b: &Bounds) -> bool {
        match self.fetch {
            Fetch::None => false,
            Fetch::Chasing { home, .. } => {
                self.carrying = true;
                self.picked = true;
                self.fetch = Fetch::Returning { home };
                let origin = self.x.round();
                if (home - origin).abs() < WALK_STRIDE * b.scale {
                    return self.fetch_arrived(b);
                }
                self.set(
                    Activity::Walk {
                        origin,
                        target: home,
                    },
                    0.0,
                );
                true
            }
            Fetch::Returning { .. } => {
                self.fetch = Fetch::None;
                self.carrying = false;
                self.dropped = Some(self.mouth_x());
                self.set(Activity::Happy, 1.6);
                true
            }
        }
    }

    /// A new notification arrived: sit up and bark. Sleeping (put to sleep) it only lifts its
    /// head; it stays asleep. Fetching, it keeps playing.
    pub fn notify(&mut self) {
        if self.is_busy() || self.forced_sleep || self.fetch != Fetch::None {
            return;
        }
        self.set(Activity::Alert, 2.4);
    }

    /// Runs back and forth fast a few times, then carries on.
    pub fn zoomies(&mut self, b: &Bounds) {
        if self.is_busy() {
            return;
        }
        self.forced_sleep = false;
        self.zoomies = 4;
        self.zoom(b);
    }

    fn zoom(&mut self, b: &Bounds) {
        let reach = 220.0 * b.scale;
        let dir = if self.zoomies.is_multiple_of(2) {
            1.0
        } else {
            -1.0
        };
        let target = (self.x + dir * reach).clamp(b.left, b.right).round();
        let target = if (target - self.x).abs() < MIN_WALK * b.scale {
            (self.x - dir * reach).clamp(b.left, b.right).round()
        } else {
            target
        };
        let origin = self.x.round();
        self.set(Activity::Walk { origin, target }, 0.0);
    }

    /// The dog was clicked.
    pub fn pet(&mut self) {
        if self.is_busy() || self.fetch != Fetch::None {
            return;
        }
        self.forced_sleep = false;
        self.set(Activity::Happy, 2.0);
    }

    pub fn grab(&mut self) {
        self.zoomies = 0;
        self.drop_toy();
        self.set(Activity::Held, 0.0);
    }

    /// Stops fetch; a carried toy falls where the mouth is.
    fn drop_toy(&mut self) {
        if let Some(x) = self.stop_fetch() {
            self.dropped = Some(x);
        }
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

    /// Put to sleep (Sleep button, `react: sleep`), not just napping: notifications stay quiet.
    pub fn is_put_to_sleep(&self) -> bool {
        self.forced_sleep
    }

    pub fn toggle_sleep(&mut self) {
        if self.is_sleeping() {
            self.wake_up();
        } else {
            self.sleep();
        }
    }

    /// Lies down to sleep until [`Dog::wake_up`]. While away (hidden) only the state is kept;
    /// the dog falls asleep when it comes back.
    pub fn sleep(&mut self) {
        self.forced_sleep = true;
        self.drop_toy();
        if !self.is_busy() {
            self.set(Activity::Sleep, f32::INFINITY);
        }
    }

    /// Ends [`Dog::sleep`].
    pub fn wake_up(&mut self) {
        let was = self.is_sleeping();
        self.forced_sleep = false;
        if was && !self.is_busy() {
            self.set(Activity::Stand, 1.5);
        }
    }

    fn is_busy(&self) -> bool {
        matches!(self.activity, Activity::Held | Activity::Falling)
            || matches!(self.visit, Visit::Entering { .. } | Visit::Leaving { .. })
            || self.away
    }

    /// True while the dog does something it was asked to (zoomies, walking in or out,
    /// fetching), so a notification or a held bubble does not cut it short.
    pub fn is_performing(&self) -> bool {
        self.zoomies > 0
            || matches!(self.visit, Visit::Entering { .. } | Visit::Leaving { .. })
            || self.fetch != Fetch::None
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
        if self.zoomies > 0 {
            self.zoomies -= 1;
            if self.zoomies > 0 {
                return self.zoom(b);
            }
            return self.set(Activity::Happy, 1.5);
        }
        if let Visit::Leaving { target } = self.visit {
            // Resume walking off (e.g. after being dropped).
            let origin = self.x.round();
            return self.set(Activity::Walk { origin, target }, 0.0);
        }
        if let Visit::Entering { .. } = self.visit {
            // Dropped while walking in: it is on screen now, close enough.
            self.visit = Visit::None;
        }
        if let Fetch::Returning { home } = self.fetch {
            // Dropped while bringing the toy back: carry on.
            let origin = self.x.round();
            return self.set(
                Activity::Walk {
                    origin,
                    target: home,
                },
                0.0,
            );
        }
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
    fn zoomies_run_then_end_happy() {
        let mut dog = Dog::new(500.0, B.ground);
        dog.zoomies(&B);
        assert!(dog.is_performing());
        assert_eq!(dog.frame().anim, Anim::Walk);
        let mut moved = 0.0f32;
        for _ in 0..(60 * 20) {
            let x = dog.x;
            dog.update(1.0 / 60.0, &B);
            moved += (dog.x - x).abs();
            if !dog.is_performing() {
                break;
            }
        }
        assert!(!dog.is_performing());
        assert!(moved > 400.0 * B.scale, "{moved}");
        assert_eq!(dog.frame().anim, Anim::Happy);
    }

    #[test]
    fn walks_off_and_back_in_from_the_right() {
        let mut dog = Dog::new(500.0, B.ground);
        let run = |dog: &mut Dog, secs: usize| {
            for _ in 0..(60 * secs) {
                dog.update(1.0 / 60.0, &B);
            }
        };
        dog.leave(&B);
        assert!(dog.is_performing());
        // Notifications and petting do not interrupt the walk.
        dog.notify();
        dog.pet();
        assert_eq!(dog.frame().anim, Anim::Walk);
        run(&mut dog, 30);
        assert!(dog.is_away());
        assert!(dog.x > B.right);
        assert!(dog.take_left());
        assert!(!dog.take_left());

        // Show: in from the right edge.
        dog.enter(&B);
        assert!(!dog.is_away());
        assert!(dog.x > B.right);
        run(&mut dog, 30);
        assert!(!dog.is_performing());
        assert!(dog.x <= B.right);

        // Leaving and coming back turns around mid-way.
        dog.leave(&B);
        run(&mut dog, 1);
        dog.enter(&B);
        run(&mut dog, 30);
        assert!(!dog.is_away() && !dog.is_performing());
        assert!(dog.x <= B.right);
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
        // Notifications do not wake it; petting does.
        dog.notify();
        assert!(dog.is_put_to_sleep());
        assert_eq!(dog.frame().anim, Anim::Sleep);
        dog.pet();
        assert!(!dog.is_sleeping());
    }

    #[test]
    fn sleep_survives_hide_and_show() {
        let mut dog = Dog::new(500.0, B.ground);
        let run = |dog: &mut Dog| {
            for _ in 0..(60 * 30) {
                dog.update(1.0 / 60.0, &B);
            }
        };
        dog.sleep();
        dog.leave(&B);
        run(&mut dog);
        assert!(dog.is_away());
        assert!(dog.is_put_to_sleep(), "still asleep while away");
        dog.wake_up();
        dog.sleep();
        dog.enter(&B);
        run(&mut dog);
        assert!(!dog.is_away());
        assert_eq!(dog.frame().anim, Anim::Sleep, "lies down again once back");
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

    fn run_until(dog: &mut Dog, secs: f32, done: impl Fn(&Dog) -> bool) {
        for _ in 0..(secs * 60.0) as usize {
            dog.update(1.0 / 60.0, &B);
            if done(dog) {
                return;
            }
        }
    }

    #[test]
    fn fetches_the_toy_and_brings_it_back() {
        let mut dog = Dog::new(500.0, B.ground);
        dog.update(1.0 / 60.0, &B);
        assert!(dog.fetch(1400.0, &B));
        assert!(dog.is_performing());
        assert_eq!(dog.facing, 1.0);
        // A notification does not interrupt the game.
        dog.notify();
        assert_eq!(dog.frame().anim, Anim::Walk);
        run_until(&mut dog, 20.0, |d| d.is_carrying());
        assert!(dog.take_picked());
        // Picked up where the mouth met the toy, then turned for home.
        assert_eq!(dog.facing, -1.0);
        assert!(
            (dog.x + MOUTH * B.scale - 1400.0).abs() <= WALK_STRIDE * B.scale + 1.0,
            "{}",
            dog.x
        );
        assert!(dog.frame().carry);
        run_until(&mut dog, 20.0, |d| !d.is_fetching());
        assert_eq!(dog.x, 500.0, "back home");
        assert!(!dog.is_carrying());
        let at = dog.take_dropped().expect("puts it down");
        assert!((at - dog.mouth_x()).abs() < 1.0);
        assert_eq!(dog.frame().anim, Anim::Happy);
    }

    #[test]
    fn fetch_to_the_left_and_dropping_the_dog() {
        let mut dog = Dog::new(1000.0, B.ground);
        dog.update(1.0 / 60.0, &B);
        dog.fetch(300.0, &B);
        assert_eq!(dog.facing, -1.0);
        run_until(&mut dog, 20.0, |d| d.is_carrying());
        // Picked up while carrying: the toy falls, the game ends.
        dog.grab();
        assert!(!dog.is_fetching() && !dog.is_carrying());
        assert!(dog.take_dropped().is_some());
        // Asleep: no fetch.
        dog.release();
        run_until(&mut dog, 3.0, |_| false);
        dog.sleep();
        assert!(!dog.fetch(900.0, &B));
    }

    #[test]
    fn sad_for_a_while() {
        let mut dog = Dog::new(500.0, B.ground);
        dog.sad();
        assert_eq!(dog.frame().anim, Anim::Sad);
        run_until(&mut dog, 10.0, |d| d.frame().anim != Anim::Sad);
        assert_ne!(dog.frame().anim, Anim::Sad);
        assert_eq!(Anim::Sad.cell(0), 32);
    }
}
