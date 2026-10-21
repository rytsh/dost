//! The toy (a bone, a pumpkin at Halloween): it lies next to the dog, can be dragged and
//! thrown, flies with gravity and bounces, and the dog fetches it. Platform-independent;
//! coordinates are physical screen pixels, `y` is the toy's bottom.

use crate::dog::Bounds;

const GRAVITY: f32 = 2600.0;
/// Bounce: kept share of the speed.
const BOUNCE: f32 = 0.42;
/// Rolling friction on the ground, per second.
const FRICTION: f32 = 3.2;
/// Fastest throw (physical pixels per second, per sprite-pixel scale).
const MAX_SPEED: f32 = 2200.0;
/// Let go faster than this and it counts as a throw (the dog fetches it).
const THROWN: f32 = 250.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    /// Lying on the ground.
    Resting,
    /// Being dragged by the mouse.
    Held,
    Flying,
    /// In the dog's mouth (not drawn here).
    Carried,
}

#[derive(Clone, Debug)]
pub struct Toy {
    pub x: f32,
    pub y: f32,
    vx: f32,
    vy: f32,
    pub state: State,
    /// Rotation, quarter turns (pixel art turns in whole quarters).
    spin: f32,
    spin_speed: f32,
    /// Recent drag positions and their times, for the throw speed.
    trail: Vec<(f32, f32, f32)>,
    clock: f32,
    /// In the air because the user threw it (not dropped or put down).
    thrown: bool,
    /// Landed after a throw; read once by [`Toy::take_landed`].
    landed: bool,
}

impl Toy {
    pub fn new(x: f32, ground: f32) -> Self {
        Toy {
            x,
            y: ground,
            vx: 0.0,
            vy: 0.0,
            state: State::Resting,
            spin: 0.0,
            spin_speed: 0.0,
            trail: Vec::new(),
            clock: 0.0,
            thrown: false,
            landed: false,
        }
    }

    /// Quarter turns to draw.
    pub fn quarter(&self) -> u8 {
        (self.spin.round() as i32).rem_euclid(4) as u8
    }

    pub fn is_shown(&self) -> bool {
        self.state != State::Carried
    }

    pub fn update(&mut self, dt: f32, b: &Bounds, edges: (f32, f32)) {
        self.clock += dt;
        match self.state {
            State::Held | State::Carried => {}
            State::Resting => {
                // The ground moved (taskbar, settings): stay on it.
                if self.y < b.ground - 1.0 {
                    self.state = State::Flying;
                } else {
                    self.y = b.ground;
                }
                self.x = self.x.clamp(edges.0, edges.1);
            }
            State::Flying => {
                self.vy += GRAVITY * b.scale * dt;
                self.x += self.vx * dt;
                self.y += self.vy * dt;
                self.spin += self.spin_speed * dt;
                // Screen sides: bounce back.
                if self.x < edges.0 {
                    self.x = edges.0;
                    self.vx = self.vx.abs() * BOUNCE;
                } else if self.x > edges.1 {
                    self.x = edges.1;
                    self.vx = -self.vx.abs() * BOUNCE;
                }
                if self.y >= b.ground {
                    self.y = b.ground;
                    if self.vy > 260.0 * b.scale {
                        self.vy = -self.vy * BOUNCE;
                        self.spin_speed *= 0.6;
                    } else {
                        self.vy = 0.0;
                        self.vx *= (1.0 - FRICTION * dt).max(0.0);
                        self.spin_speed = 0.0;
                        self.spin = self.spin.round() - self.spin.round() % 2.0;
                        if self.vx.abs() < 30.0 * b.scale {
                            self.vx = 0.0;
                            self.state = State::Resting;
                            self.landed = std::mem::take(&mut self.thrown);
                        }
                    }
                }
            }
        }
    }

    /// Mouse down on the toy.
    pub fn grab(&mut self) {
        self.state = State::Held;
        self.vx = 0.0;
        self.vy = 0.0;
        self.trail.clear();
        self.thrown = false;
        self.landed = false;
    }

    pub fn drag_to(&mut self, x: f32, y: f32) {
        if self.state != State::Held {
            return;
        }
        self.x = x;
        self.y = y;
        self.trail.push((self.clock, x, y));
        let now = self.clock;
        self.trail.retain(|(t, _, _)| now - t < 0.1);
    }

    /// Let go: thrown with the speed of the last moments of the drag.
    pub fn release(&mut self, scale: f32) {
        if self.state != State::Held {
            return;
        }
        let (vx, vy) = match (self.trail.first(), self.trail.last()) {
            (Some(a), Some(b)) if b.0 - a.0 > 0.005 => {
                ((b.1 - a.1) / (b.0 - a.0), (b.2 - a.2) / (b.0 - a.0))
            }
            _ => (0.0, 0.0),
        };
        let max = MAX_SPEED * scale;
        let speed = (vx * vx + vy * vy).sqrt();
        let k = if speed > max { max / speed } else { 1.0 };
        self.throw(vx * k, vy * k);
        // Just let go (barely moving): dropped, not thrown.
        self.thrown = speed > THROWN * scale;
    }

    /// Sends the toy flying with this speed (physical pixels per second).
    pub fn throw(&mut self, vx: f32, vy: f32) {
        self.vx = vx;
        self.vy = vy;
        self.spin_speed = (vx / 220.0).clamp(-14.0, 14.0);
        self.state = State::Flying;
        self.thrown = true;
        self.landed = false;
    }

    /// The dog took it.
    pub fn carry(&mut self) {
        self.state = State::Carried;
    }

    /// The dog put it down (or dropped it) at `x`.
    pub fn put_down(&mut self, x: f32, y: f32) {
        self.x = x;
        self.y = y;
        self.vx = 0.0;
        self.vy = 0.0;
        self.spin = 0.0;
        self.state = State::Flying;
        self.thrown = false;
        self.landed = false;
    }

    /// True once after a throw comes to rest (the dog goes for it then).
    pub fn take_landed(&mut self) -> bool {
        std::mem::take(&mut self.landed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: Bounds = Bounds {
        ground: 1000.0,
        left: 100.0,
        right: 1800.0,
        scale: 2.0,
    };
    const EDGES: (f32, f32) = (20.0, 1900.0);

    fn run(t: &mut Toy, secs: f32) {
        for _ in 0..(secs * 60.0) as usize {
            t.update(1.0 / 60.0, &B, EDGES);
        }
    }

    #[test]
    fn thrown_flies_bounces_and_rests() {
        let mut t = Toy::new(500.0, B.ground);
        t.grab();
        for i in 0..6 {
            t.clock += 0.016;
            t.drag_to(500.0 + i as f32 * 20.0, 900.0 - i as f32 * 5.0);
        }
        t.release(B.scale);
        assert_eq!(t.state, State::Flying);
        let mut highest = t.y;
        for _ in 0..600 {
            t.update(1.0 / 60.0, &B, EDGES);
            highest = highest.min(t.y);
            assert!(t.y <= B.ground && t.x >= EDGES.0 && t.x <= EDGES.1);
        }
        assert!(highest < 900.0, "it went up");
        assert_eq!(t.state, State::Resting);
        assert!(t.x > 700.0, "it went right: {}", t.x);
        assert!(t.take_landed());
        assert!(!t.take_landed());
        assert_eq!(t.quarter() % 2, 0, "lies flat");
    }

    #[test]
    fn walls_and_drop() {
        let mut t = Toy::new(1800.0, B.ground);
        t.throw(4000.0, -100.0);
        run(&mut t, 10.0);
        assert!(t.x <= EDGES.1 && t.state == State::Resting);
        assert!(t.take_landed());
        // Let go without moving: falls straight down, and is not a throw.
        t.grab();
        t.drag_to(600.0, 500.0);
        t.release(B.scale);
        run(&mut t, 5.0);
        assert_eq!((t.x, t.y), (600.0, B.ground));
        assert!(!t.take_landed());
        // Put down by the dog: not a throw either.
        t.put_down(700.0, 900.0);
        run(&mut t, 5.0);
        assert!(!t.take_landed());
    }
}
