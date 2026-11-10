using System;
using System.Collections;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Delta;

/// <summary>Which members of a behaviour are written, and how entities appear.</summary>
internal enum SerializeMode : uint
{
    /// <summary>
    /// Scenes, the inspector, undo and play mode: public fields and
    /// properties plus <c>[SerializeField]</c> private fields, minus
    /// <c>[NonSerialized]</c>. Entities are written as scene ids.
    /// </summary>
    Persist = 0,

    /// <summary>
    /// Hot reload: every instance field, public or private (including the
    /// backing fields of auto-properties), minus <c>[NonSerialized]</c> and
    /// types that cannot survive a reload. Entities are raw handles, since
    /// the world survives a reload.
    /// </summary>
    Snapshot = 1,
}

/// <summary>
/// Saves and restores a Behaviour's state, matched by member name so a class
/// can gain, lose and reorder members between versions, and edits single
/// members of a live object by path.
/// <para>
/// This is deliberately not <c>JsonSerializer</c>: its reflection caches are
/// keyed by <see cref="Type"/> and live in runtime-wide statics, which keeps
/// the old script assembly loaded forever. Only the writer and the document
/// reader are used here, and nothing in this class is cached by type.
/// </para>
/// <para>
/// Supported member types: numbers, bool, char, string, enums, Entity,
/// arrays, List, Dictionary with string keys, and nested classes or structs
/// of those (System.Numerics vectors included). Anything else is skipped
/// with a warning in <see cref="SerializeMode.Persist"/>, silently in
/// <see cref="SerializeMode.Snapshot"/> when it can never survive a reload
/// (delegates, pointers, <c>ComponentRef</c>).
/// </para>
/// </summary>
internal static class BehaviourSerializer
{
    const int MaxDepth = 16;

    /// <summary>Reserved member of a Snapshot's root object: lifecycle state that is not a field.</summary>
    const string StartedKey = "$started";

    // ---- entry points -------------------------------------------------------

    public static byte[] Save(Behaviour behaviour, SerializeMode mode)
    {
        using var stream = new MemoryStream();
        using (var writer = new Utf8JsonWriter(stream))
            WriteObject(writer, behaviour, behaviour.GetType(), mode, 0,
                mode == SerializeMode.Snapshot ? behaviour.Started : null);
        return stream.ToArray();
    }

    /// <summary>
    /// Builds a <paramref name="type"/> instance from saved state. Members
    /// the saved state lacks keep their initializers; members the class no
    /// longer has, and values that no longer fit, are dropped with a warning.
    /// A persisted instance then gets <see cref="Behaviour.OnAfterDeserialize"/>.
    /// </summary>
    public static Behaviour Restore(Type type, ReadOnlySpan<byte> json, SerializeMode mode)
    {
        var instance = (Behaviour)Activator.CreateInstance(type)!;
        try
        {
            using var document = JsonDocument.Parse(json.ToArray());
            ReadMembers(document.RootElement, instance, type, mode, type.FullName ?? type.Name, 0, enforceReadOnly: false);
            if (document.RootElement.TryGetProperty(StartedKey, out var started))
                instance.Started = started.GetBoolean();
        }
        catch (JsonException e)
        {
            Bridge.Warn($"{type.FullName}: saved state is unreadable ({e.Message}); starting from defaults");
        }
        if (mode == SerializeMode.Persist) instance.RaiseAfterDeserialize();
        return instance;
    }

    /// <summary>A fresh instance with default contents, persisted: what a newly added component starts as.</summary>
    public static byte[] Default(Type type)
    {
        var instance = (Behaviour)Activator.CreateInstance(type)!;
        return Save(instance, SerializeMode.Persist);
    }

    // ---- members -----------------------------------------------------------

    internal readonly record struct Member(
        string Name, Type Type, Func<object, object?> Get, Action<object, object?> Set, MemberInfo Info);

    static bool IsStop(Type? t) =>
        t is null || t == typeof(object) || t == typeof(ValueType) || t == typeof(Behaviour);

    /// <summary>Types that can never be saved: delegates, pointers, references to the host.</summary>
    static bool Unsaveable(Type t) =>
        typeof(Delegate).IsAssignableFrom(t) || t.IsPointer || t.IsByRef || t == typeof(Type)
        || (t.IsGenericType && t.GetGenericTypeDefinition().Name.StartsWith("ComponentRef", StringComparison.Ordinal));

    static bool Ignored(MemberInfo m) =>
        m.IsDefined(typeof(NonSerializedAttribute)) || m.IsDefined(typeof(JsonIgnoreAttribute));

    internal static IEnumerable<Member> MembersOf(Type start, SerializeMode mode)
    {
        const BindingFlags All = BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly;
        var seen = new HashSet<string>();
        for (var t = start; !IsStop(t); t = t!.BaseType)
        {
            foreach (var f in t!.GetFields(All))
            {
                if (Ignored(f)) continue;
                var backing = f.Name.StartsWith('<');
                // The backing field of `[field: SerializeField] int X { get; set; }` is X.
                var name = backing ? f.Name[1..f.Name.IndexOf('>')] : f.Name;
                if (mode == SerializeMode.Persist)
                {
                    if (f.IsInitOnly) continue;
                    var visible = backing ? f.IsDefined(typeof(SerializeFieldAttribute)) : f.IsPublic || f.IsDefined(typeof(SerializeFieldAttribute));
                    if (!visible) continue;
                }
                else
                {
                    name = f.Name; // snapshots keep the raw name; backing fields included
                }
                yield return new Member(Unique(seen, name, t), f.FieldType, f.GetValue, f.SetValue, f);
            }
            if (mode != SerializeMode.Persist) continue;
            foreach (var p in t.GetProperties(All))
            {
                if (p.GetIndexParameters().Length != 0 || p.GetMethod is not { IsPublic: true } || p.SetMethod is not { IsPublic: true }) continue;
                if (Ignored(p)) continue;
                yield return new Member(Unique(seen, p.Name, t), p.PropertyType, p.GetValue, p.SetValue, p);
            }
        }
    }

    /// <summary>A name unique within the class hierarchy: a shadowed base member gets <c>Name@Base</c>.</summary>
    static string Unique(HashSet<string> seen, string name, Type declaring)
    {
        if (seen.Add(name)) return name;
        var qualified = $"{name}@{declaring.Name}";
        seen.Add(qualified);
        return qualified;
    }

    static bool Savable(Member m, SerializeMode mode, Type owner)
    {
        if (Unsaveable(m.Type)) return false;
        if (Supported(m.Type, 0)) return true;
        if (mode == SerializeMode.Persist)
            Bridge.Warn($"{owner.FullName}.{m.Name}: type {m.Type.Name} cannot be saved, the field is skipped");
        return false;
    }

    // ---- writing -----------------------------------------------------------

    static void WriteObject(Utf8JsonWriter w, object obj, Type type, SerializeMode mode, int depth, bool? started = null)
    {
        w.WriteStartObject();
        if (started is { } s) w.WriteBoolean(StartedKey, s);
        foreach (var m in MembersOf(type, mode))
        {
            if (!Savable(m, mode, type)) continue;
            w.WritePropertyName(m.Name);
            WriteValue(w, m.Get(obj), m.Type, mode, depth + 1);
        }
        w.WriteEndObject();
    }

    static bool IsLeaf(Type t) =>
        t.IsPrimitive || t.IsEnum || t == typeof(string) || t == typeof(decimal) || t == typeof(Entity);

    static bool Supported(Type t, int depth)
    {
        if (depth > MaxDepth) return false;
        if (IsLeaf(t)) return t != typeof(IntPtr) && t != typeof(UIntPtr);
        if (t.IsArray) return t.GetArrayRank() == 1 && Supported(t.GetElementType()!, depth + 1);
        if (t.IsGenericType)
        {
            var def = t.GetGenericTypeDefinition();
            if (def == typeof(List<>)) return Supported(t.GetGenericArguments()[0], depth + 1);
            if (def == typeof(Dictionary<,>))
            {
                var a = t.GetGenericArguments();
                return a[0] == typeof(string) && Supported(a[1], depth + 1);
            }
        }
        if (Unsaveable(t) || t.IsInterface || t.IsAbstract) return false;
        return t.IsClass || t.IsValueType;
    }

    static void WriteFloat(Utf8JsonWriter w, double d)
    {
        if (double.IsFinite(d)) w.WriteNumberValue(d);
        else w.WriteStringValue(double.IsNaN(d) ? "NaN" : d > 0 ? "inf" : "-inf");
    }

    static void WriteValue(Utf8JsonWriter w, object? v, Type t, SerializeMode mode, int depth)
    {
        if (v is null) { w.WriteNullValue(); return; }
        if (depth > MaxDepth) throw new InvalidOperationException("value nests too deeply (a cycle?)");
        switch (v)
        {
            case bool b: w.WriteBooleanValue(b); return;
            case sbyte x: w.WriteNumberValue(x); return;
            case short x: w.WriteNumberValue(x); return;
            case int x: w.WriteNumberValue(x); return;
            case long x: w.WriteNumberValue(x); return;
            case byte x: w.WriteNumberValue(x); return;
            case ushort x: w.WriteNumberValue(x); return;
            case uint x: w.WriteNumberValue(x); return;
            case ulong x: w.WriteNumberValue(x); return;
            case float x: WriteFloat(w, x); return;
            case double x: WriteFloat(w, x); return;
            case decimal x: w.WriteNumberValue(x); return;
            case char x: w.WriteStringValue(x.ToString()); return;
            case string x: w.WriteStringValue(x); return;
            case Entity x:
                if (mode == SerializeMode.Snapshot) w.WriteNumberValue(x.Bits);
                else if (Host.EntityToSceneId(x) is { } id) w.WriteStringValue(id);
                else w.WriteNullValue();
                return;
            case Enum x: w.WriteStringValue(x.ToString()); return;
            case IDictionary d:
                w.WriteStartObject();
                foreach (DictionaryEntry e in d)
                {
                    w.WritePropertyName((string)e.Key);
                    WriteValue(w, e.Value, t.GetGenericArguments()[1], mode, depth + 1);
                }
                w.WriteEndObject();
                return;
            case IEnumerable seq:
                var element = t.IsArray ? t.GetElementType()! : t.GetGenericArguments()[0];
                w.WriteStartArray();
                foreach (var item in seq) WriteValue(w, item, element, mode, depth + 1);
                w.WriteEndArray();
                return;
            default:
                WriteObject(w, v, v.GetType(), mode, depth);
                return;
        }
    }

    // ---- reading -----------------------------------------------------------

    static void ReadMembers(JsonElement json, object target, Type type, SerializeMode mode, string path, int depth, bool enforceReadOnly)
    {
        if (json.ValueKind != JsonValueKind.Object) throw new JsonException($"{path}: expected an object");
        var members = MembersOf(type, mode).Where(m => !Unsaveable(m.Type) && Supported(m.Type, 0)).ToDictionary(m => m.Name);
        foreach (var property in json.EnumerateObject())
        {
            if (depth == 0 && property.Name == StartedKey) continue;
            if (!members.TryGetValue(property.Name, out var m))
            {
                Bridge.Warn($"{path}: field `{property.Name}` no longer exists, its value is dropped");
                continue;
            }
            if (enforceReadOnly && m.Info.IsDefined(typeof(ReadOnlyAttribute)))
                throw new JsonException($"{path}.{m.Name} is read-only");
            try
            {
                m.Set(target, ReadValue(property.Value, m.Type, mode, $"{path}.{m.Name}", depth + 1));
            }
            catch (Exception e) when (e is JsonException or InvalidCastException or FormatException or OverflowException or ArgumentException)
            {
                Bridge.Warn($"{path}.{m.Name}: the saved value does not fit {m.Type.Name} ({e.Message}); kept the default");
            }
        }
    }

    static double ReadFloat(JsonElement e) => e.ValueKind switch
    {
        JsonValueKind.Number => e.GetDouble(),
        JsonValueKind.String => e.GetString() switch
        {
            "NaN" => double.NaN,
            "inf" => double.PositiveInfinity,
            "-inf" => double.NegativeInfinity,
            _ => throw new JsonException("expected a number"),
        },
        _ => throw new JsonException("expected a number"),
    };

    static object? ReadValue(JsonElement e, Type t, SerializeMode mode, string path, int depth)
    {
        if (depth > MaxDepth) throw new JsonException($"{path}: nests too deeply");
        if (e.ValueKind == JsonValueKind.Null)
            return t == typeof(Entity) ? Entity.Null : t.IsValueType ? Activator.CreateInstance(t) : null;
        if (t == typeof(bool)) return e.GetBoolean();
        if (t == typeof(sbyte)) return e.GetSByte();
        if (t == typeof(short)) return e.GetInt16();
        if (t == typeof(int)) return e.GetInt32();
        if (t == typeof(long)) return e.GetInt64();
        if (t == typeof(byte)) return e.GetByte();
        if (t == typeof(ushort)) return e.GetUInt16();
        if (t == typeof(uint)) return e.GetUInt32();
        if (t == typeof(ulong)) return e.GetUInt64();
        if (t == typeof(float)) return (float)ReadFloat(e);
        if (t == typeof(double)) return ReadFloat(e);
        if (t == typeof(decimal)) return e.GetDecimal();
        if (t == typeof(char)) return e.GetString() is { Length: 1 } s ? s[0] : throw new JsonException("expected one character");
        if (t == typeof(string)) return e.ValueKind == JsonValueKind.String ? e.GetString() : throw new JsonException("expected a string");
        if (t == typeof(Entity))
        {
            if (e.ValueKind == JsonValueKind.String) return Host.SceneIdToEntity(e.GetString()!);
            return new Entity(e.GetUInt64());
        }
        if (t.IsEnum)
        {
            // By name, so reordering values between versions is safe; a name
            // that was removed is a mismatch, reported by the caller.
            if (e.ValueKind == JsonValueKind.String && Enum.TryParse(t, e.GetString(), out var named)) return named;
            if (e.ValueKind == JsonValueKind.Number) return Enum.ToObject(t, e.GetInt64());
            throw new JsonException($"`{e}` is not a value of {t.Name}");
        }
        if (t.IsArray)
        {
            var element = t.GetElementType()!;
            var items = e.EnumerateArray().Select((x, i) => ReadValue(x, element, mode, $"{path}[{i}]", depth + 1)).ToArray();
            var array = Array.CreateInstance(element, items.Length);
            for (var i = 0; i < items.Length; i++) array.SetValue(items[i], i);
            return array;
        }
        if (t.IsGenericType && t.GetGenericTypeDefinition() == typeof(List<>))
        {
            var element = t.GetGenericArguments()[0];
            var list = (IList)Activator.CreateInstance(t)!;
            var i = 0;
            foreach (var x in e.EnumerateArray()) list.Add(ReadValue(x, element, mode, $"{path}[{i++}]", depth + 1));
            return list;
        }
        if (t.IsGenericType && t.GetGenericTypeDefinition() == typeof(Dictionary<,>))
        {
            var value = t.GetGenericArguments()[1];
            var dict = (IDictionary)Activator.CreateInstance(t)!;
            foreach (var p in e.EnumerateObject()) dict[p.Name] = ReadValue(p.Value, value, mode, $"{path}.{p.Name}", depth + 1);
            return dict;
        }
        var obj = Activator.CreateInstance(t)!;
        ReadMembers(e, obj, t, mode, path, depth, enforceReadOnly: false);
        return obj;
    }

    // ---- paths -------------------------------------------------------------

    readonly record struct Segment(string? Name, int Index);

    /// <summary>Parses <c>a.b[2].c</c>; the empty path is the whole object.</summary>
    static List<Segment> ParsePath(string path)
    {
        var segments = new List<Segment>();
        var i = 0;
        while (i < path.Length)
        {
            if (path[i] == '.') { i++; continue; }
            if (path[i] == '[')
            {
                var close = path.IndexOf(']', i);
                if (close < 0 || !int.TryParse(path.AsSpan(i + 1, close - i - 1), out var index) || index < 0)
                    throw new JsonException($"bad index in path `{path}`");
                segments.Add(new Segment(null, index));
                i = close + 1;
                continue;
            }
            var start = i;
            while (i < path.Length && path[i] != '.' && path[i] != '[') i++;
            segments.Add(new Segment(path[start..i], -1));
        }
        return segments;
    }

    static Member FindMember(object target, string name, string path)
    {
        foreach (var m in MembersOf(target.GetType(), SerializeMode.Persist))
            if (m.Name == name && Supported(m.Type, 0)) return m;
        throw new JsonException($"`{path}`: {target.GetType().Name} has no editable member `{name}`");
    }

    /// <summary>One member (or the whole object, with an empty path) as JSON.</summary>
    public static byte[] GetMember(Behaviour behaviour, string path)
    {
        object? current = behaviour;
        var type = behaviour.GetType();
        foreach (var s in ParsePath(path))
        {
            (current, type) = Step(current, type, s, path);
        }
        using var stream = new MemoryStream();
        using (var writer = new Utf8JsonWriter(stream))
        {
            if (current is Behaviour b) WriteObject(writer, b, b.GetType(), SerializeMode.Persist, 0);
            else WriteValue(writer, current, type, SerializeMode.Persist, 0);
        }
        return stream.ToArray();
    }

    static (object?, Type) Step(object? current, Type type, Segment s, string path)
    {
        if (current is null) throw new JsonException($"`{path}`: null in the middle of the path");
        if (s.Name is null)
        {
            if (current is Array a && s.Index < a.Length) return (a.GetValue(s.Index), type.GetElementType()!);
            if (current is IList l && s.Index < l.Count) return (l[s.Index], type.GetGenericArguments()[0]);
            throw new JsonException($"`{path}`: no element {s.Index}");
        }
        if (current is IDictionary d)
        {
            if (!d.Contains(s.Name)) throw new JsonException($"`{path}`: no key `{s.Name}`");
            return (d[s.Name], type.GetGenericArguments()[1]);
        }
        var m = FindMember(current, s.Name, path);
        return (m.Get(current), m.Type);
    }

    /// <summary>
    /// Sets one member of the live object from JSON and returns the previous
    /// value as JSON. The behaviour is never replaced, so other scripts that
    /// hold a reference to it, and its private runtime state, are untouched.
    /// </summary>
    public static byte[] SetMember(Behaviour behaviour, string path, ReadOnlySpan<byte> json)
    {
        var old = GetMember(behaviour, path);
        using var document = JsonDocument.Parse(json.ToArray());
        var segments = ParsePath(path);
        if (segments.Count == 0)
        {
            ReadMembers(document.RootElement, behaviour, behaviour.GetType(), SerializeMode.Persist, behaviour.GetType().FullName ?? "", 0, enforceReadOnly: true);
            return old;
        }
        SetPath(behaviour, behaviour.GetType(), segments, 0, document.RootElement, path);
        return old;
    }

    /// <summary>Sets the value the path leads to inside <paramref name="current"/>; returns the (possibly new, for a struct) container.</summary>
    static object SetPath(object current, Type type, List<Segment> segments, int i, JsonElement value, string path)
    {
        var s = segments[i];
        var last = i == segments.Count - 1;
        if (s.Name is null)
        {
            var (child, childType) = Step(current, type, s, path);
            var replacement = last
                ? ReadValue(value, childType, SerializeMode.Persist, path, 0)
                : SetPath(child!, childType, segments, i + 1, value, path);
            if (current is Array a) a.SetValue(replacement, s.Index);
            else ((IList)current)[s.Index] = replacement;
            return current;
        }
        if (current is IDictionary d)
        {
            var (child, childType) = Step(current, type, s, path);
            d[s.Name] = last
                ? ReadValue(value, childType, SerializeMode.Persist, path, 0)
                : SetPath(child!, childType, segments, i + 1, value, path);
            return current;
        }
        var m = FindMember(current, s.Name, path);
        if (m.Info.IsDefined(typeof(ReadOnlyAttribute))) throw new JsonException($"`{path}` is read-only");
        object? newChild;
        if (last) newChild = ReadValue(value, m.Type, SerializeMode.Persist, path, 0);
        else
        {
            var existing = m.Get(current) ?? throw new JsonException($"`{path}`: null in the middle of the path");
            newChild = SetPath(existing, m.Type, segments, i + 1, value, path);
        }
        m.Set(current, newChild);
        return current;
    }

    // ---- schema ------------------------------------------------------------

    static string TypeName(Type t) =>
        t == typeof(float) ? "float" : t == typeof(double) ? "double" : t == typeof(int) ? "int"
        : t == typeof(bool) ? "bool" : t == typeof(string) ? "string" : t == typeof(Entity) ? "Entity"
        : t.IsEnum ? $"enum {t.Name}" : t.IsArray ? TypeName(t.GetElementType()!) + "[]" : t.Name;

    /// <summary>The members the inspector shows, as a JSON array.</summary>
    public static byte[] Schema(Type type)
    {
        using var stream = new MemoryStream();
        using (var w = new Utf8JsonWriter(stream))
        {
            w.WriteStartArray();
            foreach (var m in MembersOf(type, SerializeMode.Persist))
            {
                if (Unsaveable(m.Type) || !Supported(m.Type, 0)) continue;
                w.WriteStartObject();
                w.WriteString("name", m.Name);
                w.WriteString("type", TypeName(m.Type));
                if (Attr<RangeAttribute>(m.Info) is { } range)
                {
                    w.WritePropertyName("range");
                    w.WriteStartArray();
                    w.WriteNumberValue(range.Min);
                    w.WriteNumberValue(range.Max);
                    w.WriteEndArray();
                }
                if (Attr<TooltipAttribute>(m.Info) is { } tip) w.WriteString("tooltip", tip.Text);
                w.WriteBoolean("hidden", Attr<HideInInspectorAttribute>(m.Info) is not null);
                w.WriteBoolean("read_only", Attr<ReadOnlyAttribute>(m.Info) is not null);
                w.WriteEndObject();
            }
            w.WriteEndArray();
        }
        return stream.ToArray();
    }

    /// <summary>An attribute on a member, looking through an auto-property's backing field to the property.</summary>
    static T? Attr<T>(MemberInfo m) where T : Attribute
    {
        if (m.GetCustomAttribute<T>() is { } direct) return direct;
        if (m is FieldInfo f && f.Name.StartsWith('<'))
            return f.DeclaringType!.GetProperty(f.Name[1..f.Name.IndexOf('>')],
                BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic)?.GetCustomAttribute<T>();
        return null;
    }

    // ---- entity references -------------------------------------------------

    /// <summary>Rewrites every <c>Entity</c> equal to <paramref name="from"/> inside the behaviour; returns how many.</summary>
    public static int RemapEntities(Behaviour behaviour, ulong from, ulong to)
    {
        var count = 0;
        var visited = new HashSet<object>(ReferenceEqualityComparer.Instance);
        Remap(behaviour, behaviour.GetType(), from, to, ref count, visited, 0);
        return count;
    }

    static object? Remap(object? value, Type type, ulong from, ulong to, ref int count, HashSet<object> visited, int depth)
    {
        if (value is null || depth > MaxDepth) return value;
        if (value is Entity e) { if (e.Bits != from) return value; count++; return new Entity(to); }
        if (IsLeaf(type) || Unsaveable(type)) return value;
        if (!type.IsValueType && !visited.Add(value)) return value;
        if (value is Array array)
        {
            var element = type.GetElementType()!;
            for (var i = 0; i < array.Length; i++) array.SetValue(Remap(array.GetValue(i), element, from, to, ref count, visited, depth + 1), i);
            return value;
        }
        if (value is IList list)
        {
            var element = type.GetGenericArguments()[0];
            for (var i = 0; i < list.Count; i++) list[i] = Remap(list[i], element, from, to, ref count, visited, depth + 1);
            return value;
        }
        if (value is IDictionary dict)
        {
            var element = type.GetGenericArguments()[1];
            foreach (var key in dict.Keys.Cast<object>().ToArray()) dict[key] = Remap(dict[key], element, from, to, ref count, visited, depth + 1);
            return value;
        }
        foreach (var m in MembersOf(value.GetType(), SerializeMode.Snapshot))
        {
            if (Unsaveable(m.Type) || !Supported(m.Type, 0)) continue;
            var before = m.Get(value);
            var after = Remap(before, m.Type, from, to, ref count, visited, depth + 1);
            if (!ReferenceEquals(before, after) || m.Type.IsValueType) m.Set(value, after);
        }
        return value;
    }
}
