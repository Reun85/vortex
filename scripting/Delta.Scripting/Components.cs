using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Reflection.Emit;
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
        : t == typeof(bool) ? (Native.FtBool, 0u)
        // The engine's math types are System.Numerics' (same layout).
        : t == typeof(System.Numerics.Vector2) ? (Native.FtVec2, 0u)
        : t == typeof(System.Numerics.Vector3) ? (Native.FtVec3, 0u)
        : t == typeof(System.Numerics.Vector4) ? (Native.FtVec4, 0u)
        : t == typeof(System.Numerics.Quaternion) ? (Native.FtQuat, 0u)
        : null;

    // Layout comes from the managed type, not from `Marshal`: Marshal
    // treats `bool` as the 4-byte Win32 BOOL, while a pointer into a
    // component column sees the managed 1-byte bool. Both numbers are
    // measured with IL (`sizeof`, `ldflda`), so they are what `T*` really does.
    static int SizeOf(Type t)
    {
        if (t == typeof(Entity)) return 8;
        var m = new DynamicMethod("sizeof", typeof(int), Type.EmptyTypes, typeof(ComponentRegistry).Module, true);
        var il = m.GetILGenerator();
        il.Emit(OpCodes.Sizeof, t);
        il.Emit(OpCodes.Ret);
        return (int)m.Invoke(null, null)!;
    }

    static int OffsetOf(Type t, FieldInfo f)
    {
        var m = new DynamicMethod("offsetof", typeof(int), Type.EmptyTypes, typeof(ComponentRegistry).Module, true);
        var il = m.GetILGenerator();
        var local = il.DeclareLocal(t);
        il.Emit(OpCodes.Ldloca, local);
        il.Emit(OpCodes.Ldflda, f);
        il.Emit(OpCodes.Conv_U);
        il.Emit(OpCodes.Ldloca, local);
        il.Emit(OpCodes.Conv_U);
        il.Emit(OpCodes.Sub);
        il.Emit(OpCodes.Conv_I4);
        il.Emit(OpCodes.Ret);
        return (int)m.Invoke(null, null)!;
    }

    /// <summary>The layout the host will be told about, for tests.</summary>
    internal static (int size, int[] offsets) Measure(Type t) =>
        (SizeOf(t), t.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic)
            .Select(f => OffsetOf(t, f)).ToArray());

    static int AlignOf(Type t)
    {
        if (t == typeof(System.Numerics.Vector2) || t == typeof(System.Numerics.Vector3)
            || t == typeof(System.Numerics.Vector4) || t == typeof(System.Numerics.Quaternion)) return 4;
        if (Primitive(t) is not null) return SizeOf(t);
        var fields = t.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic);
        return fields.Length == 0 ? 1 : fields.Max(f => AlignOf(f.FieldType));
    }

    /// <summary>Registers a struct nested inside a component and returns its reflect id.</summary>
    static uint RegisterNested(nint world, Type t)
    {
        if (StructIds.TryGetValue(t, out var cached)) return cached;
        var (fields, names, tips) = Describe(world, t);
        var id = Host.RegisterComponent(world, NameOf(t), (nuint)SizeOf(t), (nuint)AlignOf(t), Native.KindStruct, fields, names, tips);
        StructIds[t] = id;
        return id;
    }

    static (FieldDescFfi[], byte[][], byte[]?[]) Describe(nint world, Type t, bool allowNested = true)
    {
        var infos = t.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic);
        var fields = new FieldDescFfi[infos.Length];
        var names = new byte[infos.Length][];
        var tips = new byte[]?[infos.Length];
        for (var i = 0; i < infos.Length; i++)
        {
            var f = infos[i];
            // Compiler-generated backing fields: `<X>k__BackingField` -> `X`.
            var name = f.Name.StartsWith('<') ? f.Name[1..f.Name.IndexOf('>')] : f.Name;
            names[i] = Encoding.UTF8.GetBytes(name);
            fields[i].Offset = (uint)OffsetOf(t, f);
            if (f.FieldType == typeof(char))
                throw new NotSupportedException(
                    $"{t.FullName}.{name}: `char` is not supported; use a ushort field instead");
            ReadAttributes(f, ref fields[i], out tips[i]);
            if (f.GetCustomAttribute<System.Runtime.CompilerServices.FixedBufferAttribute>() is { } buffer)
            {
                if (Primitive(buffer.ElementType) is not { } elem)
                    throw new NotSupportedException($"{t.FullName}.{name}: fixed buffers of {buffer.ElementType.Name} are not supported");
                fields[i].Ty = Native.FtArray;
                fields[i].Aux = (uint)buffer.Length;
                fields[i].ElemTy = elem.ty;
            }
            else if (Primitive(f.FieldType) is { } p)
            {
                fields[i].Ty = p.ty;
            }
            else if (f.FieldType.IsValueType && !f.FieldType.IsEnum)
            {
                if (!allowNested)
                    throw new NotSupportedException($"{t.FullName}.{name}: nested structs are not supported in engine mirrors");
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
        return (fields, names, tips);
    }


    static void ReadAttributes(FieldInfo f, ref FieldDescFfi desc, out byte[]? tooltip)
    {
        // `public float X { get; set; }` puts the attributes on the property.
        var prop = f.Name.StartsWith('<')
            ? f.DeclaringType!.GetProperty(f.Name[1..f.Name.IndexOf('>')],
                BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic)
            : null;
        T? Find<T>() where T : Attribute => f.GetCustomAttribute<T>() ?? prop?.GetCustomAttribute<T>();

        if (Find<RangeAttribute>() is { } range)
        {
            desc.Flags |= Native.FfHasRange;
            desc.RangeLo = range.Min;
            desc.RangeHi = range.Max;
        }
        if (Find<HideInInspectorAttribute>() is not null) desc.Flags |= Native.FfHidden;
        if (Find<ReadOnlyAttribute>() is not null) desc.Flags |= Native.FfReadOnly;
        tooltip = Find<TooltipAttribute>() is { } tip ? Encoding.UTF8.GetBytes(tip.Text) : null;
    }

    /// <summary>Asks the host to compare the mirror <paramref name="t"/> with the Rust type called <paramref name="name"/>.</summary>
    internal static void CheckLayout(nint world, string name, Type t)
    {
        var (fields, names, tips) = Describe(world, t, allowNested: false);
        Host.CheckLayout(world, name, (nuint)SizeOf(t), (nuint)AlignOf(t), fields, names, tips);
    }

    /// <summary>Registers an unmanaged component struct (or a tag, when it has no fields).</summary>
    public static uint RegisterStruct(nint world, Type t)
    {
        var infos = t.GetFields(BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic);
        uint id;
        if (infos.Length == 0)
        {
            id = Host.RegisterComponent(world, NameOf(t), 0, 1, Native.KindTag, [], [], [], RequiredIds(t));
        }
        else
        {
            var (fields, names, tips) = Describe(world, t);
            id = Host.RegisterComponent(world, NameOf(t), (nuint)SizeOf(t), (nuint)AlignOf(t), Native.KindUnmanaged, fields, names, tips, RequiredIds(t));
        }
        SetId(t, id);
        return id;
    }

    /// <summary>Registers a Behaviour class as a managed component: one 8-byte GC handle per row.</summary>
    public static uint RegisterBehaviour(nint world, Type t)
    {
        var id = Host.RegisterComponent(world, NameOf(t), 8, 8, Native.KindManaged, [], [], [], RequiredIds(t));
        SetId(t, id);
        return id;
    }

    /// <summary>
    /// Drops the nested-struct entries of a load context's assemblies; those
    /// are cached by <see cref="Type"/> and would keep the context alive.
    /// </summary>
    public static void ForgetAssembliesOf(System.Runtime.Loader.AssemblyLoadContext context)
    {
        foreach (var assembly in context.Assemblies.ToArray())
            foreach (var type in StructIds.Keys.Where(k => k.Assembly == assembly).ToArray())
                StructIds.Remove(type);
    }

    /// <summary>Drops every reference this registry holds to a script type.</summary>
    public static void Forget(Type t)
    {
        StructIds.Remove(t);
        SetId(t, Native.Invalid);
    }

    internal static uint IdOf(Type t) =>
        (uint)typeof(ComponentType<>).MakeGenericType(t)
            .GetField("Value", BindingFlags.Static | BindingFlags.NonPublic)!.GetValue(null)!;

    /// <summary>The ids named by <c>[Requires]</c>; each must already be registered.</summary>
    static uint[]? RequiredIds(Type t)
    {
        if (t.GetCustomAttribute<RequiresAttribute>() is not { } attribute) return null;
        return attribute.Components.Select(r =>
        {
            var id = IdOf(r);
            if (id == Native.Invalid)
                throw new InvalidOperationException($"{t.FullName} requires {r.FullName}, which is not a registered component");
            return id;
        }).ToArray();
    }

    /// <summary>The script component types <paramref name="t"/> requires, for registration order.</summary>
    internal static IEnumerable<Type> RequiredTypes(Type t) =>
        t.GetCustomAttribute<RequiresAttribute>()?.Components ?? [];

    public static void SetId(Type t, uint id) =>
        typeof(ComponentType<>).MakeGenericType(t)
            .GetField("Value", BindingFlags.Static | BindingFlags.NonPublic)!
            .SetValue(null, id);
}
