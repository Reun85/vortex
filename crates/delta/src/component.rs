use std::{alloc::Layout, any::TypeId, collections::HashMap, mem, ptr};

use crate::{blob_vec::DropFn, hooks::ComponentHooks, reflect::ReflectId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentId(u32);

impl ComponentId {
    /// For hosts that pass ids across an FFI boundary as plain integers.
    pub const fn from_index(index: u32) -> Self {
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    Table,
    /// Declared for forward compatibility; not implemented yet.
    SparseSet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Native,
    ScriptUnmanaged,
    ScriptManaged,
    Tag,
}

pub type CloneFn = unsafe fn(*const u8, *mut u8);

/// Everything the storage needs to know about a component, as plain data.
#[derive(Clone, Debug)]
pub struct ComponentInfo {
    pub name: Box<str>,
    pub layout: Layout,
    pub drop: Option<DropFn>,
    pub clone: Option<CloneFn>,
    pub kind: Kind,
    pub storage: Storage,
    pub hooks: ComponentHooks,
    /// Field layout for the inspector, undo and scene files, if reflected.
    pub reflect: Option<ReflectId>,
}

#[derive(Default)]
pub struct Components {
    infos: Vec<ComponentInfo>,
    by_name: HashMap<Box<str>, ComponentId>,
    by_type: HashMap<TypeId, ComponentId>,
    any_hooks: bool,
}

impl Components {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.infos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.infos.is_empty()
    }

    /// Registers a component described only by data. Registering a name that
    /// already exists returns the existing id (and keeps the old info).
    pub fn register_raw(&mut self, info: ComponentInfo) -> ComponentId {
        if let Some(&id) = self.by_name.get(&info.name) {
            return id;
        }
        let id = ComponentId(
            u32::try_from(self.infos.len()).expect("too many components"),
        );
        self.by_name.insert(info.name.clone(), id);
        self.infos.push(info);
        id
    }

    /// Registers a Rust type under `std::any::type_name`.
    pub fn register<T: Send + Sync + 'static>(&mut self) -> ComponentId {
        if let Some(&id) = self.by_type.get(&TypeId::of::<T>()) {
            return id;
        }
        self.register_named::<T>(std::any::type_name::<T>(), None)
    }

    /// Registers a Rust type under a stable `name`. If the type was already
    /// registered (for instance by a plain `insert`), it is renamed and keeps
    /// its id, so the stable name always wins.
    ///
    /// # Panics
    /// If `name` already belongs to a different component.
    pub fn register_named<T: Send + Sync + 'static>(
        &mut self,
        name: &str,
        reflect: Option<ReflectId>,
    ) -> ComponentId {
        if let Some(&id) = self.by_type.get(&TypeId::of::<T>()) {
            if &*self.infos[id.index()].name != name {
                assert!(
                    !self.by_name.contains_key(name),
                    "component name `{name}` is used by another component"
                );
                let old =
                    mem::replace(&mut self.infos[id.index()].name, name.into());
                self.by_name.remove(&old);
                self.by_name.insert(name.into(), id);
            }
            if reflect.is_some() {
                self.infos[id.index()].reflect = reflect;
            }
            return id;
        }
        assert!(
            !self.by_name.contains_key(name),
            "component name `{name}` is used by another component"
        );
        let layout = Layout::new::<T>();
        let id = self.register_raw(ComponentInfo {
            name: name.into(),
            layout,
            drop: std::mem::needs_drop::<T>().then_some(drop_fn::<T>),
            clone: None,
            kind: if layout.size() == 0 {
                Kind::Tag
            } else {
                Kind::Native
            },
            storage: Storage::Table,
            hooks: ComponentHooks::default(),
            reflect,
        });
        self.by_type.insert(TypeId::of::<T>(), id);
        id
    }

    /// Like [`register`](Self::register) but also records a clone function.
    pub fn register_cloneable<T: Clone + Send + Sync + 'static>(
        &mut self,
    ) -> ComponentId {
        let id = self.register::<T>();
        self.infos[id.index()].clone = Some(clone_fn::<T>);
        id
    }

    /// Whether any component has a hook; lets structural operations skip all
    /// hook bookkeeping in the common case.
    pub fn any_hooks(&self) -> bool {
        self.any_hooks
    }

    pub fn hooks(&self, id: ComponentId) -> ComponentHooks {
        self.infos[id.index()].hooks
    }

    pub fn hooks_mut(&mut self, id: ComponentId) -> &mut ComponentHooks {
        self.any_hooks = true;
        &mut self.infos[id.index()].hooks
    }

    pub fn id_of<T: 'static>(&self) -> Option<ComponentId> {
        self.by_type.get(&TypeId::of::<T>()).copied()
    }

    pub fn id_by_name(&self, name: &str) -> Option<ComponentId> {
        self.by_name.get(name).copied()
    }

    /// Mutable access to a component's description, for hosts that redefine
    /// a script component on hot reload.
    ///
    /// # Safety
    /// If the layout, kind or drop function change, the caller must rebuild
    /// every column of this component (see `World::rebuild_columns`) before
    /// the world is used again.
    pub unsafe fn info_mut(&mut self, id: ComponentId) -> &mut ComponentInfo {
        &mut self.infos[id.index()]
    }

    pub fn info(&self, id: ComponentId) -> &ComponentInfo {
        &self.infos[id.index()]
    }

    pub fn get(&self, id: ComponentId) -> Option<&ComponentInfo> {
        self.infos.get(id.index())
    }
}

pub(crate) unsafe fn drop_fn<T>(p: *mut u8) {
    // SAFETY: caller guarantees `p` points to an initialized `T`.
    unsafe { ptr::drop_in_place(p.cast::<T>()) }
}

unsafe fn clone_fn<T: Clone>(src: *const u8, dst: *mut u8) {
    // SAFETY: caller guarantees `src` is an initialized `T` and `dst` is
    // valid, aligned, uninitialized memory for one `T`.
    unsafe { dst.cast::<T>().write((*src.cast::<T>()).clone()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_idempotent() {
        let mut c = Components::new();
        let a = c.register::<u32>();
        assert_eq!(c.register::<u32>(), a);
        let b = c.register::<String>();
        assert_ne!(a, b);
        assert_eq!(c.id_of::<String>(), Some(b));
        assert_eq!(c.id_by_name(&c.info(a).name), Some(a));
        assert!(c.info(b).drop.is_some());
        assert!(c.info(a).drop.is_none());
    }

    #[test]
    fn zero_sized_is_tag() {
        struct Marker;
        let mut c = Components::new();
        let m = c.register::<Marker>();
        assert_eq!(c.info(m).kind, Kind::Tag);
    }

    #[test]
    fn clone_fn_works() {
        let mut c = Components::new();
        let s = c.register_cloneable::<String>();
        let src = String::from("hi");
        let mut dst = std::mem::MaybeUninit::<String>::uninit();
        // SAFETY: src is a valid String, dst is valid uninit storage.
        let out = unsafe {
            (c.info(s).clone.unwrap())(
                (&raw const src).cast(),
                dst.as_mut_ptr().cast(),
            );
            dst.assume_init()
        };
        assert_eq!(out, "hi");
    }
}
