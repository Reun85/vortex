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
    /// For hosts that pass ids across an FFI boundary as plain integers.
    pub const fn from_index(index: u32) -> Self {
        Self(index)
    }

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
#[derive(Clone, Default)]
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

    /// Registers a type known only by data, such as a script-declared
    /// struct. Registering the same name with the same layout again returns
    /// the existing id.
    ///
    /// # Errors
    /// If `name` is already registered with a different layout.
    pub fn register_dynamic(
        &mut self,
        name: &str,
        size: usize,
        align: usize,
        fields: Vec<FieldReflect>,
    ) -> Result<ReflectId, String> {
        if let Some(&id) = self.by_name.get(name) {
            let existing = &self.types[id.index()];
            return if existing.size == size
                && existing.align == align
                && *existing.fields == *fields
            {
                Ok(id)
            } else {
                Err(format!(
                    "type `{name}` is already registered with a different layout"
                ))
            };
        }
        let id = ReflectId(u32::try_from(self.types.len()).expect("types"));
        self.types.push(TypeReflect {
            name: name.into(),
            size,
            align,
            fields: fields.into(),
        });
        self.by_name.insert(name.into(), id);
        Ok(id)
    }

    /// Like [`register_dynamic`](Self::register_dynamic), but a type with
    /// the same name and a different layout is *redefined* and keeps its id,
    /// which is what a script hot reload needs. Take a [`Clone`] of the
    /// registry first to keep the old definitions for migration.
    pub fn replace_dynamic(
        &mut self,
        name: &str,
        size: usize,
        align: usize,
        fields: Vec<FieldReflect>,
    ) -> ReflectId {
        if let Some(&id) = self.by_name.get(name) {
            self.types[id.index()] = TypeReflect {
                name: name.into(),
                size,
                align,
                fields: fields.into(),
            };
            return id;
        }
        self.register_dynamic(name, size, align, fields)
            .expect("a fresh name cannot conflict")
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

impl FieldType {
    /// The size in bytes of a value of this type.
    pub fn size(&self, registry: &TypeRegistry) -> usize {
        match self {
            Self::Bool | Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 | Self::Entity => 8,
            Self::Array { len, stride, .. } => len * stride,
            Self::Struct(id) => registry.get(*id).size,
        }
    }
}

/// Whether two types, each resolved in its own registry, describe exactly
/// the same layout: same fields, names, offsets and types, all the way down.
/// Ids are not compared, so this works across a hot reload.
pub fn types_equal(
    a_registry: &TypeRegistry,
    a: &FieldType,
    b_registry: &TypeRegistry,
    b: &FieldType,
) -> bool {
    match (a, b) {
        (FieldType::Struct(x), FieldType::Struct(y)) => {
            let (ta, tb) = (a_registry.get(*x), b_registry.get(*y));
            ta.size == tb.size
                && ta.align == tb.align
                && ta.fields.len() == tb.fields.len()
                && ta.fields.iter().zip(tb.fields.iter()).all(|(fa, fb)| {
                    fa.name == fb.name
                        && fa.offset == fb.offset
                        && types_equal(a_registry, &fa.ty, b_registry, &fb.ty)
                })
        }
        (
            FieldType::Array {
                elem: ea,
                len: la,
                stride: sa,
            },
            FieldType::Array {
                elem: eb,
                len: lb,
                stride: sb,
            },
        ) => {
            la == lb && sa == sb && types_equal(a_registry, ea, b_registry, eb)
        }
        (FieldType::Struct(_), _)
        | (_, FieldType::Struct(_))
        | (FieldType::Array { .. }, _)
        | (_, FieldType::Array { .. }) => false,
        _ => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

/// The registry a value's old layout is described in, and the one its new
/// layout is described in; they differ across a hot reload.
#[derive(Clone, Copy)]
pub struct RegistryPair<'a> {
    pub old: &'a TypeRegistry,
    pub new: &'a TypeRegistry,
}

/// Converts a value from an old layout to a new one: fields are matched by
/// name, copied where their types agree, left at the zeroed default when
/// they are new, and dropped (with a warning) when they are gone. A field
/// whose type changed is reset to its default, also with a warning.
///
/// # Safety
/// `old` points to a valid value of `old_ty`; `new` to zeroed memory with
/// room for `new_ty`.
pub unsafe fn migrate_value(
    registries: RegistryPair<'_>,
    old_ty: &FieldType,
    old: *const u8,
    new_ty: &FieldType,
    new: *mut u8,
    path: &str,
    warnings: &mut Vec<String>,
) {
    let (old_registry, new_registry) = (registries.old, registries.new);
    match (old_ty, new_ty) {
        (FieldType::Struct(o), FieldType::Struct(n)) => {
            let (to, tn) = (old_registry.get(*o), new_registry.get(*n));
            for nf in tn.fields.iter() {
                let Some(of) = to.field(&nf.name) else {
                    continue; // new field: stays at its zeroed default
                };
                let sub = format!("{path}.{}", nf.name);
                // SAFETY: offsets come from the types' own descriptions.
                unsafe {
                    migrate_value(
                        registries,
                        &of.ty,
                        old.add(of.offset),
                        &nf.ty,
                        new.add(nf.offset),
                        &sub,
                        warnings,
                    );
                }
            }
            for of in to.fields.iter() {
                if tn.field(&of.name).is_none() {
                    warnings.push(format!(
                        "{path}.{}: field was removed, its value is dropped",
                        of.name
                    ));
                }
            }
        }
        (
            FieldType::Array {
                elem: eo,
                len: lo,
                stride: so,
            },
            FieldType::Array {
                elem: en,
                len: ln,
                stride: sn,
            },
        ) => {
            for i in 0..(*lo).min(*ln) {
                // SAFETY: element `i` is inside both arrays.
                unsafe {
                    migrate_value(
                        registries,
                        eo,
                        old.add(i * so),
                        en,
                        new.add(i * sn),
                        &format!("{path}[{i}]"),
                        warnings,
                    );
                }
            }
            if lo != ln {
                warnings.push(format!(
                    "{path}: array length changed from {lo} to {ln}"
                ));
            }
        }
        (a, b)
            if !matches!(a, FieldType::Struct(_) | FieldType::Array { .. })
                && std::mem::discriminant(a) == std::mem::discriminant(b) =>
        {
            // SAFETY: both sides hold a value of this primitive type.
            unsafe {
                std::ptr::copy_nonoverlapping(old, new, a.size(old_registry));
            }
        }
        (a, b) => warnings.push(format!(
            "{path}: type changed from {a:?} to {b:?}, reset to the default"
        )),
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    fn field(name: &str, offset: usize, ty: FieldType) -> FieldReflect {
        FieldReflect::new(name, offset, ty, FieldAttrs::default())
    }

    /// `Vec { x: f32, y: f32 }` and `Body { pos: Vec, mass: i32 }`.
    fn v1() -> (TypeRegistry, FieldType) {
        let mut r = TypeRegistry::default();
        let v = r
            .register_dynamic(
                "Vec",
                8,
                4,
                vec![
                    field("x", 0, FieldType::F32),
                    field("y", 4, FieldType::F32),
                ],
            )
            .unwrap();
        let b = r
            .register_dynamic(
                "Body",
                12,
                4,
                vec![
                    field("pos", 0, FieldType::Struct(v)),
                    field("mass", 8, FieldType::I32),
                ],
            )
            .unwrap();
        (r, FieldType::Struct(b))
    }

    /// `Vec` gains `z`; `Body` gains `tag` and loses nothing.
    fn v2(mut r: TypeRegistry) -> (TypeRegistry, FieldType) {
        let v = r.replace_dynamic(
            "Vec",
            12,
            4,
            vec![
                field("x", 0, FieldType::F32),
                field("y", 4, FieldType::F32),
                field("z", 8, FieldType::F32),
            ],
        );
        let b = r.replace_dynamic(
            "Body",
            20,
            4,
            vec![
                field("pos", 0, FieldType::Struct(v)),
                field("mass", 12, FieldType::I32),
                field("tag", 16, FieldType::U32),
            ],
        );
        (r, FieldType::Struct(b))
    }

    #[test]
    fn replace_keeps_the_id_and_old_clone_keeps_the_old_layout() {
        let (old, old_ty) = v1();
        let (new, new_ty) = v2(old.clone());
        assert_eq!(old_ty, new_ty, "same id after redefinition");
        assert_eq!(old.get(ReflectId(1)).size, 12);
        assert_eq!(new.get(ReflectId(1)).size, 20);
        assert!(!types_equal(&old, &old_ty, &new, &new_ty));
        assert!(types_equal(&old, &old_ty, &old.clone(), &old_ty));
    }

    #[test]
    fn migrates_by_name_through_nested_structs() {
        let (old, old_ty) = v1();
        let (new, new_ty) = v2(old.clone());
        // Body { pos: (1.5, -2.5), mass: 77 }
        let mut old_bytes = [0u8; 12];
        old_bytes[0..4].copy_from_slice(&1.5f32.to_ne_bytes());
        old_bytes[4..8].copy_from_slice(&(-2.5f32).to_ne_bytes());
        old_bytes[8..12].copy_from_slice(&77i32.to_ne_bytes());
        let mut new_bytes = [0u8; 20];
        let mut warnings = Vec::new();
        // SAFETY: buffers match their types.
        unsafe {
            migrate_value(
                RegistryPair {
                    old: &old,
                    new: &new,
                },
                &old_ty,
                old_bytes.as_ptr(),
                &new_ty,
                new_bytes.as_mut_ptr(),
                "Body",
                &mut warnings,
            );
        }
        assert!(warnings.is_empty(), "{warnings:?}");
        let f = |at: usize| {
            f32::from_ne_bytes(new_bytes[at..at + 4].try_into().unwrap())
        };
        assert_eq!((f(0), f(4), f(8)), (1.5, -2.5, 0.0), "z defaults to zero");
        assert_eq!(
            i32::from_ne_bytes(new_bytes[12..16].try_into().unwrap()),
            77
        );
        assert_eq!(&new_bytes[16..20], &[0; 4], "new field is zero");
    }

    #[test]
    fn removed_fields_and_changed_types_warn() {
        let mut old = TypeRegistry::default();
        let o = old
            .register_dynamic(
                "S",
                12,
                4,
                vec![
                    field("keep", 0, FieldType::I32),
                    field("retype", 4, FieldType::F32),
                    field("gone", 8, FieldType::I32),
                ],
            )
            .unwrap();
        let mut new = old.clone();
        let n = new.replace_dynamic(
            "S",
            16,
            8,
            vec![
                field("keep", 0, FieldType::I32),
                field("retype", 8, FieldType::F64),
            ],
        );
        let mut old_bytes = [0u8; 12];
        old_bytes[0..4].copy_from_slice(&5i32.to_ne_bytes());
        old_bytes[4..8].copy_from_slice(&9.0f32.to_ne_bytes());
        old_bytes[8..12].copy_from_slice(&3i32.to_ne_bytes());
        let mut new_bytes = [0u8; 16];
        let mut warnings = Vec::new();
        // SAFETY: buffers match their types.
        unsafe {
            migrate_value(
                RegistryPair {
                    old: &old,
                    new: &new,
                },
                &FieldType::Struct(o),
                old_bytes.as_ptr(),
                &FieldType::Struct(n),
                new_bytes.as_mut_ptr(),
                "S",
                &mut warnings,
            );
        }
        assert_eq!(i32::from_ne_bytes(new_bytes[0..4].try_into().unwrap()), 5);
        assert_eq!(&new_bytes[8..16], &[0; 8], "retyped field was reset");
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("S.gone") && w.contains("removed"))
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("S.retype") && w.contains("type changed"))
        );
    }

    #[test]
    fn arrays_copy_the_common_prefix() {
        let arr = |len| FieldType::Array {
            elem: Box::new(FieldType::U16),
            len,
            stride: 2,
        };
        let r = TypeRegistry::default();
        let old = [1u16, 2, 3, 4];
        let mut new = [0u16; 2];
        let mut warnings = Vec::new();
        // SAFETY: buffers match their types.
        unsafe {
            migrate_value(
                RegistryPair { old: &r, new: &r },
                &arr(4),
                old.as_ptr().cast(),
                &arr(2),
                new.as_mut_ptr().cast(),
                "a",
                &mut warnings,
            );
        }
        assert_eq!(new, [1, 2]);
        assert_eq!(warnings.len(), 1);
    }
}
