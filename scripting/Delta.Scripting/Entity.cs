using System;

namespace Delta;

/// <summary>
/// A reference to an entity: low 32 bits index, high 32 bits generation.
/// Blittable and 8 bytes, so it can be a field of a component.
/// </summary>
public readonly struct Entity : IEquatable<Entity>
{
    public readonly ulong Bits;

    public Entity(ulong bits) => Bits = bits;

    /// <summary>A reference to nothing.</summary>
    public static readonly Entity Null = new(ulong.MaxValue);

    public uint Index => (uint)Bits;
    public uint Generation => (uint)(Bits >> 32);
    public bool IsNull => Bits == ulong.MaxValue;

    public bool Equals(Entity other) => Bits == other.Bits;
    public override bool Equals(object? obj) => obj is Entity e && Equals(e);
    public override int GetHashCode() => Bits.GetHashCode();
    public override string ToString() => IsNull ? "Entity(null)" : $"Entity({Index}v{Generation})";
    public static bool operator ==(Entity a, Entity b) => a.Bits == b.Bits;
    public static bool operator !=(Entity a, Entity b) => a.Bits != b.Bits;
}
