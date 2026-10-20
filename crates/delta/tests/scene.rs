//! Reflection and scene files, through the public API only.

use std::mem::offset_of;

use delta::{Entity, FieldType, Reflect, SceneError, SceneId, World};

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(name = "game::Transform")]
struct Transform {
    position: Vec3,
    scale: f32,
    rotation: [f32; 4],
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
struct Health {
    current: i32,
    #[reflect(range(0.0, 100.0), tooltip = "Hit points")]
    max: i32,
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
struct Follows {
    target: Entity,
    speed: f64,
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
struct Player;

#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
struct Misc {
    flag: bool,
    a: u8,
    b: i16,
    c: u64,
    d: i64,
    e: u32,
    f: [u16; 3],
    #[reflect(hidden, read_only)]
    g: i8,
}

/// Not reflected: lives only at runtime.
struct Runtime(#[allow(dead_code)] u8);

struct Unrelated1;
struct Unrelated2;

fn sid(n: u128) -> SceneId {
    SceneId(n)
}

fn transform(seed: f32) -> Transform {
    Transform {
        position: Vec3 {
            x: seed,
            y: seed + 1.5,
            z: -seed,
        },
        scale: 2.0,
        rotation: [0.0, 0.5, -0.25, 1.0],
    }
}

fn misc_extremes() -> Misc {
    Misc {
        flag: true,
        a: u8::MAX,
        b: i16::MIN,
        c: u64::MAX,
        d: i64::MIN,
        e: 4_000_000_007,
        f: [1, 2, 65535],
        g: -128,
    }
}

/// Registration order A: Transform, Health, Follows, Player, Misc.
fn world_a() -> World {
    let mut w = World::new();
    w.register_reflected::<Transform>();
    w.register_reflected::<Health>();
    w.register_reflected::<Follows>();
    w.register_reflected::<Player>();
    w.register_reflected::<Misc>();
    w
}

/// Registration order B: unrelated components first, then the reverse.
fn world_b() -> World {
    let mut w = World::new();
    w.component_id::<Unrelated1>();
    w.component_id::<Unrelated2>();
    w.register_reflected::<Misc>();
    w.register_reflected::<Player>();
    w.register_reflected::<Follows>();
    w.register_reflected::<Health>();
    w.register_reflected::<Transform>();
    w
}

// --- the roadmap exit test -------------------------------------------------

#[test]
fn save_reload_with_components_registered_in_a_different_order() {
    let mut a = world_a();

    let e1 = a.spawn_with_scene_id(sid(0xA1));
    a.insert(e1, transform(1.0)).unwrap();
    a.insert(
        e1,
        Health {
            current: 7,
            max: 100,
        },
    )
    .unwrap();
    a.insert(e1, Player).unwrap();

    let e2 = a.spawn_with_scene_id(sid(0xA2));
    a.insert(
        e2,
        Follows {
            target: e1,
            speed: 3.25,
        },
    )
    .unwrap();
    a.insert(e2, misc_extremes()).unwrap();

    // References may point backwards, forwards or nowhere at all.
    let e3 = a.spawn_with_scene_id(sid(0xA3));
    a.insert(
        e3,
        Follows {
            target: e2,
            speed: -0.0,
        },
    )
    .unwrap();
    a.insert(e3, transform(-4.0)).unwrap();

    // Not saved: no SceneId, so it is transient. A reference to it saves as
    // null and loads as the placeholder.
    let transient = a.spawn();
    a.insert(transient, Health { current: 1, max: 1 }).unwrap();
    let e4 = a.spawn_with_scene_id(sid(0xA4));
    a.insert(
        e4,
        Follows {
            target: transient,
            speed: 1.0,
        },
    )
    .unwrap();
    let e5 = a.spawn_with_scene_id(sid(0xA5));
    a.insert(
        e5,
        Follows {
            target: Entity::PLACEHOLDER,
            speed: 2.0,
        },
    )
    .unwrap();
    // Runtime-only components are skipped, the rest of the entity is kept.
    a.insert(e1, Runtime(9)).unwrap();

    let json = a.save_scene();

    let mut b = world_b();
    // The two worlds really do disagree about component ids.
    assert_ne!(
        a.components().id_of::<Transform>(),
        b.components().id_of::<Transform>()
    );
    assert_ne!(
        a.components().id_of::<Misc>(),
        b.components().id_of::<Misc>()
    );

    let report = b.load_scene(&json).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.entities.len(), 5);

    let ids = [0xA1, 0xA2, 0xA3, 0xA4, 0xA5].map(sid);
    for id in ids {
        let (ea, eb) =
            (a.scene_entity(id).unwrap(), b.scene_entity(id).unwrap());
        assert_eq!(a.get::<Transform>(ea), b.get::<Transform>(eb));
        assert_eq!(a.get::<Health>(ea), b.get::<Health>(eb));
        assert_eq!(a.get::<Misc>(ea), b.get::<Misc>(eb));
        assert_eq!(a.has::<Player>(ea), b.has::<Player>(eb));
        assert!(!b.has::<Runtime>(eb));

        match (a.get::<Follows>(ea), b.get::<Follows>(eb)) {
            (None, None) => {}
            (Some(fa), Some(fb)) => {
                assert_eq!(fa.speed.to_bits(), fb.speed.to_bits());
                // References survive as the *same scene entity*, even though
                // the runtime Entity values differ between the worlds.
                match a.scene_id_of(fa.target) {
                    Some(target) => {
                        assert_eq!(b.scene_id_of(fb.target), Some(target));
                    }
                    None => assert_eq!(fb.target, Entity::PLACEHOLDER),
                }
            }
            other => panic!("Follows mismatch: {other:?}"),
        }
    }
    assert_eq!(
        b.get::<Misc>(b.scene_entity(sid(0xA2)).unwrap()),
        Some(&misc_extremes())
    );

    // Only the five scene entities were created; the transient one was not.
    assert_eq!(b.entities().len(), 5);

    // Saving again is byte-for-byte the same document: names, not ids, and a
    // deterministic order.
    assert_eq!(b.save_scene(), json);
}

// --- reflection data -------------------------------------------------------

#[test]
fn derive_describes_layout_names_and_attributes() {
    let mut w = World::new();
    let c = w.register_reflected::<Transform>();
    let rid = w.components().info(c).reflect.unwrap();
    let reg = w.reflection();
    let t = reg.get(rid);

    assert_eq!(&*t.name, "game::Transform");
    assert_eq!(t.size, size_of::<Transform>());
    assert_eq!(t.align, align_of::<Transform>());
    assert_eq!(w.components().id_by_name("game::Transform"), Some(c));

    let names: Vec<_> = t.fields.iter().map(|f| &*f.name).collect();
    assert_eq!(names, ["position", "scale", "rotation"]);
    assert_eq!(
        t.field("scale").unwrap().offset,
        offset_of!(Transform, scale)
    );
    assert_eq!(t.field("scale").unwrap().ty, FieldType::F32);
    assert_eq!(
        t.field("rotation").unwrap().ty,
        FieldType::Array {
            elem: Box::new(FieldType::F32),
            len: 4,
            stride: 4
        }
    );
    // Nested structs are registered too and referenced by id.
    let FieldType::Struct(vec3) = t.field("position").unwrap().ty else {
        panic!("position should be a struct");
    };
    assert!(reg.get(vec3).name.ends_with("::Vec3"));
    assert_eq!(reg.get(vec3).fields.len(), 3);

    let mut w2 = World::new();
    let h = w2.register_reflected::<Health>();
    let health = w2
        .reflection()
        .get(w2.components().info(h).reflect.unwrap());
    let max = health.field("max").unwrap();
    assert_eq!(max.attrs.range, Some((0.0, 100.0)));
    assert_eq!(max.attrs.tooltip.as_deref(), Some("Hit points"));
    assert!(!health.field("current").unwrap().attrs.hidden);

    let m = w2.register_reflected::<Misc>();
    let misc = w2
        .reflection()
        .get(w2.components().info(m).reflect.unwrap());
    let g = misc.field("g").unwrap();
    assert!(g.attrs.hidden && g.attrs.read_only);

    let p = w2.register_reflected::<Player>();
    let player = w2
        .reflection()
        .get(w2.components().info(p).reflect.unwrap());
    assert!(player.fields.is_empty());
}

#[test]
fn registering_after_a_plain_insert_renames_to_the_stable_name() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(
        e,
        Transform {
            position: Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            scale: 1.0,
            rotation: [0.0; 4],
        },
    )
    .unwrap();
    let before = w.components().id_of::<Transform>().unwrap();
    let id = w.register_reflected::<Transform>();
    // Same component, same id, now known by its stable name.
    assert_eq!(id, before);
    assert_eq!(w.components().id_by_name("game::Transform"), Some(id));
    assert!(w.components().info(id).reflect.is_some());
}

// --- partial and invalid data ---------------------------------------------

fn scene_with(components: &str) -> String {
    format!(
        r#"{{"version":1,"entities":[{{"id":"{}","components":{components}}}]}}"#,
        sid(1)
    )
}

#[test]
fn missing_fields_default_and_unknown_fields_are_dropped_with_warnings() {
    let mut w = world_a();
    let health = name_of::<Health>(&w);
    let components = format!(r#"{{"{health}":{{"current":5,"bogus":1}}}}"#);
    let report = w.load_scene(&scene_with(&components)).unwrap();
    let e = report.entities[0];
    assert_eq!(w.get::<Health>(e), Some(&Health { current: 5, max: 0 }));
    assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
    assert!(
        report
            .warnings
            .iter()
            .any(|m| m.contains("max") && m.contains("missing"))
    );
    assert!(report.warnings.iter().any(|m| m.contains("bogus")));
}

fn name_of<T: 'static>(w: &World) -> String {
    let id = w.components().id_of::<T>().unwrap();
    w.components().info(id).name.to_string()
}

#[test]
fn errors_leave_the_world_untouched() {
    let mut w = world_a();
    let health = name_of::<Health>(&w);
    let misc = name_of::<Misc>(&w);

    // Unknown component name.
    let err = w
        .load_scene(&scene_with(r#"{"nope::Missing":{}}"#))
        .unwrap_err();
    assert_eq!(
        err,
        SceneError::UnknownComponent {
            entity: sid(1),
            name: "nope::Missing".into()
        }
    );

    // A value that does not fit its field.
    let err = w
        .load_scene(&scene_with(&format!(r#"{{"{misc}":{{"a":300}}}}"#)))
        .unwrap_err();
    assert!(
        matches!(&err, SceneError::Field { message, .. } if message.contains("out of range")),
        "{err}"
    );

    // Wrong shape.
    let err = w
        .load_scene(&scene_with(&format!(
            r#"{{"{health}":{{"current":"many"}}}}"#
        )))
        .unwrap_err();
    assert!(matches!(err, SceneError::Field { .. }));

    // Nothing was left behind by any of the failed loads.
    assert_eq!(w.entities().len(), 0);
    assert_eq!(w.scene_entity(sid(1)), None);
}

#[test]
fn a_registered_but_not_reflected_component_is_reported() {
    let mut w = world_a();
    w.component_id::<Runtime>();
    let name = name_of::<Runtime>(&w);
    let err = w
        .load_scene(&scene_with(&format!(r#"{{"{name}":{{}}}}"#)))
        .unwrap_err();
    assert_eq!(err, SceneError::NotReflected { name });
}

#[test]
fn duplicate_ids_and_bad_documents() {
    let mut w = world_a();
    let one = format!(r#"{{"id":"{}","components":{{}}}}"#, sid(7));
    let twice = format!(r#"{{"version":1,"entities":[{one},{one}]}}"#);
    assert_eq!(
        w.load_scene(&twice).unwrap_err(),
        SceneError::DuplicateId(sid(7))
    );
    assert_eq!(w.entities().len(), 0);

    let once = format!(r#"{{"version":1,"entities":[{one}]}}"#);
    w.load_scene(&once).unwrap();
    // Loading it again would clash with the entity that now exists.
    assert_eq!(
        w.load_scene(&once).unwrap_err(),
        SceneError::DuplicateId(sid(7))
    );
    assert_eq!(w.entities().len(), 1);

    assert!(matches!(
        w.load_scene("not json").unwrap_err(),
        SceneError::Json(_)
    ));
    assert!(matches!(
        w.load_scene(r#"{"version":99,"entities":[]}"#).unwrap_err(),
        SceneError::Format(_)
    ));
    assert!(matches!(
        w.load_scene(
            r#"{"version":1,"entities":[{"id":"zz","components":{}}]}"#
        )
        .unwrap_err(),
        SceneError::Format(_)
    ));
}

#[test]
fn references_can_target_entities_already_in_the_world() {
    let mut w = world_a();
    let existing = w.spawn_with_scene_id(sid(0x99));
    let follows = name_of::<Follows>(&w);
    let json = scene_with(&format!(
        r#"{{"{follows}":{{"target":"{}","speed":1.5}}}}"#,
        sid(0x99)
    ));
    let report = w.load_scene(&json).unwrap();
    assert!(report.warnings.is_empty());
    let f = w.get::<Follows>(report.entities[0]).unwrap();
    assert_eq!(f.target, existing);

    // A dangling reference becomes the placeholder, with a warning.
    let mut w2 = world_a();
    let json = scene_with(&format!(
        r#"{{"{follows}":{{"target":"{}","speed":1.5}}}}"#,
        sid(0xDEAD)
    ));
    let report = w2.load_scene(&json).unwrap();
    assert_eq!(
        w2.get::<Follows>(report.entities[0]).unwrap().target,
        Entity::PLACEHOLDER
    );
    assert_eq!(report.warnings.len(), 1);
}

#[test]
fn non_finite_floats_survive_json() {
    let mut a = world_a();
    let e = a.spawn_with_scene_id(sid(1));
    let t = Transform {
        position: Vec3 {
            x: f32::INFINITY,
            y: f32::NEG_INFINITY,
            z: f32::NAN,
        },
        scale: f32::MIN_POSITIVE,
        rotation: [-0.0, f32::MAX, 1e-30, 0.1],
    };
    a.insert(e, t).unwrap();
    let mut b = world_b();
    let report = b.load_scene(&a.save_scene()).unwrap();
    let got = b.get::<Transform>(report.entities[0]).unwrap();
    assert_eq!(got.position.x, f32::INFINITY);
    assert_eq!(got.position.y, f32::NEG_INFINITY);
    assert!(got.position.z.is_nan());
    assert_eq!(got.scale, f32::MIN_POSITIVE);
    for (x, y) in got.rotation.iter().zip(t.rotation) {
        assert_eq!(x.to_bits(), y.to_bits());
    }
}

// --- SceneId bookkeeping ---------------------------------------------------

#[test]
fn scene_ids_follow_spawn_despawn_and_overwrite() {
    let mut w = World::new();
    let e = w.spawn_with_scene_id(sid(1));
    assert_eq!(w.scene_entity(sid(1)), Some(e));
    assert_eq!(w.scene_id_of(e), Some(sid(1)));

    // Giving the entity a new identity moves the mapping.
    w.insert(e, sid(2)).unwrap();
    assert_eq!(w.scene_entity(sid(1)), None);
    assert_eq!(w.scene_entity(sid(2)), Some(e));

    // Despawning removes it; a recycled entity slot never inherits an id.
    w.despawn(e).unwrap();
    assert_eq!(w.scene_entity(sid(2)), None);
    let reused = w.spawn();
    assert_eq!(reused.index(), e.index());
    assert_eq!(w.scene_id_of(reused), None);

    let f = w.spawn_scene_entity();
    assert!(w.scene_id_of(f).is_some());
}

#[test]
#[should_panic(expected = "already in use")]
fn spawning_a_duplicate_scene_id_panics() {
    let mut w = World::new();
    w.spawn_with_scene_id(sid(1));
    w.spawn_with_scene_id(sid(1));
}

#[test]
fn random_scene_ids_are_distinct_and_roundtrip_as_text() {
    let ids: std::collections::HashSet<_> =
        (0..2000).map(|_| SceneId::random()).collect();
    assert_eq!(ids.len(), 2000);
    let id = SceneId::random();
    assert_eq!(SceneId::parse(&id.to_string()), Some(id));
    assert_eq!(id.to_string().len(), 32);
}

// --- property: round trip through the file preserves everything -----------

mod roundtrip {
    use proptest::prelude::*;

    use super::*;

    fn same(a: f32, b: f32) -> bool {
        a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
    }

    prop_compose! {
        fn arb_transform()(
            p in any::<[f32; 3]>(), scale in any::<f32>(), r in any::<[f32; 4]>()
        ) -> Transform {
            Transform { position: Vec3 { x: p[0], y: p[1], z: p[2] }, scale, rotation: r }
        }
    }
    prop_compose! {
        fn arb_misc()(
            flag in any::<bool>(), a in any::<u8>(), b in any::<i16>(),
            c in any::<u64>(), d in any::<i64>(), e in any::<u32>(),
            f in any::<[u16; 3]>(), g in any::<i8>()
        ) -> Misc {
            Misc { flag, a, b, c, d, e, f, g }
        }
    }

    proptest! {
        #[test]
        fn save_then_load_is_lossless(
            items in proptest::collection::vec(
                (arb_transform(), arb_misc(), any::<i32>(), any::<f64>(), any::<bool>()),
                0..12,
            )
        ) {
            let mut a = world_a();
            let mut ents = Vec::new();
            for (i, (t, m, hp, speed, player)) in items.iter().enumerate() {
                let e = a.spawn_with_scene_id(sid(100 + i as u128));
                a.insert(e, *t).unwrap();
                a.insert(e, *m).unwrap();
                a.insert(e, Health { current: *hp, max: 100 }).unwrap();
                // Each entity follows the previous one (or nothing).
                let target = ents.last().copied().unwrap_or(Entity::PLACEHOLDER);
                a.insert(e, Follows { target, speed: *speed }).unwrap();
                if *player { a.insert(e, Player).unwrap(); }
                ents.push(e);
            }
            let json = a.save_scene();
            let mut b = world_b();
            let report = b.load_scene(&json).unwrap();
            prop_assert!(report.warnings.is_empty());
            prop_assert_eq!(&b.save_scene(), &json);

            for (i, (t, m, hp, speed, player)) in items.iter().enumerate() {
                let e = b.scene_entity(sid(100 + i as u128)).unwrap();
                let got = b.get::<Transform>(e).unwrap();
                prop_assert!(same(got.position.x, t.position.x));
                prop_assert!(same(got.position.y, t.position.y));
                prop_assert!(same(got.position.z, t.position.z));
                prop_assert!(same(got.scale, t.scale));
                for (x, y) in got.rotation.iter().zip(t.rotation) { prop_assert!(same(*x, y)); }
                prop_assert_eq!(b.get::<Misc>(e), Some(m));
                prop_assert_eq!(b.get::<Health>(e).unwrap().current, *hp);
                prop_assert_eq!(b.has::<Player>(e), *player);
                let f = b.get::<Follows>(e).unwrap();
                prop_assert!(f.speed.to_bits() == speed.to_bits() || (f.speed.is_nan() && speed.is_nan()));
                if i == 0 {
                    prop_assert_eq!(f.target, Entity::PLACEHOLDER);
                } else {
                    prop_assert_eq!(b.scene_id_of(f.target), Some(sid(100 + i as u128 - 1)));
                }
            }
        }
    }
}
