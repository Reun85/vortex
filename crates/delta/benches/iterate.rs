use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use delta::{QueryDesc, QueryState, Tick, World};

const N: usize = 1_000_000;

#[derive(Clone, Copy)]
struct Position([f32; 4]);
#[derive(Clone, Copy)]
struct Velocity([f32; 4]);

fn step(p: &mut [Position], v: &[Velocity]) {
    for (p, v) in p.iter_mut().zip(v) {
        for i in 0..4 {
            p.0[i] += v.0[i];
        }
    }
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("iterate_1m");

    let mut pos = vec![Position([0.0; 4]); N];
    let vel = vec![Velocity([1.0; 4]); N];
    group.bench_function("plain_vec", |b| {
        b.iter(|| step(black_box(&mut pos), black_box(&vel)));
    });

    let mut world = World::new();
    let (p, v) = (
        world.component_id::<Position>(),
        world.component_id::<Velocity>(),
    );
    for _ in 0..N {
        let e = world.spawn();
        world.insert(e, Position([0.0; 4])).unwrap();
        world.insert(e, Velocity([1.0; 4])).unwrap();
    }
    let mut query = QueryState::new(QueryDesc {
        read: vec![v],
        write: vec![p],
        ..Default::default()
    });
    group.bench_function("delta_chunks", |b| {
        b.iter(|| {
            query.for_each_chunk_mut(&mut world, Tick::new(0), |mut chunk| {
                // SAFETY: term 0 is Velocity (read), term 1 is Position
                // (write), and they are distinct columns.
                let (v, p) = unsafe {
                    (chunk.slice::<Velocity>(0), chunk.slice_mut::<Position>(1))
                };
                step(black_box(p), black_box(v));
            });
        });
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
