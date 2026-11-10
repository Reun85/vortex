//! The binary contract with the C# side. Every type here has a mirror in
//! `scripting/Delta.Scripting/Native.cs`; change both together and bump
//! [`API_VERSION`].

use delta::{World, query::Cursor};

pub const API_VERSION: u32 = 6;

/// Most data terms a query may have; sizes [`ChunkFfi::columns`].
pub const MAX_TERMS: usize = 16;

/// `ComponentDescFfi::kind`
pub const KIND_UNMANAGED: u32 = 0;
pub const KIND_MANAGED: u32 = 1;
pub const KIND_TAG: u32 = 2;
/// Not a component: a plain struct that other components embed.
pub const KIND_STRUCT: u32 = 3;

/// `FieldDescFfi::ty`
pub const FT_BOOL: u32 = 0;
pub const FT_I8: u32 = 1;
pub const FT_I16: u32 = 2;
pub const FT_I32: u32 = 3;
pub const FT_I64: u32 = 4;
pub const FT_U8: u32 = 5;
pub const FT_U16: u32 = 6;
pub const FT_U32: u32 = 7;
pub const FT_U64: u32 = 8;
pub const FT_F32: u32 = 9;
pub const FT_F64: u32 = 10;
pub const FT_ENTITY: u32 = 11;
/// `aux` is the reflect id of a struct registered with [`KIND_STRUCT`].
pub const FT_STRUCT: u32 = 12;
/// `aux` is the length; `elem_ty`/`elem_aux` describe the element (a
/// scalar, or a struct by reflect id).
pub const FT_ARRAY: u32 = 13;
/// `System.Numerics` math types, which are `vortex::Vec2/3/4/Quat` in Rust.
pub const FT_VEC2: u32 = 14;
pub const FT_VEC3: u32 = 15;
pub const FT_VEC4: u32 = 16;
pub const FT_QUAT: u32 = 17;

/// `ScriptApi::behaviour_to_json` mode: scenes, the inspector, undo, play
/// mode. Public and `[SerializeField]` members; entities as scene ids.
pub const MODE_PERSIST: u32 = 0;
/// `ScriptApi::behaviour_to_json` mode: hot reload. Every field, entities
/// as raw handles.
pub const MODE_SNAPSHOT: u32 = 1;

/// `ScriptApi::behaviour_event`: lifecycle events of a behaviour.
pub const EVENT_AWAKE: u32 = 0;
pub const EVENT_ON_DESTROY: u32 = 1;
/// Reserved for a later phase.
pub const EVENT_ON_ENABLE: u32 = 2;
/// Reserved for a later phase.
pub const EVENT_ON_DISABLE: u32 = 3;
/// A hot reload rebuilt the instance from its saved state.
pub const EVENT_AFTER_RELOAD: u32 = 4;

/// `FieldDescFfi::flags`
pub const FF_HIDDEN: u32 = 1;
pub const FF_READ_ONLY: u32 = 2;
/// `range_lo`/`range_hi` are meaningful.
pub const FF_HAS_RANGE: u32 = 4;

#[repr(C)]
pub struct FieldDescFfi {
    pub name: *const u8,
    pub name_len: usize,
    pub offset: u32,
    pub ty: u32,
    pub aux: u32,
    pub flags: u32,
    pub range_lo: f64,
    pub range_hi: f64,
    pub tooltip: *const u8,
    pub tooltip_len: usize,
    pub elem_ty: u32,
    pub elem_aux: u32,
}

/// Checked by the managed side too, so the two mirrors cannot drift apart.
pub const FIELD_DESC_SIZE: usize = 72;
const _: () = assert!(size_of::<FieldDescFfi>() == FIELD_DESC_SIZE);

#[repr(C)]
pub struct ComponentDescFfi {
    pub name: *const u8,
    pub name_len: usize,
    pub size: usize,
    pub align: usize,
    pub kind: u32,
    pub field_count: u32,
    pub fields: *const FieldDescFfi,
    /// Component ids inserted alongside this one when missing (data and tag
    /// components only).
    pub requires: IdList,
}

#[repr(C)]
pub struct IdList {
    pub ptr: *const u32,
    pub len: usize,
}

#[repr(C)]
pub struct QueryDescFfi {
    pub read: IdList,
    pub write: IdList,
    pub with: IdList,
    pub without: IdList,
    pub optional: IdList,
    pub added: IdList,
    pub changed: IdList,
}

/// One run of rows, as raw pointers. Valid until the next structural change,
/// which cannot happen while a script system runs.
#[repr(C)]
pub struct ChunkFfi {
    pub archetype: u32,
    pub len: u32,
    pub entities: *const u64,
    /// One pointer per data term (reads, then writes, then optionals), to
    /// the first row of this chunk; null for an absent optional.
    pub columns: [*mut u8; MAX_TERMS],
}

#[repr(C)]
pub struct SystemDescFfi {
    pub name: *const u8,
    pub name_len: usize,
    /// A [`delta::Stage`] as an integer.
    pub stage: u32,
    pub order: i32,
}

/// Rust -> C#. Every call that touches the world passes the world pointer
/// the host was handed in `run_system`/`load_assembly`.
#[repr(C)]
pub struct HostApi {
    pub version: u32,
    pub size: u32,
    pub register_component:
        unsafe extern "C" fn(*mut World, *const ComponentDescFfi) -> u32,
    pub component_id_by_name:
        unsafe extern "C" fn(*mut World, *const u8, usize) -> u32,
    pub register_system:
        unsafe extern "C" fn(*mut World, *const SystemDescFfi) -> u32,
    pub query_create:
        unsafe extern "C" fn(*mut World, *const QueryDescFfi) -> u32,
    pub query_next:
        unsafe extern "C" fn(*mut World, u32, *mut Cursor, *mut ChunkFfi) -> u8,
    pub get_ptr: unsafe extern "C" fn(*mut World, u64, u32, u8) -> *mut u8,
    pub is_alive: unsafe extern "C" fn(*mut World, u64) -> u8,
    pub cmd_spawn: unsafe extern "C" fn(*mut World) -> u64,
    pub cmd_despawn: unsafe extern "C" fn(*mut World, u64),
    pub cmd_insert:
        unsafe extern "C" fn(*mut World, u64, u32, *const u8, usize),
    pub cmd_remove: unsafe extern "C" fn(*mut World, u64, u32),
    pub log: unsafe extern "C" fn(u32, *const u8, usize),
    pub last_error: unsafe extern "C" fn(*mut World, *mut u8, usize) -> usize,
    /// Releases a query; its id may be handed out again.
    pub query_free: unsafe extern "C" fn(*mut World, u32),
    /// The id of a resource exposed to scripts by name, or `u32::MAX`.
    pub resource_id_by_name:
        unsafe extern "C" fn(*mut World, *const u8, usize) -> u32,
    /// The address of an exposed resource's value (`write` != 0 to modify
    /// it), or null. Valid for the system run; the resource may be replaced
    /// between runs, so callers must not cache it.
    pub resource_ptr: unsafe extern "C" fn(*mut World, u32, u8) -> *mut u8,
    /// Compares a C# mirror (name, size, align, fields) with the Rust layout
    /// of the exposed resource or script-visible component of that name.
    /// Returns 0 when they are identical, `u32::MAX` otherwise with both
    /// layouts in `last_error`.
    pub check_layout: unsafe extern "C" fn(
        *mut World,
        *const u8,
        usize,
        usize,
        usize,
        *const FieldDescFfi,
        usize,
    ) -> u32,
    /// The slot of an input axis name, or `u32::MAX`.
    pub input_axis_id:
        unsafe extern "C" fn(*mut World, *const u8, usize) -> u32,
    /// The scene id of an entity, written as text; also for an entity that
    /// was despawned if the world still remembers its id. Returns the text
    /// length (copying at most `cap` bytes), or 0 when it has none.
    pub entity_to_scene_id:
        unsafe extern "C" fn(*mut World, u64, *mut u8, usize) -> usize,
    /// The entity for a scene id given as text, or `u64::MAX` (the null
    /// entity) when none is known.
    pub scene_id_to_entity:
        unsafe extern "C" fn(*mut World, *const u8, usize) -> u64,
}

/// C# -> Rust, filled in by `Bridge.Initialize`. Optional so an
/// uninitialized table is representable. Field order is the contract.
#[repr(C)]
pub struct ScriptApi {
    pub version: u32,
    pub size: u32,
    /// Loads a script assembly into its own collectible load context,
    /// registers what it declares, writes an assembly handle to the last
    /// argument, and returns the number of systems registered, or a negative
    /// value on error (see `last_error`).
    pub load_assembly: Option<
        unsafe extern "C" fn(*mut World, *const u8, usize, *mut u32) -> i32,
    >,
    pub run_system: Option<unsafe extern "C" fn(u32, *mut World)>,
    pub free_handle: Option<unsafe extern "C" fn(u64)>,
    /// Number of live `GCHandle`s held for managed components (for tests).
    pub live_handles: Option<unsafe extern "C" fn() -> u64>,
    /// The message of the last managed failure, copied into the buffer;
    /// returns its full length.
    pub last_error: Option<unsafe extern "C" fn(*mut u8, usize) -> usize>,
    /// Tears down an assembly's systems and unloads its load context. Every
    /// handle to its objects must have been freed first.
    pub unload_assembly: Option<unsafe extern "C" fn(u32)>,
    /// Collects garbage and reports whether the assembly's load context is
    /// still alive (nonzero): a zero means the old code is really gone.
    pub assembly_alive: Option<unsafe extern "C" fn(u32) -> u8>,
    /// Serializes a behaviour object for a reload. Copies up to `cap` bytes
    /// and returns the full length, or `usize::MAX` on failure. The call
    /// that actually delivers the bytes (`cap` large enough) also runs the
    /// object's `OnUnload`; a size query with `cap` 0 does not.
    pub serialize_behaviour:
        Option<unsafe extern "C" fn(u64, *mut u8, usize) -> usize>,
    /// Creates a behaviour of a component's (new) class from serialized
    /// state and returns its handle, or 0 on failure.
    pub deserialize_behaviour:
        Option<unsafe extern "C" fn(u32, *const u8, usize) -> u64>,
    /// Drains the warnings the managed side collected (newline separated).
    pub take_warnings: Option<unsafe extern "C" fn(*mut u8, usize) -> usize>,
    /// Delivers a lifecycle event (`EVENT_*`) to a behaviour: world, the
    /// object's handle, the entity it is on, the event. The managed side sets
    /// the object's `Entity` and `World` first.
    pub behaviour_event:
        Option<unsafe extern "C" fn(*mut World, u64, u64, u32)>,
    /// Serializes a behaviour (`MODE_*`) without tearing it down. Stores the
    /// JSON, returns its length or `usize::MAX` (see `last_error`); fetch it
    /// with `take_result`.
    pub behaviour_to_json:
        Option<unsafe extern "C" fn(*mut World, u64, u32) -> usize>,
    /// Builds a persisted behaviour of a class from JSON (running
    /// `OnAfterDeserialize`) and returns its handle, or 0 (see `last_error`).
    pub behaviour_from_json:
        Option<unsafe extern "C" fn(*mut World, u32, *const u8, usize) -> u64>,
    /// One member of a behaviour (path "" is the whole), as JSON.
    pub behaviour_get_member: Option<
        unsafe extern "C" fn(*mut World, u64, *const u8, usize) -> usize,
    >,
    /// Sets one member of the live object in place from JSON and stores the
    /// old value's JSON.
    pub behaviour_set_member: Option<
        unsafe extern "C" fn(
            *mut World,
            u64,
            *const u8,
            usize,
            *const u8,
            usize,
        ) -> usize,
    >,
    /// The inspector's member list for a class, as JSON.
    pub behaviour_schema: Option<unsafe extern "C" fn(u32) -> usize>,
    /// A default instance of a class, persisted, as JSON.
    pub behaviour_default_json:
        Option<unsafe extern "C" fn(*mut World, u32) -> usize>,
    /// Rewrites entity references inside a behaviour; returns how many.
    pub behaviour_remap_entity:
        Option<unsafe extern "C" fn(*mut World, u64, u64, u64) -> u32>,
    /// Copies the text stored by the last call that returned a length.
    pub take_result: Option<unsafe extern "C" fn(*mut u8, usize) -> usize>,
}
