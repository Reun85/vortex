using System;

namespace Delta;

/// <summary>
/// A component of one entity, named once (typically in <c>Awake</c>) and used
/// every frame. Currently a thin wrapper over <see cref="World.Get{T}"/>: the
/// API is the final one, but nothing is cached until the Phase 5 benchmark
/// shows <c>Get</c> in a profile. Never serialized; a restored behaviour
/// simply builds a new one.
/// </summary>
public readonly unsafe struct ComponentRef<T> where T : unmanaged, IComponent
{
    readonly World? world;
    readonly Entity entity;

    internal ComponentRef(World world, Entity entity)
    {
        this.world = world;
        this.entity = entity;
    }

    public Entity Entity => entity;

    /// <summary>Reads the component. Throws if the entity is dead or lacks <typeparamref name="T"/>.</summary>
    public ref readonly T Read => ref Resolve(0);

    /// <summary>Writes the component, marking it changed. Throws like <see cref="Read"/>.</summary>
    public ref T Write => ref Resolve(1);

    /// <summary>False once the entity is dead or no longer has <typeparamref name="T"/>.</summary>
    public bool TryResolve() =>
        world is not null
        && Host.Api->GetPtr(world.Ptr, entity.Bits, ComponentType<T>.Id, 0) != null;

    ref T Resolve(byte write)
    {
        if (world is null) throw new InvalidOperationException($"default ComponentRef<{typeof(T).Name}>");
        var p = Host.Api->GetPtr(world.Ptr, entity.Bits, ComponentType<T>.Id, write);
        if (p == null) throw new InvalidOperationException($"{entity} has no {typeof(T).Name}");
        return ref *(T*)p;
    }
}
