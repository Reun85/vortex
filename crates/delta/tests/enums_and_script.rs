//! Reflected enums and `#[reflect(script)]` marking.

use delta::{FieldType, Reflect, World, reflect::EnumRepr};
use serde_json::json;

#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[repr(u8)]
enum Mode {
    Idle,
    Walk = 4,
    Run,
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[repr(u16)]
enum Wide {
    A = 300,
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[repr(C)]
#[reflect(name = "test::Agent", script)]
struct Agent {
    mode: Mode,
    wide: Wide,
    speed: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[repr(C)]
struct Plain {
    n: u32,
}

#[test]
fn enum_fields_describe_themselves() {
    let mut world = World::new();
    world.register_reflected::<Agent>();
    let id = world.reflection().id_by_name("test::Agent").unwrap();
    let t = world.reflection().get(id);
    assert!(t.script, "marked #[reflect(script)]");
    let mode = t.field("mode").unwrap();
    let FieldType::Enum { repr, variants } = &mode.ty else {
        panic!("{:?}", mode.ty)
    };
    assert_eq!(*repr, EnumRepr::U8);
    let names: Vec<_> = variants.iter().map(|(n, v)| (&**n, *v)).collect();
    assert_eq!(names, [("Idle", 0), ("Walk", 4), ("Run", 5)]);
    assert_eq!(t.field("wide").unwrap().offset, 2);
    assert_eq!(t.size, 8);

    world.register_reflected::<Plain>();
    let plain = world
        .reflection()
        .id_by_name(&format!("{}::Plain", module_path!()));
    assert!(!world.reflection().get(plain.unwrap()).script);
}

#[test]
fn enums_round_trip_by_name() {
    let mut world = World::new();
    world.register_reflected::<Agent>();
    let e = world.spawn();
    world
        .insert(
            e,
            Agent {
                mode: Mode::Walk,
                wide: Wide::A,
                speed: 1.5,
            },
        )
        .unwrap();
    let v = world.component_value(e, "test::Agent").unwrap();
    assert_eq!(v["mode"], "Walk");
    assert_eq!(v["wide"], "A");

    // Set by name or by number; a bad name says what is valid.
    world
        .set_field_value(e, "test::Agent", "mode", &json!("Run"))
        .unwrap();
    assert_eq!(world.get::<Agent>(e).unwrap().mode, Mode::Run);
    world
        .set_field_value(e, "test::Agent", "mode", &json!(0))
        .unwrap();
    assert_eq!(world.get::<Agent>(e).unwrap().mode, Mode::Idle);
    let err = world
        .set_field_value(e, "test::Agent", "mode", &json!("Fly"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("Idle, Walk, Run"), "{err}");
    let err = world
        .set_field_value(e, "test::Agent", "mode", &json!(300))
        .unwrap_err()
        .to_string();
    assert!(err.contains("does not fit"), "{err}");
}
