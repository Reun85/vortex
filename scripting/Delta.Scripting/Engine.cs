using System;
using System.Collections.Generic;
using System.Numerics;
using System.Reflection;

namespace Delta;

/// <summary>
/// Marks a struct as the C# mirror of a Rust resource exposed to scripts by
/// name. At load the bridge sends its layout to the host, which compares it
/// with the Rust reflection and refuses to load on any mismatch.
/// </summary>
[AttributeUsage(AttributeTargets.Struct)]
public sealed class EngineResourceAttribute(string name) : Attribute
{
    public string Name { get; } = name;
}

/// <summary>
/// Marks a struct as the C# mirror of a Rust component, bound by name at
/// load time like <see cref="EngineResourceAttribute"/>.
/// </summary>
[AttributeUsage(AttributeTargets.Struct)]
public sealed class EngineComponentAttribute(string name) : Attribute
{
    public string Name { get; } = name;
}

/// <summary>The id the host gave a resource type, set when its assembly loads.</summary>
internal static class ResourceType<T> where T : unmanaged
{
    internal static uint Value = Native.Invalid;
}

/// <summary>
/// Access to a resource the host exposes by name. Each access is one host
/// call; do not keep the reference across systems, because inserting a
/// resource replaces its storage.
/// </summary>
public static unsafe class Resource<T> where T : unmanaged
{
    static uint Id => ResourceType<T>.Value != Native.Invalid
        ? ResourceType<T>.Value
        : throw new InvalidOperationException(
            $"{typeof(T).FullName} is not bound to an engine resource; mark it [EngineResource(\"name\")] in a loaded assembly");

    static byte* Pointer(byte write)
    {
        var world = World.Current;
        var p = Host.Api->ResourcePtr(world.Ptr, Id, write);
        if (p == null) throw new InvalidOperationException($"resource {typeof(T).Name}: {Host.LastError(world.Ptr)}");
        return p;
    }

    public static ref readonly T Read => ref *(T*)Pointer(0);

    public static ref T Write => ref *(T*)Pointer(1);
}

/// <summary>Time for the running system, as in Unity: inside <c>FixedUpdate</c> <see cref="DeltaTime"/> is the fixed step.</summary>
public static class Time
{
    public static float DeltaTime
    {
        get
        {
            ref readonly var t = ref Resource<TimeData>.Read;
            return t.InFixed != 0 ? t.FixedDelta : t.Delta;
        }
    }

    public static float FixedDeltaTime => Resource<TimeData>.Read.FixedDelta;
    public static double ElapsedTime => Resource<TimeData>.Read.Elapsed;
    public static ulong Frame => Resource<TimeData>.Read.Frame;
}

/// <summary>Keyboard, mouse buttons and named axes. Numbering matches <c>delta::KeyCode</c>.</summary>
public static unsafe class Input
{
    static readonly Dictionary<string, uint> Axes = new();

    /// <summary>Forgets resolved axis names; called when scripts unload.</summary>
    internal static void Forget() => Axes.Clear();

    static bool Bit(ulong* words, KeyCode key)
    {
        var k = (int)key;
        return (words[k / 64] >> (k % 64) & 1) != 0;
    }

    public static bool GetKey(KeyCode key) { fixed (ulong* w = Resource<InputData>.Read.Down) return Bit(w, key); }
    public static bool GetKeyDown(KeyCode key) { fixed (ulong* w = Resource<InputData>.Read.Pressed) return Bit(w, key); }
    public static bool GetKeyUp(KeyCode key) { fixed (ulong* w = Resource<InputData>.Read.Released) return Bit(w, key); }

    /// <summary>The value of a named axis, such as "Horizontal".</summary>
    public static float Axis(string name)
    {
        if (!Axes.TryGetValue(name, out var slot))
        {
            var world = World.Current;
            var bytes = System.Text.Encoding.UTF8.GetBytes(name);
            fixed (byte* p = bytes)
                slot = Host.Check(world.Ptr, Host.Api->InputAxisId(world.Ptr, p, (nuint)bytes.Length), $"input axis `{name}`");
            Axes[name] = slot;
        }
        fixed (float* a = Resource<InputData>.Read.Axes) return a[slot];
    }

    public static Vector2 Axis(string x, string y) => new(Axis(x), Axis(y));
}

/// <summary>Binds the engine's resource mirrors to the host's resources.</summary>
internal static unsafe class EngineBinding
{
    static bool coreBound;

    /// <summary>
    /// Checks every <see cref="EngineResourceAttribute"/> mirror in
    /// <paramref name="types"/> against the host and remembers its id. Throws,
    /// naming the field, when a mirror does not match.
    /// </summary>
    public static void Bind(nint world, IEnumerable<Type> types, List<Type> bound)
    {
        if (!coreBound)
        {
            BindAll(world, typeof(Bridge).Assembly.GetTypes(), []);
            coreBound = true;
        }
        BindAll(world, types, bound);
    }

    /// <summary>Unbinds mirrors declared by a script assembly that is being unloaded.</summary>
    public static void Forget(List<Type> bound)
    {
        foreach (var t in bound)
        {
            if (t.GetCustomAttribute<EngineComponentAttribute>() is not null)
            {
                ComponentRegistry.SetId(t, Native.Invalid);
                continue;
            }
            typeof(ResourceType<>).MakeGenericType(t)
                .GetField("Value", BindingFlags.Static | BindingFlags.NonPublic)!
                .SetValue(null, Native.Invalid);
        }
        bound.Clear();
        Input.Forget();
    }

    static void BindAll(nint world, IEnumerable<Type> types, List<Type> bound)
    {
        foreach (var t in types)
        {
            if (t.GetCustomAttribute<EngineComponentAttribute>() is { } component)
            {
                ComponentRegistry.CheckLayout(world, component.Name, t);
                var cid = Host.ComponentId(world, component.Name);
                ComponentRegistry.SetId(t, cid);
                bound.Add(t);
                continue;
            }
            if (t.GetCustomAttribute<EngineResourceAttribute>() is not { } attribute) continue;
            ComponentRegistry.CheckLayout(world, attribute.Name, t);
            var id = Host.Check(world, Host.ResourceId(world, attribute.Name), $"binding `{attribute.Name}`");
            typeof(ResourceType<>).MakeGenericType(t)
                .GetField("Value", BindingFlags.Static | BindingFlags.NonPublic)!
                .SetValue(null, id);
            bound.Add(t);
        }
    }
}
