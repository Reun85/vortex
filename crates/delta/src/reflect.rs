//! Runtime description of component layouts: the one mechanism behind the
//! inspector, undo, scene files, copy/paste and hot reload.
//!
//! Only plain data can be reflected: scalars, [`Entity`] references, fixed
//! arrays, and structs made of those. That is exactly what the scripting
//! bridge allows in a script-visible component, so a reflected type never
//! needs drop glue.

use std::{any::TypeId, collections::HashMap};

use crate::entity::Entity;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReflectId(u32);

impl ReflectId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// The type of one field, as far as serialization and tools care.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldType {
    Bool,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    /// An entity reference. Scene files store it as a `SceneId`.
    Entity,
    Array {
        elem: Box<FieldType>,
        len: usize,
        /// Bytes between consecutive elements.
        stride: usize,
    },
    Struct(ReflectId),
}

/// Editor hints attached to a field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldAttrs {
    pub range: Option<(f64, f64)>,
    pub tooltip: Option<Box<str>>,
    pub hidden: bool,
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldReflect {
    pub name: Box<str>,
    pub offset: usize,
    pub ty: FieldType,
    pub attrs: FieldAttrs,
}

impl FieldReflect {
    pub fn new(
        name: &str,
        offset: usize,
        ty: FieldType,
        attrs: FieldAttrs,
    ) -> Self {
        Self {
            name: name.into(),
            offset,
            ty,
            attrs,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeReflect {
    /// Stable name used by scene files and the script binding.
    pub name: Box<str>,
    pub size: usize,
    pub align: usize,
    pub fields: Box<[FieldReflect]>,
}

impl TypeReflect {
    pub fn field(&self, name: &str) -> Option<&FieldReflect> {
        self.fields.iter().find(|f| &*f.name == name)
    }
}

/// All reflected types of a world, by id, name and Rust type.
#[derive(Default)]
pub struct TypeRegistry {
    types: Vec<TypeReflect>,
    by_name: HashMap<Box<str>, ReflectId>,
    by_type: HashMap<TypeId, ReflectId>,
}

impl TypeRegistry {
    pub fn len(&self) -> usize {
        self.types.len()
    }

    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    pub fn id_of<T: 'static>(&self) -> Option<ReflectId> {
        self.by_type.get(&TypeId::of::<T>()).copied()
    }

    pub fn id_by_name(&self, name: &str) -> Option<ReflectId> {
        self.by_name.get(name).copied()
    }

    pub fn get(&self, id: ReflectId) -> &TypeReflect {
        &self.types[id.index()]
    }

    /// Registers `T`. Used by the derive; call [`Reflect::reflect_id`]
    /// instead, which makes registration idempotent.
    ///
    /// # Panics
    /// If a different type already uses `name`.
    pub fn register<T: 'static>(
        &mut self,
        name: &str,
        size: usize,
        align: usize,
        fields: Vec<FieldReflect>,
    ) -> ReflectId {
        assert!(
            !self.by_name.contains_key(name),
            "reflected type name `{name}` is used by two different types"
        );
        let id = ReflectId(u32::try_from(self.types.len()).expect("types"));
        self.types.push(TypeReflect {
            name: name.into(),
            size,
            align,
            fields: fields.into(),
        });
        self.by_name.insert(name.into(), id);
        self.by_type.insert(TypeId::of::<T>(), id);
        id
    }
}

/// How a type appears as a field of a reflected struct.
pub trait ReflectField {
    fn field_type(registry: &mut TypeRegistry) -> FieldType;
}

/// A type with a reflected layout, normally via `#[derive(Reflect)]`.
pub trait Reflect: ReflectField + Sized + 'static {
    /// Registers this type, and the structs it contains, returning its id.
    /// Calling it again returns the same id.
    fn reflect_id(registry: &mut TypeRegistry) -> ReflectId;
}

macro_rules! scalar_field {
    ($($t:ty => $v:ident),* $(,)?) => {$(
        impl ReflectField for $t {
            fn field_type(_: &mut TypeRegistry) -> FieldType {
                FieldType::$v
            }
        }
    )*};
}

scalar_field! {
    bool => Bool,
    i8 => I8, i16 => I16, i32 => I32, i64 => I64,
    u8 => U8, u16 => U16, u32 => U32, u64 => U64,
    f32 => F32, f64 => F64,
    Entity => Entity,
}

impl<T: ReflectField, const N: usize> ReflectField for [T; N] {
    fn field_type(registry: &mut TypeRegistry) -> FieldType {
        FieldType::Array {
            elem: Box::new(T::field_type(registry)),
            len: N,
            stride: size_of::<T>(),
        }
    }
}
