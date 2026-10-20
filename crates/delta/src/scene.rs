//! Scene files: entities and their reflected components, saved and loaded
//! by *name*.
//!
//! Component ids depend on registration order and differ between runs, so a
//! scene never contains one. Components are keyed by their stable name, and
//! entity references are stored as [`SceneId`]s, never as [`Entity`] values.

use std::{
    alloc::{self, Layout},
    collections::{HashMap, hash_map::RandomState},
    fmt,
    hash::{BuildHasher, Hasher},
    mem,
    ptr,
};

use serde_json::{Map, Number, Value};

use crate::{
    archetype::Archetype,
    component::ComponentId,
    entity::Entity,
    hooks::DeferredWorld,
    reflect::{FieldType, Reflect, TypeRegistry},
    world::World,
};

/// The stable identity of an editable entity, unlike [`Entity`] which is a
/// reusable runtime handle. Scene files, undo records and cross-entity
/// references store this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SceneId(pub u128);

impl SceneId {
    /// A fresh id, unique with overwhelming probability.
    pub fn random() -> Self {
        let hi = RandomState::new().build_hasher().finish();
        let lo = RandomState::new().build_hasher().finish();
        Self((u128::from(hi) << 64) | u128::from(lo))
    }

    /// Parses the 32-digit hex form written by [`Display`](fmt::Display).
    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() || s.len() > 32 {
            return None;
        }
        u128::from_str_radix(s, 16).ok().map(Self)
    }
}

impl fmt::Display for SceneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

#[derive(Debug, PartialEq)]
pub enum SceneError {
    /// The text is not valid JSON.
    Json(String),
    /// Valid JSON that is not a scene.
    Format(String),
    /// A scene entity uses a component name nothing has registered.
    UnknownComponent { entity: SceneId, name: String },
    /// The component is registered but has no reflection data.
    NotReflected { name: String },
    /// A `SceneId` appears twice, or already exists in the world.
    DuplicateId(SceneId),
    /// A field value does not fit its type.
    Field { component: String, message: String },
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(m) => write!(f, "invalid scene json: {m}"),
            Self::Format(m) => write!(f, "malformed scene: {m}"),
            Self::UnknownComponent { entity, name } => {
                write!(f, "entity {entity} uses unknown component `{name}`")
            }
            Self::NotReflected { name } => {
                write!(f, "component `{name}` has no reflection data")
            }
            Self::DuplicateId(id) => write!(f, "scene id {id} is already used"),
            Self::Field { component, message } => {
                write!(f, "in component `{component}`: {message}")
            }
        }
    }
}

impl std::error::Error for SceneError {}

/// What a successful load did.
#[derive(Debug, Default)]
pub struct LoadReport {
    /// The entities created, in file order.
    pub entities: Vec<Entity>,
    /// Recoverable problems: missing fields took defaults, unknown fields
    /// were dropped, dangling references became placeholders.
    pub warnings: Vec<String>,
}

const SCENE_VERSION: u64 = 1;

fn scene_id_inserted(w: DeferredWorld<'_>, e: Entity) {
    let id = *w.get::<SceneId>(e).expect("hook runs with the component");
    w.world.scene_ids.insert(id, e);
}

fn scene_id_replaced(w: DeferredWorld<'_>, e: Entity) {
    let id = *w.get::<SceneId>(e).expect("hook runs with the component");
    if w.world.scene_ids.get(&id) == Some(&e) {
        w.world.scene_ids.remove(&id);
    }
}

/// Keeps the `SceneId -> Entity` map patched on spawn and despawn.
pub(crate) fn init(world: &mut World) {
    world
        .register_hooks::<SceneId>()
        .on_insert(scene_id_inserted)
        .on_replace(scene_id_replaced);
}

impl World {
    pub fn reflection(&self) -> &TypeRegistry {
        &self.reflection
    }

    pub fn reflection_mut(&mut self) -> &mut TypeRegistry {
        &mut self.reflection
    }

    /// Registers `T` as a component under its reflected, stable name and
    /// records its layout for scenes, the inspector and undo.
    ///
    /// # Panics
    /// If `T` needs drop glue, which reflected data never has.
    pub fn register_reflected<T: Reflect + Send + Sync>(
        &mut self,
    ) -> ComponentId {
        assert!(
            !mem::needs_drop::<T>(),
            "reflected components must be plain data"
        );
        let rid = T::reflect_id(&mut self.reflection);
        let name = self.reflection.get(rid).name.clone();
        self.components.register_named::<T>(&name, Some(rid))
    }

    /// Spawns an entity with the given stable identity.
    ///
    /// # Panics
    /// If the id is already in use.
    pub fn spawn_with_scene_id(&mut self, id: SceneId) -> Entity {
        assert!(
            !self.scene_ids.contains_key(&id),
            "scene id {id} is already in use"
        );
        let e = self.spawn();
        self.insert(e, id).expect("just spawned");
        e
    }

    /// Spawns an entity with a fresh [`SceneId`].
    pub fn spawn_scene_entity(&mut self) -> Entity {
        self.spawn_with_scene_id(SceneId::random())
    }

    pub fn scene_entity(&self, id: SceneId) -> Option<Entity> {
        self.scene_ids.get(&id).copied()
    }

    pub fn scene_id_of(&self, e: Entity) -> Option<SceneId> {
        self.get::<SceneId>(e).copied()
    }

    fn entity_ref_to_json(&self, e: Entity) -> Value {
        self.scene_id_of(e)
            .map_or(Value::Null, |id| Value::String(id.to_string()))
    }

    /// The scene as a JSON value: every entity that has a [`SceneId`], with
    /// its reflected components. Entities are ordered by id and components by
    /// name, so equal worlds always produce equal text.
    pub fn scene_to_value(&self) -> Value {
        let scene_component = self.components.id_of::<SceneId>();
        let mut rows: Vec<(SceneId, &Archetype, usize)> = Vec::new();
        for arch in self.archetypes.iter() {
            let Some(col) = scene_component.and_then(|c| arch.column(c)) else {
                continue;
            };
            for row in 0..arch.len() {
                // SAFETY: the column holds initialized `SceneId`s.
                let id = unsafe { col.get_ptr(row).cast::<SceneId>().read() };
                rows.push((id, arch, row));
            }
        }
        rows.sort_by_key(|&(id, ..)| id);

        let entities = rows
            .into_iter()
            .map(|(id, arch, row)| {
                let mut components = Map::new();
                for (i, &c) in arch.components().iter().enumerate() {
                    let info = self.components.info(c);
                    let Some(reflect) = info.reflect else {
                        continue;
                    };
                    let ptr = arch.columns()[i].get_ptr(row);
                    let value = read_value(
                        &self.reflection,
                        &FieldType::Struct(reflect),
                        ptr,
                        &|e| self.entity_ref_to_json(e),
                    );
                    components.insert(info.name.to_string(), value);
                }
                let mut entity = Map::new();
                entity.insert("id".into(), Value::String(id.to_string()));
                entity.insert("components".into(), Value::Object(components));
                Value::Object(entity)
            })
            .collect();

        let mut doc = Map::new();
        doc.insert("version".into(), Value::from(SCENE_VERSION));
        doc.insert("entities".into(), Value::Array(entities));
        Value::Object(doc)
    }

    pub fn save_scene(&self) -> String {
        serde_json::to_string_pretty(&self.scene_to_value())
            .expect("a json value always serializes")
    }

    /// Creates the scene's entities in this world. Components are matched by
    /// name against what is registered here, whatever order that happened in.
    ///
    /// On error the world is left as it was before the call.
    pub fn load_scene(&mut self, json: &str) -> Result<LoadReport, SceneError> {
        let doc: Value = serde_json::from_str(json)
            .map_err(|e| SceneError::Json(e.to_string()))?;
        self.load_scene_value(&doc)
    }

    pub fn load_scene_value(
        &mut self,
        doc: &Value,
    ) -> Result<LoadReport, SceneError> {
        let format = |m: &str| SceneError::Format(m.to_owned());
        if doc.get("version").and_then(Value::as_u64) != Some(SCENE_VERSION) {
            return Err(format("unsupported or missing `version`"));
        }
        let list = doc
            .get("entities")
            .and_then(Value::as_array)
            .ok_or_else(|| format("`entities` must be an array"))?;

        let mut parsed: Vec<(SceneId, &Map<String, Value>)> = Vec::new();
        for entry in list {
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .and_then(SceneId::parse)
                .ok_or_else(|| format("entity without a valid `id`"))?;
            let components = entry
                .get("components")
                .and_then(Value::as_object)
                .ok_or_else(|| format("entity without `components`"))?;
            if self.scene_ids.contains_key(&id)
                || parsed.iter().any(|&(other, _)| other == id)
            {
                return Err(SceneError::DuplicateId(id));
            }
            parsed.push((id, components));
        }

        // Everything exists before any component is read, so references
        // between the scene's entities (in any direction) resolve.
        let spawned: Vec<Entity> = parsed
            .iter()
            .map(|&(id, _)| self.spawn_with_scene_id(id))
            .collect();

        let mut warnings = Vec::new();
        for (&(id, components), &entity) in parsed.iter().zip(&spawned) {
            if let Err(err) =
                self.fill_entity(id, entity, components, &mut warnings)
            {
                for &e in &spawned {
                    let _ = self.despawn(e);
                }
                return Err(err);
            }
        }
        Ok(LoadReport {
            entities: spawned,
            warnings,
        })
    }

    fn fill_entity(
        &mut self,
        id: SceneId,
        entity: Entity,
        components: &Map<String, Value>,
        warnings: &mut Vec<String>,
    ) -> Result<(), SceneError> {
        let mut buffers: Vec<(ComponentId, ZeroedBuf)> = Vec::new();
        for (name, value) in components {
            let c = self.components.id_by_name(name).ok_or_else(|| {
                SceneError::UnknownComponent {
                    entity: id,
                    name: name.clone(),
                }
            })?;
            let info = self.components.info(c);
            let reflect = info.reflect.ok_or_else(|| {
                SceneError::NotReflected { name: name.clone() }
            })?;
            let buffer = ZeroedBuf::new(info.layout);
            let mut ctx = WriteCtx {
                scene_ids: &self.scene_ids,
                warnings,
            };
            write_value(
                &self.reflection,
                &FieldType::Struct(reflect),
                value,
                buffer.ptr,
                &mut ctx,
                name,
            )
            .map_err(|message| SceneError::Field {
                component: name.clone(),
                message,
            })?;
            buffers.push((c, buffer));
        }
        let items: Vec<(ComponentId, *const u8)> = buffers
            .iter()
            .map(|(c, b)| (*c, b.ptr.cast_const()))
            .collect();
        // SAFETY: each buffer holds a fully initialized value of its
        // component's layout (zeroed, then filled), the ids are distinct
        // (JSON object keys), and reflected data has no drop glue.
        unsafe { self.insert_many_by_id(entity, &items) }
            .expect("the entity was just spawned");
        Ok(())
    }
}

/// A zero-initialized, correctly aligned scratch value.
struct ZeroedBuf {
    ptr: *mut u8,
    layout: Layout,
}

impl ZeroedBuf {
    fn new(layout: Layout) -> Self {
        let ptr = if layout.size() == 0 {
            ptr::without_provenance_mut(layout.align())
        } else {
            // SAFETY: the layout has non-zero size.
            let p = unsafe { alloc::alloc_zeroed(layout) };
            if p.is_null() {
                alloc::handle_alloc_error(layout);
            }
            p
        };
        Self { ptr, layout }
    }
}

impl Drop for ZeroedBuf {
    fn drop(&mut self) {
        if self.layout.size() != 0 {
            // SAFETY: allocated in `new` with this layout.
            unsafe { alloc::dealloc(self.ptr, self.layout) };
        }
    }
}

fn float_value(f: f64) -> Value {
    match Number::from_f64(f) {
        Some(n) => Value::Number(n),
        // JSON has no NaN or infinity.
        None if f.is_nan() => Value::String("NaN".into()),
        None if f > 0.0 => Value::String("inf".into()),
        None => Value::String("-inf".into()),
    }
}

fn parse_float(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => match s.as_str() {
            "NaN" => Some(f64::NAN),
            "inf" => Some(f64::INFINITY),
            "-inf" => Some(f64::NEG_INFINITY),
            _ => None,
        },
        _ => None,
    }
}

/// Turns the bytes of a reflected value into JSON.
fn read_value(
    reg: &TypeRegistry,
    ty: &FieldType,
    p: *const u8,
    refs: &dyn Fn(Entity) -> Value,
) -> Value {
    macro_rules! scalar {
        ($t:ty) => {
            // SAFETY: the caller passes a pointer to a value of this type;
            // unaligned reads also serve scratch buffers.
            Value::from(unsafe { p.cast::<$t>().read_unaligned() })
        };
    }
    match ty {
        FieldType::Bool => scalar!(bool),
        FieldType::I8 => scalar!(i8),
        FieldType::I16 => scalar!(i16),
        FieldType::I32 => scalar!(i32),
        FieldType::I64 => scalar!(i64),
        FieldType::U8 => scalar!(u8),
        FieldType::U16 => scalar!(u16),
        FieldType::U32 => scalar!(u32),
        FieldType::U64 => scalar!(u64),
        // SAFETY: as above.
        FieldType::F32 => {
            float_value(f64::from(unsafe { p.cast::<f32>().read_unaligned() }))
        }
        // SAFETY: as above.
        FieldType::F64 => {
            float_value(unsafe { p.cast::<f64>().read_unaligned() })
        }
        FieldType::Entity => {
            // SAFETY: as above.
            let e = unsafe { p.cast::<Entity>().read_unaligned() };
            if e == Entity::PLACEHOLDER {
                Value::Null
            } else {
                refs(e)
            }
        }
        FieldType::Array { elem, len, stride } => Value::Array(
            (0..*len)
                .map(|i| {
                    // SAFETY: element `i` lies inside the array.
                    read_value(reg, elem, unsafe { p.add(i * stride) }, refs)
                })
                .collect(),
        ),
        FieldType::Struct(id) => {
            let mut map = Map::new();
            for f in reg.get(*id).fields.iter() {
                // SAFETY: `offset` comes from `offset_of!` on this type.
                let fp = unsafe { p.add(f.offset) };
                map.insert(
                    f.name.to_string(),
                    read_value(reg, &f.ty, fp, refs),
                );
            }
            Value::Object(map)
        }
    }
}

struct WriteCtx<'a> {
    scene_ids: &'a HashMap<SceneId, Entity>,
    warnings: &'a mut Vec<String>,
}

impl WriteCtx<'_> {
    fn entity(&mut self, v: &Value, path: &str) -> Result<Entity, String> {
        match v {
            Value::Null => Ok(Entity::PLACEHOLDER),
            Value::String(s) => {
                let id = SceneId::parse(s).ok_or_else(|| {
                    format!("{path}: `{s}` is not a scene id")
                })?;
                if let Some(&e) = self.scene_ids.get(&id) {
                    Ok(e)
                } else {
                    self.warnings.push(format!(
                        "{path}: unknown scene id {s}, using a placeholder"
                    ));
                    Ok(Entity::PLACEHOLDER)
                }
            }
            _ => Err(format!("{path}: expected a scene id or null")),
        }
    }
}

/// Fills `dst` from JSON. `dst` must be zeroed and large enough: fields
/// missing from the JSON keep that zero default.
fn write_value(
    reg: &TypeRegistry,
    ty: &FieldType,
    v: &Value,
    dst: *mut u8,
    ctx: &mut WriteCtx<'_>,
    path: &str,
) -> Result<(), String> {
    macro_rules! int {
        ($t:ty, $get:ident) => {{
            let n = v.$get().ok_or_else(|| {
                format!("{path}: expected {}", stringify!($t))
            })?;
            let n = <$t>::try_from(n).map_err(|_| {
                format!("{path}: {n} is out of range for {}", stringify!($t))
            })?;
            // SAFETY: `dst` has room for this field.
            unsafe { dst.cast::<$t>().write_unaligned(n) };
        }};
    }
    match ty {
        FieldType::Bool => {
            let b = v
                .as_bool()
                .ok_or_else(|| format!("{path}: expected a bool"))?;
            // SAFETY: `dst` has room for this field.
            unsafe { dst.cast::<bool>().write_unaligned(b) };
        }
        FieldType::I8 => int!(i8, as_i64),
        FieldType::I16 => int!(i16, as_i64),
        FieldType::I32 => int!(i32, as_i64),
        FieldType::I64 => int!(i64, as_i64),
        FieldType::U8 => int!(u8, as_u64),
        FieldType::U16 => int!(u16, as_u64),
        FieldType::U32 => int!(u32, as_u64),
        FieldType::U64 => int!(u64, as_u64),
        FieldType::F32 => {
            let f = parse_float(v)
                .ok_or_else(|| format!("{path}: expected a number"))?;
            // SAFETY: `dst` has room for this field.
            unsafe { dst.cast::<f32>().write_unaligned(f as f32) };
        }
        FieldType::F64 => {
            let f = parse_float(v)
                .ok_or_else(|| format!("{path}: expected a number"))?;
            // SAFETY: `dst` has room for this field.
            unsafe { dst.cast::<f64>().write_unaligned(f) };
        }
        FieldType::Entity => {
            let e = ctx.entity(v, path)?;
            // SAFETY: `dst` has room for this field.
            unsafe { dst.cast::<Entity>().write_unaligned(e) };
        }
        FieldType::Array { elem, len, stride } => {
            let items = v
                .as_array()
                .ok_or_else(|| format!("{path}: expected an array"))?;
            if items.len() != *len {
                return Err(format!(
                    "{path}: expected {len} elements, found {}",
                    items.len()
                ));
            }
            for (i, item) in items.iter().enumerate() {
                // SAFETY: element `i` lies inside the array.
                let ep = unsafe { dst.add(i * stride) };
                write_value(reg, elem, item, ep, ctx, &format!("{path}[{i}]"))?;
            }
        }
        FieldType::Struct(id) => {
            let t = reg.get(*id);
            let obj = v
                .as_object()
                .ok_or_else(|| format!("{path}: expected an object"))?;
            for f in t.fields.iter() {
                let fpath = format!("{path}.{}", f.name);
                match obj.get(&*f.name) {
                    Some(item) => {
                        // SAFETY: `offset` comes from `offset_of!`.
                        let fp = unsafe { dst.add(f.offset) };
                        write_value(reg, &f.ty, item, fp, ctx, &fpath)?;
                    }
                    None => ctx.warnings.push(format!(
                        "{fpath}: missing, using the default (zero)"
                    )),
                }
            }
            for key in obj.keys() {
                if t.field(key).is_none() {
                    ctx.warnings
                        .push(format!("{path}.{key}: unknown field, dropped"));
                }
            }
        }
    }
    Ok(())
}
