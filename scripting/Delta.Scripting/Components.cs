using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Text;

namespace Delta;

/// <summary>
/// The component id of <typeparamref name="T"/> in the world scripts were
/// loaded into. Set when the declaring assembly is loaded.
/// </summary>
public static class ComponentType<T>
{
    internal static uint Value = Native.Invalid;

    public static uint Id => Value != Native.Invalid
        ? Value
        : throw new InvalidOperationException(
            $"{typeof(T).FullName} is not a registered component; declare it as an `unmanaged, IComponent` struct or a Behaviour in a loaded script assembly");
}

/// <summary>Describes script types to the host by reflection and registers them.</summary>
internal static unsafe class ComponentRegistry
{
    static readonly Dictionary<Type, uint> StructIds = new();

    public static string NameOf(Type t) =>
        t.GetCustomAttribute<ComponentNameAttribute>()?.Name ?? t.FullName ?? t.Name;

    static (uint ty, uint aux)? Primitive(Type t) =>
        t == typeof(sbyte) ? (Native.FtI8, 0u)
        : t == typeof(short) ? (Native.FtI16, 0u)
        : t == typeof(int) ? (Native.FtI32, 0u)
        : t == typeof(long) ? (Native.FtI64, 0u)
        : t == typeof(byte) ? (Native.FtU8, 0u)
        : t == typeof(ushort) ? (Native.FtU16, 0u)
        : t == typeof(uint) ? (Native.FtU32, 0u)
        : t == typeof(ulong) ? (Native.FtU64, 0u)
        : t == typeof(float) ? (Native.FtF32, 0u)
        : t == typeof(double) ? (Native.FtF64, 0u)
        : t == typeof(Entity) ? (Native.FtEntity, 0u)
        : null;

    static int SizeOf(Type t) => t == typeof(Entity) ? 8 : Marshal.SizeOf(t);

    static int AlignOf(Type t)
    {
        if (Primitive(t) is not null) return SizeOf(t);
        var fields = t.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic);
        return fields.Length == 0 ? 1 : fields.Max(f => AlignOf(f.FieldType));
    }

    /// <summary>Registers a struct nested inside a component and returns its reflect id.</summary>
    static uint RegisterNested(nint world, Type t)
    {
        if (StructIds.TryGetValue(t, out var cached)) return cached;
        var (fields, names) = Describe(world, t);
        var id = Host.RegisterComponent(world, NameOf(t), (nuint)SizeOf(t), (nuint)AlignOf(t), Native.KindStruct, fields, names);
        StructIds[t] = id;
        return id;
    }

    static (FieldDescFfi[], byte[][]) Describe(nint world, Type t)
    {
        var infos = t.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic);
        var fields = new FieldDescFfi[infos.Length];
        var names = new byte[infos.Length][];
        for (var i = 0; i < infos.Length; i++)
        {
            var f = infos[i];
            // Compiler-generated backing fields: `<X>k__BackingField` -> `X`.
            var name = f.Name.StartsWith('<') ? f.Name[1..f.Name.IndexOf('>')] : f.Name;
            names[i] = Encoding.UTF8.GetBytes(name);
            fields[i].Offset = (uint)Marshal.OffsetOf(t, f.Name);
            if (f.FieldType == typeof(bool) || f.FieldType == typeof(char))
                throw new NotSupportedException(
                    $"{t.FullName}.{name}: `{f.FieldType.Name}` is not blittable; use a byte/ushort field instead");
            if (Primitive(f.FieldType) is { } p)
            {
                fields[i].Ty = p.ty;
            }
            else if (f.FieldType.IsValueType && !f.FieldType.IsEnum)
            {
                fields[i].Ty = Native.FtStruct;
                fields[i].Aux = RegisterNested(world, f.FieldType);
            }
            else if (f.FieldType.IsEnum)
            {
                var (ty, _) = Primitive(Enum.GetUnderlyingType(f.FieldType))!.Value;
                fields[i].Ty = ty;
            }
            else
            {
                throw new NotSupportedException(
                    $"{t.FullName}.{name}: {f.FieldType.Name} cannot be stored in a component; use a handle");
            }
        }
        return (fields, names);
    }

    /// <summary>Registers an unmanaged component struct (or a tag, when it has no fields).</summary>
    public static uint RegisterStruct(nint world, Type t)
    {
        var infos = t.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic);
        uint id;
        if (infos.Length == 0)
        {
            id = Host.RegisterComponent(world, NameOf(t), 0, 1, Native.KindTag, [], []);
        }
        else
        {
            var (fields, names) = Describe(world, t);
            id = Host.RegisterComponent(world, NameOf(t), (nuint)SizeOf(t), (nuint)AlignOf(t), Native.KindUnmanaged, fields, names);
        }
        SetId(t, id);
        return id;
    }

    /// <summary>Registers a Behaviour class as a managed component: one 8-byte GC handle per row.</summary>
    public static uint RegisterBehaviour(nint world, Type t)
    {
        var id = Host.RegisterComponent(world, NameOf(t), 8, 8, Native.KindManaged, [], []);
        SetId(t, id);
        return id;
    }

    static void SetId(Type t, uint id) =>
        typeof(ComponentType<>).MakeGenericType(t)
            .GetField("Value", BindingFlags.Static | BindingFlags.NonPublic)!
            .SetValue(null, id);
}
