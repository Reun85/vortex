use std::{
    alloc::{self, Layout},
    mem::{self, ManuallyDrop},
    ptr,
};

use crate::{
    blob_vec::DropFn,
    component::{ComponentId, Components, drop_fn},
    entity::{Entities, Entity},
    observer::Event,
    system::{Access, SystemParam},
    tick::Tick,
    world::{World, WorldCell},
};

const OP_INSERT: u8 = 0;
const OP_REMOVE: u8 = 1;
const OP_DESPAWN: u8 = 2;
const OP_CUSTOM: u8 = 3;

const REF_ID: u8 = 0;
const REF_RESOLVER: u8 = 1;

/// Turns a Rust type into its component id at apply time, when the registry
/// is reachable. Lets typed commands be recorded without registry access.
type Resolver = fn(&mut Components) -> ComponentId;

type Custom = Box<dyn FnOnce(&mut World)>;

fn resolve<T: Send + Sync + 'static>(
    components: &mut Components,
) -> ComponentId {
    components.register::<T>()
}

/// How a recorded command names its component.
#[derive(Clone, Copy)]
enum ComponentRef {
    Id(ComponentId),
    Resolver(Resolver),
}

impl ComponentRef {
    fn resolve(self, components: &mut Components) -> ComponentId {
        match self {
            Self::Id(c) => c,
            Self::Resolver(f) => f(components),
        }
    }
}

/// A flat, ordered byte log of structural changes, applied in one pass at a
/// sync point.
///
/// Records are `(opcode, entity, component, payload bytes)`. Consecutive
/// inserts on one entity are applied as a single archetype move. Because the
/// payload is plain bytes, other languages can append to the same format.
#[derive(Default)]
pub struct CommandQueue {
    bytes: Vec<u8>,
    customs: Vec<Custom>,
    ops: usize,
}

impl CommandQueue {
    pub fn is_empty(&self) -> bool {
        self.ops == 0
    }

    /// Number of recorded commands.
    pub fn len(&self) -> usize {
        self.ops
    }

    /// Queues arbitrary work with full world access. Runs in order with the
    /// other commands.
    pub fn push(&mut self, command: impl FnOnce(&mut World) + 'static) {
        self.bytes.push(OP_CUSTOM);
        self.customs.push(Box::new(command));
        self.ops += 1;
    }

    pub fn despawn(&mut self, entity: Entity) {
        self.bytes.push(OP_DESPAWN);
        put_u64(&mut self.bytes, entity.to_bits());
        self.ops += 1;
    }

    pub fn insert<T: Send + Sync + 'static>(
        &mut self,
        entity: Entity,
        value: T,
    ) {
        let value = ManuallyDrop::new(value);
        let drop = mem::needs_drop::<T>().then_some(drop_fn::<T> as DropFn);
        // SAFETY: `value` is a valid `T` whose ownership moves into the log;
        // the resolver registers exactly `T`.
        unsafe {
            self.push_insert(
                entity,
                ComponentRef::Resolver(resolve::<T>),
                Layout::new::<T>(),
                drop,
                (&raw const *value).cast(),
            );
        }
    }

    pub fn remove<T: Send + Sync + 'static>(&mut self, entity: Entity) {
        self.push_remove(entity, ComponentRef::Resolver(resolve::<T>));
    }

    /// Records an insert of an already registered component, for callers that
    /// know ids only (scripts, tools).
    ///
    /// # Safety
    /// `src` must point to an initialized value with `layout`, matching `c`,
    /// and `drop` must be `c`'s drop function. Ownership of the value moves
    /// into the queue.
    pub unsafe fn insert_by_id(
        &mut self,
        entity: Entity,
        c: ComponentId,
        layout: Layout,
        drop: Option<DropFn>,
        src: *const u8,
    ) {
        // SAFETY: forwarded contract.
        unsafe {
            self.push_insert(entity, ComponentRef::Id(c), layout, drop, src);
        }
    }

    pub fn remove_by_id(&mut self, entity: Entity, c: ComponentId) {
        self.push_remove(entity, ComponentRef::Id(c));
    }

    /// # Safety
    /// See [`insert_by_id`](Self::insert_by_id).
    unsafe fn push_insert(
        &mut self,
        entity: Entity,
        component: ComponentRef,
        layout: Layout,
        drop: Option<DropFn>,
        src: *const u8,
    ) {
        let buf = &mut self.bytes;
        buf.push(OP_INSERT);
        put_u64(buf, entity.to_bits());
        put_ref(buf, component);
        put_usize(buf, layout.size());
        put_usize(buf, layout.align());
        put_value(buf, drop);
        // SAFETY: `src` is valid for `layout.size()` bytes per the contract.
        buf.extend_from_slice(unsafe {
            std::slice::from_raw_parts(src, layout.size())
        });
        self.ops += 1;
    }

    fn push_remove(&mut self, entity: Entity, component: ComponentRef) {
        self.bytes.push(OP_REMOVE);
        put_u64(&mut self.bytes, entity.to_bits());
        put_ref(&mut self.bytes, component);
        self.ops += 1;
    }

    /// Applies every command in order and empties the queue. Entities
    /// reserved while recording become alive first, so commands may target
    /// them even if they have no other command.
    pub fn apply(&mut self, world: &mut World) {
        world.apply_depth += 1;
        self.apply_inner(world);
        world.apply_depth -= 1;
        // Hooks and observers may have queued more work; it runs before the
        // sync point ends, in rounds.
        world.finish_op();
    }

    /// Applies the commands without running the work they spawn; the caller
    /// is responsible for `World::flush_deferred`.
    pub(crate) fn apply_inner(&mut self, world: &mut World) {
        world.flush_entities();
        let bytes = mem::take(&mut self.bytes);
        let mut customs = mem::take(&mut self.customs).into_iter();
        self.ops = 0;

        let mut reader = Reader::new(&bytes);
        let mut batch = Batch::default();
        while !reader.done() {
            match reader.op() {
                Op::Insert(rec) => {
                    let c = rec.component.resolve(world.components_mut());
                    if batch.entity != Some(rec.entity) {
                        batch.flush(world);
                        batch.entity = Some(rec.entity);
                    }
                    batch.add(Pending {
                        c,
                        layout: rec.layout,
                        drop: rec.drop,
                        payload: rec.payload,
                    });
                }
                Op::Remove(entity, component) => {
                    batch.flush(world);
                    let c = component.resolve(world.components_mut());
                    let _ = world.remove_by_id(entity, c);
                }
                Op::Despawn(entity) => {
                    batch.flush(world);
                    let _ = world.despawn(entity);
                }
                Op::Custom => {
                    batch.flush(world);
                    let command = customs.next().expect("custom command");
                    command(world);
                }
            }
        }
        batch.flush(world);
    }
}

impl Drop for CommandQueue {
    /// Values recorded but never applied are still dropped exactly once.
    fn drop(&mut self) {
        let mut reader = Reader::new(&self.bytes);
        while !reader.done() {
            if let Op::Insert(rec) = reader.op() {
                // SAFETY: the payload is an owned value of this layout.
                unsafe { drop_payload(rec.layout, rec.drop, rec.payload) };
            }
        }
    }
}

/// Drops a value that lives unaligned inside the byte log.
///
/// # Safety
/// `bytes` holds an initialized value of `layout` that `drop` can drop.
unsafe fn drop_payload(layout: Layout, drop: Option<DropFn>, bytes: &[u8]) {
    let Some(drop) = drop else { return };
    if layout.size() == 0 {
        // SAFETY: a zero-sized value lives at any aligned address.
        unsafe { drop(ptr::without_provenance_mut(layout.align())) };
        return;
    }
    // The log gives no alignment guarantee, so move the bytes somewhere
    // properly aligned first.
    // SAFETY: the layout has non-zero size.
    let aligned = unsafe { alloc::alloc(layout) };
    if aligned.is_null() {
        alloc::handle_alloc_error(layout);
    }
    // SAFETY: `aligned` is valid for `layout.size()` bytes, and the value in
    // `bytes` is initialized per the contract.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), aligned, layout.size());
        drop(aligned);
        alloc::dealloc(aligned, layout);
    }
}

/// A decoded insert whose component has not been resolved yet.
struct InsertRecord<'a> {
    entity: Entity,
    component: ComponentRef,
    layout: Layout,
    drop: Option<DropFn>,
    payload: &'a [u8],
}

enum Op<'a> {
    Insert(InsertRecord<'a>),
    Remove(Entity, ComponentRef),
    Despawn(Entity),
    Custom,
}

/// Appends the bytes of a `Copy` value, keeping pointer provenance (so
/// function pointers survive the round trip, which integers would not).
fn put_value<T: Copy>(buf: &mut Vec<u8>, v: T) {
    let at = buf.len();
    buf.resize(at + mem::size_of::<T>(), 0);
    // SAFETY: the buffer was just grown to hold a `T` at `at`.
    unsafe { ptr::write_unaligned(buf.as_mut_ptr().add(at).cast::<T>(), v) };
}

fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_ne_bytes());
}

fn put_usize(buf: &mut Vec<u8>, v: usize) {
    buf.extend_from_slice(&v.to_ne_bytes());
}

fn put_ref(buf: &mut Vec<u8>, r: ComponentRef) {
    match r {
        ComponentRef::Id(c) => {
            buf.push(REF_ID);
            put_usize(buf, c.index());
        }
        ComponentRef::Resolver(f) => {
            buf.push(REF_RESOLVER);
            put_value(buf, f);
        }
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn done(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn take(&mut self, n: usize) -> &'a [u8] {
        let bytes = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        bytes
    }

    /// # Safety
    /// The next bytes were written by [`put_value`] with the same `T`.
    unsafe fn value<T: Copy>(&mut self) -> T {
        let bytes = self.take(mem::size_of::<T>());
        // SAFETY: in bounds per `take`; the value is valid per the contract.
        unsafe { ptr::read_unaligned(bytes.as_ptr().cast::<T>()) }
    }

    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }

    fn u64(&mut self) -> u64 {
        u64::from_ne_bytes(self.take(8).try_into().expect("8 bytes"))
    }

    fn usize(&mut self) -> usize {
        const N: usize = mem::size_of::<usize>();
        usize::from_ne_bytes(self.take(N).try_into().expect("usize bytes"))
    }

    fn entity(&mut self) -> Entity {
        Entity::from_bits(self.u64())
    }

    fn component(&mut self) -> ComponentRef {
        match self.u8() {
            REF_ID => {
                let index = self.usize();
                ComponentRef::Id(ComponentId::from_index(
                    u32::try_from(index).expect("component id"),
                ))
            }
            // SAFETY: written by `put_ref` as a `Resolver`.
            REF_RESOLVER => ComponentRef::Resolver(unsafe { self.value() }),
            _ => unreachable!("corrupt command log"),
        }
    }

    fn op(&mut self) -> Op<'a> {
        match self.u8() {
            OP_INSERT => {
                let entity = self.entity();
                let component = self.component();
                let size = self.usize();
                let align = self.usize();
                // SAFETY: written by `push_insert` as an `Option<DropFn>`.
                let drop = unsafe { self.value::<Option<DropFn>>() };
                let layout = Layout::from_size_align(size, align)
                    .expect("recorded layout");
                Op::Insert(InsertRecord {
                    entity,
                    component,
                    layout,
                    drop,
                    payload: self.take(size),
                })
            }
            OP_REMOVE => {
                let entity = self.entity();
                Op::Remove(entity, self.component())
            }
            OP_DESPAWN => Op::Despawn(self.entity()),
            OP_CUSTOM => Op::Custom,
            _ => unreachable!("corrupt command log"),
        }
    }
}

struct Pending<'a> {
    c: ComponentId,
    layout: Layout,
    drop: Option<DropFn>,
    payload: &'a [u8],
}

impl Pending<'_> {
    fn discard(&self) {
        // SAFETY: the payload is an owned, initialized value.
        unsafe { drop_payload(self.layout, self.drop, self.payload) };
    }
}

/// Consecutive inserts on one entity, applied together as one move.
#[derive(Default)]
struct Batch<'a> {
    entity: Option<Entity>,
    items: Vec<Pending<'a>>,
}

impl<'a> Batch<'a> {
    fn add(&mut self, pending: Pending<'a>) {
        // A repeated component keeps the last value, like sequential inserts.
        match self.items.iter().position(|p| p.c == pending.c) {
            Some(i) => {
                self.items[i].discard();
                self.items[i] = pending;
            }
            None => self.items.push(pending),
        }
    }

    fn flush(&mut self, world: &mut World) {
        let entity = self.entity.take();
        let items = mem::take(&mut self.items);
        let Some(entity) = entity else { return };
        if items.is_empty() {
            return;
        }
        let ptrs: Vec<_> =
            items.iter().map(|p| (p.c, p.payload.as_ptr())).collect();
        // SAFETY: each payload is an initialized value of its component, the
        // ids are distinct (see `add`), and ownership moves on `Ok`.
        let result = unsafe { world.insert_many_by_id(entity, &ptrs) };
        if result.is_err() {
            items.iter().for_each(Pending::discard);
        }
    }
}

/// Records structural changes instead of performing them, so no column
/// pointer or slice held by a running system is ever invalidated.
pub struct Commands<'w, 's> {
    queue: &'s mut CommandQueue,
    entities: &'w Entities,
}

impl<'w, 's> Commands<'w, 's> {
    pub(crate) fn new(
        queue: &'s mut CommandQueue,
        entities: &'w Entities,
    ) -> Self {
        Self { queue, entities }
    }

    /// Emits `event` at `target`; observers run at the sync point, in order
    /// with the other commands.
    pub fn emit<E: Event>(&mut self, target: Entity, event: E) {
        self.queue
            .push(move |w| w.trigger_or_defer(Some(target), event));
    }

    pub fn emit_global<E: Event>(&mut self, event: E) {
        self.queue.push(move |w| w.trigger_or_defer(None, event));
    }

    /// Reserves an entity now; it exists and gets its components at the sync
    /// point.
    pub fn spawn(&mut self) -> EntityCommands<'_, 'w, 's> {
        let entity = self.entities.reserve();
        EntityCommands {
            entity,
            commands: self,
        }
    }

    pub fn entity(&mut self, entity: Entity) -> EntityCommands<'_, 'w, 's> {
        EntityCommands {
            entity,
            commands: self,
        }
    }

    pub fn despawn(&mut self, entity: Entity) {
        self.queue.despawn(entity);
    }

    pub fn insert_resource<T: Send + Sync + 'static>(&mut self, value: T) {
        self.queue.push(move |world| world.insert_resource(value));
    }

    /// Queues arbitrary work with full world access.
    pub fn add(&mut self, command: impl FnOnce(&mut World) + 'static) {
        self.queue.push(command);
    }
}

pub struct EntityCommands<'a, 'w, 's> {
    entity: Entity,
    commands: &'a mut Commands<'w, 's>,
}

impl EntityCommands<'_, '_, '_> {
    pub fn id(&self) -> Entity {
        self.entity
    }

    /// Adds (or overwrites) a component. Inserts on a dead entity are ignored.
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) -> &mut Self {
        self.commands.queue.insert(self.entity, value);
        self
    }

    pub fn remove<T: Send + Sync + 'static>(&mut self) -> &mut Self {
        self.commands.queue.remove::<T>(self.entity);
        self
    }

    pub fn despawn(&mut self) {
        self.commands.despawn(self.entity);
    }
}

// SAFETY: only shared access to the entity allocator (atomic reservation),
// and it declares no component or resource access.
unsafe impl SystemParam for Commands<'_, '_> {
    type State = CommandQueue;
    type Item<'w, 's> = Commands<'w, 's>;

    fn init_state(_: &mut World, _: &mut Access) -> CommandQueue {
        CommandQueue::default()
    }

    unsafe fn get_param<'w, 's>(
        state: &'s mut CommandQueue,
        _: Tick,
        world: WorldCell<'w>,
    ) -> Commands<'w, 's> {
        Commands {
            queue: state,
            // SAFETY: nothing mutates the allocator during a system run.
            entities: unsafe { world.entities() },
        }
    }

    fn apply(state: &mut CommandQueue, world: &mut World) {
        state.apply(world);
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct A(u32);
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct B(u32);
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct C(u32);
    struct Tag;

    #[test]
    fn spawn_with_several_inserts_moves_once() {
        let mut world = World::new();
        let mut q = CommandQueue::default();
        let e = world.entities().reserve();
        q.insert(e, A(1));
        q.insert(e, B(2));
        q.insert(e, C(3));
        q.apply(&mut world);
        assert_eq!(world.archetype_moves(), 1);
        assert_eq!(world.get::<A>(e), Some(&A(1)));
        assert_eq!(world.get::<B>(e), Some(&B(2)));
        assert_eq!(world.get::<C>(e), Some(&C(3)));
        // Only the final archetype was created, no intermediate ones.
        let with_comps = world
            .archetypes()
            .iter()
            .filter(|a| !a.components().is_empty())
            .count();
        assert_eq!(with_comps, 1);
    }

    #[test]
    fn unbatched_inserts_move_each_time() {
        // The baseline the batching improves on.
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, A(1)).unwrap();
        world.insert(e, B(2)).unwrap();
        world.insert(e, C(3)).unwrap();
        assert_eq!(world.archetype_moves(), 3);
    }

    #[test]
    fn interleaved_entities_still_apply_correctly() {
        let mut world = World::new();
        let (e1, e2) = (world.spawn(), world.spawn());
        let mut q = CommandQueue::default();
        q.insert(e1, A(1));
        q.insert(e2, A(2));
        q.insert(e1, B(3));
        q.apply(&mut world);
        assert_eq!(world.get::<B>(e1), Some(&B(3)));
        assert_eq!(world.get::<A>(e2), Some(&A(2)));
        assert!(!world.has::<B>(e2));
    }

    #[test]
    fn commands_apply_in_order() {
        let mut world = World::new();
        let e = world.spawn();
        let mut q = CommandQueue::default();
        q.insert(e, A(1));
        q.remove::<A>(e);
        q.insert(e, A(2));
        q.insert(e, B(3));
        q.push(move |w| {
            // The batch before a custom command is already applied.
            assert_eq!(w.get::<B>(e), Some(&B(3)));
            w.insert(e, C(4)).unwrap();
        });
        q.despawn(e);
        q.apply(&mut world);
        assert!(!world.is_alive(e));
        assert!(q.is_empty());
    }

    #[test]
    fn repeated_component_keeps_last_value() {
        let mut world = World::new();
        let e = world.spawn();
        let mut q = CommandQueue::default();
        q.insert(e, A(1));
        q.insert(e, A(2));
        q.apply(&mut world);
        assert_eq!(world.get::<A>(e), Some(&A(2)));
    }

    #[test]
    fn existing_components_are_overwritten_in_a_batch() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, A(1)).unwrap();
        let before = world.archetype_moves();
        let mut q = CommandQueue::default();
        q.insert(e, A(9)); // overwrite
        q.insert(e, B(5)); // new
        q.apply(&mut world);
        assert_eq!(world.archetype_moves() - before, 1);
        assert_eq!(world.get::<A>(e), Some(&A(9)));
        assert_eq!(world.get::<B>(e), Some(&B(5)));
    }

    #[test]
    fn overwrite_only_batch_does_not_move() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, A(1)).unwrap();
        let before = world.archetype_moves();
        let mut q = CommandQueue::default();
        q.insert(e, A(2));
        q.apply(&mut world);
        assert_eq!(world.archetype_moves(), before);
        assert_eq!(world.get::<A>(e), Some(&A(2)));
    }

    #[test]
    fn reserved_entity_without_commands_becomes_alive() {
        let mut world = World::new();
        let e = world.entities().reserve();
        assert!(!world.is_alive(e));
        CommandQueue::default().apply(&mut world);
        assert!(world.is_alive(e));
    }

    #[test]
    fn tags_and_over_aligned_payloads_roundtrip() {
        #[repr(align(64))]
        #[derive(Debug, PartialEq)]
        struct Wide([u8; 100]);
        let mut world = World::new();
        let e = world.entities().reserve();
        let mut q = CommandQueue::default();
        q.insert(e, Tag);
        q.insert(e, Wide([7; 100]));
        q.insert(e, A(1));
        q.apply(&mut world);
        assert!(world.has::<Tag>(e));
        assert_eq!(world.get::<Wide>(e), Some(&Wide([7; 100])));
        let p = world.get::<Wide>(e).unwrap();
        assert_eq!((p as *const Wide as usize) % 64, 0);
    }

    struct Counter(Rc<Cell<usize>>);
    impl Drop for Counter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    // `Counter` is not Send/Sync, so register it by hand for these tests.
    fn counter_component(world: &mut World) -> ComponentId {
        use crate::component::{ComponentInfo, Kind, Storage};
        world.components_mut().register_raw(ComponentInfo {
            name: "Counter".into(),
            layout: Layout::new::<Counter>(),
            drop: Some(drop_fn::<Counter>),
            clone: None,
            kind: Kind::Native,
            storage: Storage::Table,
            hooks: Default::default(),
            reflect: None,
        })
    }

    fn push_counter(
        q: &mut CommandQueue,
        e: Entity,
        c: ComponentId,
        drops: &Rc<Cell<usize>>,
    ) {
        let value = ManuallyDrop::new(Counter(drops.clone()));
        // SAFETY: a valid Counter whose ownership moves into the queue.
        unsafe {
            q.insert_by_id(
                e,
                c,
                Layout::new::<Counter>(),
                Some(drop_fn::<Counter>),
                (&raw const *value).cast(),
            );
        }
    }

    #[test]
    fn unapplied_payloads_drop_once() {
        let mut world = World::new();
        let c = counter_component(&mut world);
        let drops = Rc::new(Cell::new(0));
        let e = world.spawn();
        let mut q = CommandQueue::default();
        push_counter(&mut q, e, c, &drops);
        push_counter(&mut q, e, c, &drops);
        drop(q);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn dead_entity_and_duplicate_payloads_drop_once() {
        let mut world = World::new();
        let c = counter_component(&mut world);
        let drops = Rc::new(Cell::new(0));

        // Duplicate in one batch: the first is dropped, the last kept.
        let e = world.spawn();
        let mut q = CommandQueue::default();
        push_counter(&mut q, e, c, &drops);
        push_counter(&mut q, e, c, &drops);
        q.apply(&mut world);
        assert_eq!(drops.get(), 1);

        // Insert on a dead entity: the payload is dropped, not leaked.
        let dead = world.spawn();
        world.despawn(dead).unwrap();
        let mut q = CommandQueue::default();
        push_counter(&mut q, dead, c, &drops);
        q.apply(&mut world);
        assert_eq!(drops.get(), 2);

        // Overwriting drops the old value; despawning drops the last one.
        let mut q = CommandQueue::default();
        push_counter(&mut q, e, c, &drops);
        q.apply(&mut world);
        assert_eq!(drops.get(), 3);
        world.despawn(e).unwrap();
        assert_eq!(drops.get(), 4);
    }

    #[test]
    fn insert_by_id_matches_typed_insert() {
        let mut world = World::new();
        let a = world.component_id::<A>();
        let e = world.spawn();
        let mut q = CommandQueue::default();
        let value = A(42);
        // SAFETY: a valid A; A needs no drop.
        unsafe {
            q.insert_by_id(
                e,
                a,
                Layout::new::<A>(),
                None,
                (&raw const value).cast(),
            );
        }
        q.remove_by_id(e, a);
        q.insert(e, A(43));
        q.apply(&mut world);
        assert_eq!(world.get::<A>(e), Some(&A(43)));
    }

    #[derive(Debug, Clone)]
    enum Cmd {
        InsertA(usize, u32),
        InsertB(usize, u32),
        InsertC(usize, u32),
        RemoveA(usize),
        RemoveB(usize),
        Despawn(usize),
    }

    fn cmd() -> impl proptest::strategy::Strategy<Value = Cmd> {
        use proptest::prelude::*;
        let e = 0usize..4;
        prop_oneof![
            3 => (e.clone(), any::<u32>()).prop_map(|(e, v)| Cmd::InsertA(e, v)),
            3 => (e.clone(), any::<u32>()).prop_map(|(e, v)| Cmd::InsertB(e, v)),
            3 => (e.clone(), any::<u32>()).prop_map(|(e, v)| Cmd::InsertC(e, v)),
            2 => e.clone().prop_map(Cmd::RemoveA),
            2 => e.clone().prop_map(Cmd::RemoveB),
            1 => e.prop_map(Cmd::Despawn),
        ]
    }

    proptest::proptest! {
        /// Batching must be invisible: applying a command log gives the same
        /// world as running the same operations one by one.
        #[test]
        fn queue_matches_direct_application(
            cmds in proptest::collection::vec(cmd(), 0..60),
        ) {
            let mut queued = World::new();
            let mut direct = World::new();
            let ents: Vec<_> = (0..4).map(|_| queued.spawn()).collect();
            let same: Vec<_> = (0..4).map(|_| direct.spawn()).collect();
            proptest::prop_assert_eq!(&ents, &same);

            let mut q = CommandQueue::default();
            for c in &cmds {
                match *c {
                    Cmd::InsertA(i, v) => {
                        q.insert(ents[i], A(v));
                        let _ = direct.insert(ents[i], A(v));
                    }
                    Cmd::InsertB(i, v) => {
                        q.insert(ents[i], B(v));
                        let _ = direct.insert(ents[i], B(v));
                    }
                    Cmd::InsertC(i, v) => {
                        q.insert(ents[i], C(v));
                        let _ = direct.insert(ents[i], C(v));
                    }
                    Cmd::RemoveA(i) => {
                        q.remove::<A>(ents[i]);
                        let _ = direct.remove::<A>(ents[i]);
                    }
                    Cmd::RemoveB(i) => {
                        q.remove::<B>(ents[i]);
                        let _ = direct.remove::<B>(ents[i]);
                    }
                    Cmd::Despawn(i) => {
                        q.despawn(ents[i]);
                        let _ = direct.despawn(ents[i]);
                    }
                }
            }
            q.apply(&mut queued);

            for &e in &ents {
                proptest::prop_assert_eq!(queued.is_alive(e), direct.is_alive(e));
                proptest::prop_assert_eq!(queued.get::<A>(e), direct.get::<A>(e));
                proptest::prop_assert_eq!(queued.get::<B>(e), direct.get::<B>(e));
                proptest::prop_assert_eq!(queued.get::<C>(e), direct.get::<C>(e));
            }
            queued.validate();
            // Never more moves than the unbatched version.
            proptest::prop_assert!(queued.archetype_moves() <= direct.archetype_moves());
        }
    }
}
