//! Reading and writing components through their reflection data: the
//! mechanism under the inspector, undo and copy/paste.
//!
//! Components are addressed by their stable name and fields by a path such
//! as `position.x` or `rotation[2]`. Values are JSON in the same form scene
//! files use, so entity references are [`SceneId`] strings.

use std::{collections::HashMap, fmt};

use serde_json::Value;

use crate::{
    component::ComponentId,
    entity::Entity,
    reflect::{FieldType, ReflectId, TypeRegistry},
    scene::{WriteCtx, ZeroedBuf, read_value, write_value},
    world::World,
};

#[derive(Debug, PartialEq)]
pub enum InspectError {
    DeadEntity(Entity),
    UnknownComponent(String),
    NotReflected(String),
    /// The entity lacks the component.
    Missing {
        component: String,
    },
    BadPath(String),
    /// The value does not fit the field.
    BadValue(String),
}

impl fmt::Display for InspectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeadEntity(e) => write!(f, "entity {e:?} is not alive"),
            Self::UnknownComponent(n) => write!(f, "unknown component `{n}`"),
            Self::NotReflected(n) => {
                write!(f, "component `{n}` has no reflection data")
            }
            Self::Missing { component } => {
                write!(f, "the entity has no `{component}`")
            }
            Self::BadPath(m) => write!(f, "bad field path: {m}"),
            Self::BadValue(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for InspectError {}

/// A field inside a component: its byte offset and its type.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedField {
    pub offset: usize,
    pub ty: FieldType,
}

/// Follows `path` (`a.b[2].c`; empty means the whole component) from the
/// reflected struct `root`.
pub fn resolve_path(
    registry: &TypeRegistry,
    root: ReflectId,
    path: &str,
) -> Result<ResolvedField, InspectError> {
    let bad = |m: String| InspectError::BadPath(format!("`{path}`: {m}"));
    let mut field = ResolvedField {
        offset: 0,
        ty: FieldType::Struct(root),
    };
    let mut rest = path;
    let mut first = true;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('[') {
            let end =
                after.find(']').ok_or_else(|| bad("missing `]`".into()))?;
            let index: usize = after[..end].parse().map_err(|_| {
                bad(format!("`{}` is not an index", &after[..end]))
            })?;
            let FieldType::Array { elem, len, stride } = field.ty else {
                return Err(bad(
                    "indexing something that is not an array".into()
                ));
            };
            if index >= len {
                return Err(bad(format!(
                    "index {index} is out of range 0..{len}"
                )));
            }
            field = ResolvedField {
                offset: field.offset + index * stride,
                ty: *elem,
            };
            rest = &after[end + 1..];
        } else {
            let name_part = if first {
                rest
            } else {
                rest.strip_prefix('.').ok_or_else(|| {
                    bad(format!("expected `.` before `{rest}`"))
                })?
            };
            let end = name_part.find(['.', '[']).unwrap_or(name_part.len());
            let name = &name_part[..end];
            let FieldType::Struct(id) = field.ty else {
                return Err(bad(format!(
                    "`{name}` is not a field of a non-struct value"
                )));
            };
            let f = registry.get(id).field(name).ok_or_else(|| {
                bad(format!("no field `{name}` in {}", registry.get(id).name))
            })?;
            field = ResolvedField {
                offset: field.offset + f.offset,
                ty: f.ty.clone(),
            };
            rest = &name_part[end..];
        }
        first = false;
    }
    Ok(field)
}

/// Byte offsets of every `Entity` inside a value of type `ty`.
fn entity_offsets(
    registry: &TypeRegistry,
    ty: &FieldType,
    base: usize,
    out: &mut Vec<usize>,
) {
    match ty {
        FieldType::Entity => out.push(base),
        FieldType::Array { elem, len, stride } => {
            for i in 0..*len {
                entity_offsets(registry, elem, base + i * stride, out);
            }
        }
        FieldType::Struct(id) => {
            for f in registry.get(*id).fields.iter() {
                entity_offsets(registry, &f.ty, base + f.offset, out);
            }
        }
        _ => {}
    }
}

impl World {
    fn reflected_component(
        &self,
        name: &str,
    ) -> Result<(ComponentId, ReflectId), InspectError> {
        let c = self
            .components
            .id_by_name(name)
            .ok_or_else(|| InspectError::UnknownComponent(name.to_owned()))?;
        let reflect = self
            .components
            .info(c)
            .reflect
            .ok_or_else(|| InspectError::NotReflected(name.to_owned()))?;
        Ok((c, reflect))
    }

    /// Pointer to the component's value on `e`.
    fn component_ptr(
        &self,
        e: Entity,
        c: ComponentId,
        name: &str,
    ) -> Result<*mut u8, InspectError> {
        let loc = self.location(e).ok_or(InspectError::DeadEntity(e))?;
        let column =
            self.archetypes
                .get(loc.archetype)
                .column(c)
                .ok_or_else(|| InspectError::Missing {
                    component: name.to_owned(),
                })?;
        Ok(column.get_ptr(loc.row))
    }

    /// A whole component as JSON, as it appears in a scene file.
    pub fn component_value(
        &self,
        e: Entity,
        component: &str,
    ) -> Result<Value, InspectError> {
        self.field_value(e, component, "")
    }

    /// One field of a component as JSON.
    pub fn field_value(
        &self,
        e: Entity,
        component: &str,
        path: &str,
    ) -> Result<Value, InspectError> {
        let (c, reflect) = self.reflected_component(component)?;
        let field = resolve_path(&self.reflection, reflect, path)?;
        let base = self.component_ptr(e, c, component)?;
        // SAFETY: the resolved offset lies inside the component's value.
        let ptr = unsafe { base.add(field.offset) };
        Ok(read_value(&self.reflection, &field.ty, ptr, &|r| {
            self.entity_ref_to_json(r)
        }))
    }

    /// Overwrites one field (or the whole component, with an empty path) and
    /// returns the previous value. A partial object leaves the fields it
    /// omits as they are. The component is marked changed. Nothing is
    /// modified if the value does not fit.
    pub fn set_field_value(
        &mut self,
        e: Entity,
        component: &str,
        path: &str,
        value: &Value,
    ) -> Result<Value, InspectError> {
        let (c, reflect) = self.reflected_component(component)?;
        let field = resolve_path(&self.reflection, reflect, path)?;
        let old = self.field_value(e, component, path)?;
        let size = field.ty.size(&self.reflection);

        // Build the new bytes aside, starting from the current ones, so a bad
        // value cannot leave the component half written.
        let base = self.component_ptr(e, c, component)?;
        let mut scratch = vec![0u8; size];
        // SAFETY: the field lies inside the component's value.
        unsafe {
            std::ptr::copy_nonoverlapping(
                base.add(field.offset),
                scratch.as_mut_ptr(),
                size,
            );
        }
        let mut warnings = Vec::new();
        let mut ctx = WriteCtx {
            scene_ids: &self.scene_ids,
            retired: self.retired.as_ref().map(|r| &r.by_id),
            warnings: &mut warnings,
        };
        write_value(
            &self.reflection,
            &field.ty,
            value,
            scratch.as_mut_ptr(),
            &mut ctx,
            component,
        )
        .map_err(InspectError::BadValue)?;

        let loc = self.location(e).ok_or(InspectError::DeadEntity(e))?;
        let tick = self.change_tick;
        let column = self
            .archetypes
            .get_mut(loc.archetype)
            .column_mut(c)
            .expect("the component was found above");
        // SAFETY: same region as read above; `scratch` holds `size` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(
                scratch.as_ptr(),
                column.get_ptr(loc.row).add(field.offset),
                size,
            );
        }
        column.set_changed(loc.row, tick);
        Ok(old)
    }

    /// When the component on `e` was last written, for refreshing only what
    /// changed.
    pub fn component_changed_tick(
        &self,
        e: Entity,
        component: &str,
    ) -> Result<crate::tick::Tick, InspectError> {
        let (c, _) = self.reflected_component(component)?;
        let loc = self.location(e).ok_or(InspectError::DeadEntity(e))?;
        self.archetypes
            .get(loc.archetype)
            .column(c)
            .map(|col| col.changed(loc.row))
            .ok_or_else(|| InspectError::Missing {
                component: component.to_owned(),
            })
    }

    /// Every live entity, in no particular order.
    pub fn all_entities(&self) -> Vec<Entity> {
        self.archetypes
            .iter()
            .flat_map(|a| a.entities().iter().copied())
            .collect()
    }

    /// A component value with every field zero, as a new component would
    /// start out.
    pub fn default_component_value(
        &self,
        component: &str,
    ) -> Result<Value, InspectError> {
        let (c, reflect) = self.reflected_component(component)?;
        let buffer = ZeroedBuf::new(self.components.info(c).layout);
        Ok(read_value(
            &self.reflection,
            &FieldType::Struct(reflect),
            buffer.ptr,
            &|_| Value::Null,
        ))
    }

    /// Adds (or overwrites) a component from its JSON form. Fields the value
    /// omits are zero.
    pub fn insert_component_value(
        &mut self,
        e: Entity,
        component: &str,
        value: &Value,
    ) -> Result<(), InspectError> {
        let (c, reflect) = self.reflected_component(component)?;
        let buffer = ZeroedBuf::new(self.components.info(c).layout);
        let mut warnings = Vec::new();
        let mut ctx = WriteCtx {
            scene_ids: &self.scene_ids,
            retired: self.retired.as_ref().map(|r| &r.by_id),
            warnings: &mut warnings,
        };
        write_value(
            &self.reflection,
            &FieldType::Struct(reflect),
            value,
            buffer.ptr,
            &mut ctx,
            component,
        )
        .map_err(InspectError::BadValue)?;
        // SAFETY: the buffer holds a fully initialized value of the
        // component's layout; reflected data has no drop glue.
        unsafe { self.insert_by_id(e, c, buffer.ptr.cast_const()) }
            .map_err(|_| InspectError::DeadEntity(e))
    }

    /// Removes a component by its stable name. Returns whether it was there.
    pub fn remove_component_named(
        &mut self,
        e: Entity,
        component: &str,
    ) -> Result<bool, InspectError> {
        let c = self.components.id_by_name(component).ok_or_else(|| {
            InspectError::UnknownComponent(component.to_owned())
        })?;
        self.remove_by_id(e, c)
            .map_err(|_| InspectError::DeadEntity(e))
    }

    /// Names of the entity's components that have reflection data.
    pub fn reflected_components(&self, e: Entity) -> Vec<String> {
        let Some(loc) = self.location(e) else {
            return Vec::new();
        };
        let mut names: Vec<_> = self
            .archetypes
            .get(loc.archetype)
            .components()
            .iter()
            .map(|&c| self.components.info(c))
            .filter(|i| i.reflect.is_some())
            .map(|i| i.name.to_string())
            .collect();
        names.sort();
        names
    }

    /// Rewrites every `Entity` field in the world that holds `from` to hold
    /// `to`, and marks those components changed. Returns how many fields
    /// changed.
    ///
    /// This is how undoing a delete re-points everything that referenced the
    /// entity: the restored entity has a new handle, and the references left
    /// behind still hold the old one.
    pub fn remap_entity_references(
        &mut self,
        from: Entity,
        to: Entity,
    ) -> usize {
        let tick = self.change_tick;
        let mut offsets_by_type: HashMap<ReflectId, Vec<usize>> =
            HashMap::new();
        let mut rewritten = 0;
        for arch in self.archetypes.iter_mut() {
            let (components, columns) = arch.components_and_columns_mut();
            for (&c, column) in components.iter().zip(columns.iter_mut()) {
                let Some(reflect) = self.components.info(c).reflect else {
                    continue;
                };
                let offsets =
                    offsets_by_type.entry(reflect).or_insert_with(|| {
                        let mut out = Vec::new();
                        entity_offsets(
                            &self.reflection,
                            &FieldType::Struct(reflect),
                            0,
                            &mut out,
                        );
                        out
                    });
                if offsets.is_empty() {
                    continue;
                }
                for row in 0..column.len() {
                    let mut touched = false;
                    for &offset in offsets.iter() {
                        // SAFETY: `offset` is an `Entity` field of this
                        // component's value, from its reflection data.
                        unsafe {
                            let p = column
                                .get_ptr(row)
                                .add(offset)
                                .cast::<Entity>();
                            if p.read_unaligned() == from {
                                p.write_unaligned(to);
                                touched = true;
                                rewritten += 1;
                            }
                        }
                    }
                    if touched {
                        column.set_changed(row, tick);
                    }
                }
            }
        }
        rewritten
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflect::{FieldAttrs, FieldReflect};

    fn registry() -> (TypeRegistry, ReflectId) {
        let mut r = TypeRegistry::default();
        let f =
            |n: &str, o, ty| FieldReflect::new(n, o, ty, FieldAttrs::default());
        let vec = r
            .register_dynamic(
                "Vec",
                8,
                4,
                vec![f("x", 0, FieldType::F32), f("y", 4, FieldType::F32)],
            )
            .unwrap();
        let t = r
            .register_dynamic(
                "T",
                8 + 16 + 4,
                4,
                vec![
                    f("pos", 0, FieldType::Struct(vec)),
                    f(
                        "rot",
                        8,
                        FieldType::Array {
                            elem: Box::new(FieldType::F32),
                            len: 4,
                            stride: 4,
                        },
                    ),
                    f("n", 24, FieldType::I32),
                ],
            )
            .unwrap();
        (r, t)
    }

    #[test]
    fn paths_resolve_through_structs_and_arrays() {
        let (r, t) = registry();
        let at = |p: &str| resolve_path(&r, t, p).unwrap();
        assert_eq!(at("").offset, 0);
        assert_eq!((at("pos.y").offset, at("pos.y").ty), (4, FieldType::F32));
        assert_eq!(at("rot[3]").offset, 8 + 12);
        assert_eq!(at("n").ty, FieldType::I32);
        assert!(matches!(at("pos").ty, FieldType::Struct(_)));
    }

    #[test]
    fn bad_paths_are_rejected_with_a_reason() {
        let (r, t) = registry();
        let err = |p: &str| resolve_path(&r, t, p).unwrap_err().to_string();
        assert!(err("nope").contains("no field `nope`"));
        assert!(err("rot[4]").contains("out of range"));
        assert!(err("rot[x]").contains("not an index"));
        assert!(err("n[0]").contains("not an array"));
        assert!(err("n.z").contains("non-struct"));
        assert!(err("rot[1").contains("missing `]`"));
        assert!(err("pos x").contains("no field"));
    }
}
