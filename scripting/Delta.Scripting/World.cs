using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;

namespace Delta;

/// <summary>
/// A handle to the world a script system is running in. Structural changes
/// (<see cref="Spawn"/>, <see cref="Insert{T}"/>, ...) are deferred to the
/// stage's sync point, so spans from queries stay valid for the whole run.
/// </summary>
public sealed unsafe class World
{
    static readonly Dictionary<nint, World> Worlds = new();

    internal readonly nint Ptr;

    World(nint ptr) => Ptr = ptr;

    internal static World For(nint ptr)
    {
        lock (Worlds)
        {
            if (!Worlds.TryGetValue(ptr, out var w)) Worlds[ptr] = w = new World(ptr);
            return w;
        }
    }

    public QueryBuilder Query() => new(this);

    public bool IsAlive(Entity e) => Host.Api->IsAlive(Ptr, e.Bits) != 0;

    /// <summary>Reserves an entity now; it exists, with its components, at the sync point.</summary>
    public Entity Spawn() => new(Host.Api->CmdSpawn(Ptr));

    public void Despawn(Entity e) => Host.Api->CmdDespawn(Ptr, e.Bits);

    public void Insert<T>(Entity e, in T value) where T : unmanaged, IComponent
    {
        fixed (T* p = &value)
            Host.Api->CmdInsert(Ptr, e.Bits, ComponentType<T>.Id, (byte*)p, (nuint)(TagInfo<T>.IsTag ? 0 : sizeof(T)));
    }

    public void Remove<T>(Entity e) where T : unmanaged, IComponent =>
        Host.Api->CmdRemove(Ptr, e.Bits, ComponentType<T>.Id);

    /// <summary>Attaches a new <typeparamref name="T"/> behaviour to the entity.</summary>
    public T Add<T>(Entity e) where T : Behaviour, new()
    {
        var behaviour = new T { Entity = e, World = this };
        var handle = (ulong)GCHandle.ToIntPtr(GCHandle.Alloc(behaviour, GCHandleType.Normal));
        Interlocked.Increment(ref Bridge.LiveHandleCount);
        // Ownership of the handle moves into the command queue, and from
        // there into the column; dropping either frees it.
        Host.Api->CmdInsert(Ptr, e.Bits, ComponentType<T>.Id, (byte*)&handle, 8);
        return behaviour;
    }

    /// <summary>Direct access to a component of an existing entity. Marks it changed.</summary>
    public ref T Get<T>(Entity e) where T : unmanaged, IComponent
    {
        var p = Host.Api->GetPtr(Ptr, e.Bits, ComponentType<T>.Id, 1);
        if (p == null) throw new InvalidOperationException($"{e} has no {typeof(T).Name}");
        return ref *(T*)p;
    }

    public bool TryGet<T>(Entity e, out T value) where T : unmanaged, IComponent
    {
        var p = Host.Api->GetPtr(Ptr, e.Bits, ComponentType<T>.Id, 0);
        value = p == null ? default : *(T*)p;
        return p != null;
    }
}

/// <summary>A struct with no fields has size 1 in C#, but is a zero-sized tag in the world.</summary>
static class TagInfo<T> where T : unmanaged
{
    public static readonly bool IsTag = typeof(T).GetFields(System.Reflection.BindingFlags.Instance | System.Reflection.BindingFlags.Public | System.Reflection.BindingFlags.NonPublic).Length == 0;
}
