using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace Delta;

// Mirrors of crates/delta_script/src/ffi.rs. Change both together and bump
// Native.ApiVersion.

internal static class Native
{
    public const uint ApiVersion = 6;
    public const int MaxTerms = 16;

    // ComponentDescFfi.Kind
    public const uint KindUnmanaged = 0;
    public const uint KindManaged = 1;
    public const uint KindTag = 2;
    public const uint KindStruct = 3;

    // FieldDescFfi.Ty
    public const uint FtBool = 0;
    public const uint FtI8 = 1, FtI16 = 2, FtI32 = 3, FtI64 = 4;
    public const uint FtU8 = 5, FtU16 = 6, FtU32 = 7, FtU64 = 8;
    public const uint FtF32 = 9, FtF64 = 10, FtEntity = 11, FtStruct = 12, FtArray = 13;
    public const uint FtVec2 = 14, FtVec3 = 15, FtVec4 = 16, FtQuat = 17;

    // ScriptApi.BehaviourEvent
    public const uint EventAwake = 0, EventOnDestroy = 1, EventAfterReload = 4;

    // FieldDescFfi.Flags
    public const uint FfHidden = 1, FfReadOnly = 2, FfHasRange = 4;

    /// <summary>Size of <see cref="FieldDescFfi"/>; Rust asserts the same number.</summary>
    public const int FieldDescSize = 72;

    public const uint Invalid = uint.MaxValue;
}

[StructLayout(LayoutKind.Sequential)]
internal unsafe struct FieldDescFfi
{
    public byte* Name;
    public nuint NameLen;
    public uint Offset;
    public uint Ty;
    public uint Aux;
    public uint Flags;
    public double RangeLo;
    public double RangeHi;
    public byte* Tooltip;
    public nuint TooltipLen;
    public uint ElemTy;
    public uint ElemAux;
}

[StructLayout(LayoutKind.Sequential)]
internal unsafe struct ComponentDescFfi
{
    public byte* Name;
    public nuint NameLen;
    public nuint Size;
    public nuint Align;
    public uint Kind;
    public uint FieldCount;
    public FieldDescFfi* Fields;
    public IdList Requires;
}

[StructLayout(LayoutKind.Sequential)]
internal unsafe struct IdList
{
    public uint* Ptr;
    public nuint Len;
}

[StructLayout(LayoutKind.Sequential)]
internal struct QueryDescFfi
{
    public IdList Read, Write, With, Without, Optional, Added, Changed;
}

[InlineArray(Native.MaxTerms)]
internal struct ColumnPtrs
{
    private nint _element;
}

[StructLayout(LayoutKind.Sequential)]
internal unsafe struct ChunkFfi
{
    public uint Archetype;
    public uint Len;
    public ulong* Entities;
    public ColumnPtrs Columns;
}

/// <summary>Opaque iteration position; the Rust side owns its meaning.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct Cursor
{
    public nuint A;
    public nuint B;
}

[StructLayout(LayoutKind.Sequential)]
internal unsafe struct SystemDescFfi
{
    public byte* Name;
    public nuint NameLen;
    public uint Stage;
    public int Order;
}

/// <summary>Rust -> C#.</summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct HostApi
{
    public uint Version;
    public uint Size;
    public delegate* unmanaged<nint, ComponentDescFfi*, uint> RegisterComponent;
    public delegate* unmanaged<nint, byte*, nuint, uint> ComponentIdByName;
    public delegate* unmanaged<nint, SystemDescFfi*, uint> RegisterSystem;
    public delegate* unmanaged<nint, QueryDescFfi*, uint> QueryCreate;
    public delegate* unmanaged<nint, uint, Cursor*, ChunkFfi*, byte> QueryNext;
    public delegate* unmanaged<nint, ulong, uint, byte, byte*> GetPtr;
    public delegate* unmanaged<nint, ulong, byte> IsAlive;
    public delegate* unmanaged<nint, ulong> CmdSpawn;
    public delegate* unmanaged<nint, ulong, void> CmdDespawn;
    public delegate* unmanaged<nint, ulong, uint, byte*, nuint, void> CmdInsert;
    public delegate* unmanaged<nint, ulong, uint, void> CmdRemove;
    public delegate* unmanaged<uint, byte*, nuint, void> Log;
    public delegate* unmanaged<nint, byte*, nuint, nuint> LastError;
    public delegate* unmanaged<nint, uint, void> QueryFree;
    public delegate* unmanaged<nint, byte*, nuint, uint> ResourceIdByName;
    public delegate* unmanaged<nint, uint, byte, byte*> ResourcePtr;
    public delegate* unmanaged<nint, byte*, nuint, nuint, nuint, FieldDescFfi*, nuint, uint> CheckLayout;
    public delegate* unmanaged<nint, byte*, nuint, uint> InputAxisId;
    public delegate* unmanaged<nint, ulong, byte*, nuint, nuint> EntityToSceneId;
    public delegate* unmanaged<nint, byte*, nuint, ulong> SceneIdToEntity;
}

/// <summary>C# -> Rust, filled in by <see cref="Bridge.Initialize"/>. Field order is the contract.</summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct ScriptApi
{
    public uint Version;
    public uint Size;
    public delegate* unmanaged<nint, byte*, nuint, uint*, int> LoadAssembly;
    public delegate* unmanaged<uint, nint, void> RunSystem;
    public delegate* unmanaged<ulong, void> FreeHandle;
    public delegate* unmanaged<ulong> LiveHandles;
    public delegate* unmanaged<byte*, nuint, nuint> LastError;
    public delegate* unmanaged<uint, void> UnloadAssembly;
    public delegate* unmanaged<uint, byte> AssemblyAlive;
    public delegate* unmanaged<ulong, byte*, nuint, nuint> SerializeBehaviour;
    public delegate* unmanaged<uint, byte*, nuint, ulong> DeserializeBehaviour;
    public delegate* unmanaged<byte*, nuint, nuint> TakeWarnings;
    public delegate* unmanaged<nint, ulong, ulong, uint, void> BehaviourEvent;
    public delegate* unmanaged<nint, ulong, uint, nuint> BehaviourToJson;
    public delegate* unmanaged<nint, uint, byte*, nuint, ulong> BehaviourFromJson;
    public delegate* unmanaged<nint, ulong, byte*, nuint, nuint> BehaviourGetMember;
    public delegate* unmanaged<nint, ulong, byte*, nuint, byte*, nuint, nuint> BehaviourSetMember;
    public delegate* unmanaged<uint, nuint> BehaviourSchema;
    public delegate* unmanaged<nint, uint, nuint> BehaviourDefaultJson;
    public delegate* unmanaged<nint, ulong, ulong, ulong, uint> BehaviourRemapEntity;
    public delegate* unmanaged<byte*, nuint, nuint> TakeResult;
}
