//! Bouncing balls in the terminal, driven by a delta schedule.
//!
//! Run with: `cargo run -p delta --example bouncing`

use std::{thread::sleep, time::Duration};

use delta::{Query, Res, ResMut, Schedule, Stage, Without, World};

const W: usize = 60;
const H: usize = 20;

struct Position(f32, f32);
struct Velocity(f32, f32);
struct Glyph(char);
/// Marks a ball that doesn't move.
struct Pinned;

struct Time {
    dt: f32,
}

struct Frame(Vec<Vec<char>>);

fn movement(
    time: Res<Time>,
    mut q: Query<(&mut Position, &Velocity), Without<Pinned>>,
) {
    for (mut p, v) in &mut q {
        p.0 += v.0 * time.dt;
        p.1 += v.1 * time.dt;
    }
}

fn bounce(mut q: Query<(&mut Position, &mut Velocity)>) {
    for (mut p, mut v) in &mut q {
        if p.0 < 0.0 || p.0 > (W - 1) as f32 {
            v.0 = -v.0;
            p.0 = p.0.clamp(0.0, (W - 1) as f32);
        }
        if p.1 < 0.0 || p.1 > (H - 1) as f32 {
            v.1 = -v.1;
            p.1 = p.1.clamp(0.0, (H - 1) as f32);
        }
    }
}

fn render(mut frame: ResMut<Frame>, q: Query<(&Position, &Glyph)>) {
    frame.0 = vec![vec![' '; W]; H];
    for (p, g) in &q {
        frame.0[p.1 as usize][p.0 as usize] = g.0;
    }
}

fn main() {
    let mut world = World::new();
    world.insert_resource(Time { dt: 1.0 / 20.0 });
    world.insert_resource(Frame(Vec::new()));

    let balls = [
        ('o', 5.0, 3.0, 22.0, 9.0, false),
        ('O', 30.0, 10.0, -15.0, 6.0, false),
        ('*', 50.0, 15.0, -30.0, -11.0, false),
        ('@', 12.0, 17.0, 18.0, -14.0, false),
        ('#', 30.0, 10.0, 0.0, 0.0, true), // pinned in the middle
    ];
    for (ch, x, y, vx, vy, pinned) in balls {
        let e = world.spawn();
        world.insert(e, Position(x, y)).unwrap();
        world.insert(e, Velocity(vx, vy)).unwrap();
        world.insert(e, Glyph(ch)).unwrap();
        if pinned {
            world.insert(e, Pinned).unwrap();
        }
    }

    let mut schedule = Schedule::new();
    schedule.add_system(Stage::Update, movement);
    schedule.add_system(Stage::Update, bounce);
    schedule.add_system(Stage::Update, render);

    for tick in 0..400 {
        schedule.run(&mut world);
        let frame = world.resource::<Frame>().unwrap();
        let mut out = String::from("\x1b[2J\x1b[H");
        out += &format!(
            "delta bouncing demo  tick {tick}  (Ctrl-C to quit)\n+{}+\n",
            "-".repeat(W)
        );
        for row in &frame.0 {
            out += &format!("|{}|\n", row.iter().collect::<String>());
        }
        out += &format!("+{}+\n", "-".repeat(W));
        print!("{out}");
        sleep(Duration::from_millis(50));
    }
}
