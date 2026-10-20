using System;
using System.Collections;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Reflection;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Delta;

/// <summary>
/// Saves and restores a Behaviour's state across a script reload. Public
/// fields and read/write properties are kept, matched by name, so a class
/// can gain, lose and reorder members between versions.
/// <para>
/// This is deliberately not <c>JsonSerializer</c>: its reflection caches are
/// keyed by <see cref="Type"/> and live in runtime-wide statics, which keeps
/// the old script assembly loaded forever. Only the writer and the document
/// reader are used here, and nothing in this class is cached by type.
/// </para>
/// <para>
/// Mark a member with <c>[JsonIgnore]</c> to keep it out of the saved state.
/// Supported member types: numbers, bool, char, string, enums, Entity,
/// arrays, List, Dictionary with string keys, and nested classes or structs
/// of those. Anything else is skipped with a warning.
/// </para>
/// </summary>
internal static class BehaviourSerializer
{
    const int MaxDepth = 16;

    public static byte[] Save(Behaviour behaviour)
    {
        using var stream = new MemoryStream();
        using (var writer = new Utf8JsonWriter(stream))
            WriteObject(writer, behaviour, behaviour.GetType(), 0);
        return stream.ToArray();
    }

    /// <summary>
    /// Builds a <paramref name="type"/> instance from saved state. Members
    /// the saved state lacks keep their initializers; members the class no
    /// longer has, and values that no longer fit, are dropped with a warning.
    /// </summary>
    public static Behaviour Restore(Type type, ReadOnlySpan<byte> json)
    {
        var instance = (Behaviour)Activator.CreateInstance(type)!;
        try
        {
            using var document = JsonDocument.Parse(json.ToArray());
            ReadMembers(document.RootElement, instance, type, type.FullName ?? type.Name, 0);
        }
        catch (JsonException e)
        {
            Bridge.Warn($"{type.FullName}: saved state is unreadable ({e.Message}); starting from defaults");
        }
        return instance;
    }

    // ---- members -----------------------------------------------------------

    readonly record struct Member(string Name, Type Type, Func<object, object?> Get, Action<object, object?> Set);

    static IEnumerable<Member> MembersOf(Type t)
    {
        foreach (var f in t.GetFields(BindingFlags.Public | BindingFlags.Instance))
        {
            if (f.IsInitOnly || f.DeclaringType == typeof(Behaviour) || f.IsDefined(typeof(JsonIgnoreAttribute))) continue;
            yield return new Member(f.Name, f.FieldType, f.GetValue, f.SetValue);
        }
        foreach (var p in t.GetProperties(BindingFlags.Public | BindingFlags.Instance))
        {
            if (p.GetIndexParameters().Length != 0 || p.GetMethod is null || p.SetMethod is null) continue;
            if (p.DeclaringType == typeof(Behaviour) || p.IsDefined(typeof(JsonIgnoreAttribute))) continue;
            yield return new Member(p.Name, p.PropertyType, p.GetValue, p.SetValue);
        }
    }

    // ---- writing -----------------------------------------------------------

    static void WriteObject(Utf8JsonWriter w, object obj, Type type, int depth)
    {
        w.WriteStartObject();
        foreach (var m in MembersOf(type))
        {
            if (!Supported(m.Type, 0))
            {
                Bridge.Warn($"{type.FullName}.{m.Name}: type {m.Type.Name} cannot be saved, the field is skipped");
                continue;
            }
            w.WritePropertyName(m.Name);
            WriteValue(w, m.Get(obj), m.Type, depth + 1);
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
        if (typeof(Delegate).IsAssignableFrom(t) || t.IsInterface || t.IsAbstract || t.IsPointer) return false;
        return t.IsClass || t.IsValueType;
    }

    static void WriteFloat(Utf8JsonWriter w, double d)
    {
        if (double.IsFinite(d)) w.WriteNumberValue(d);
        else w.WriteStringValue(double.IsNaN(d) ? "NaN" : d > 0 ? "inf" : "-inf");
    }

    static void WriteValue(Utf8JsonWriter w, object? v, Type t, int depth)
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
            case Entity x: w.WriteNumberValue(x.Bits); return;
            case Enum x: w.WriteStringValue(x.ToString()); return;
            case IDictionary d:
                w.WriteStartObject();
                foreach (DictionaryEntry e in d)
                {
                    w.WritePropertyName((string)e.Key);
                    WriteValue(w, e.Value, t.GetGenericArguments()[1], depth + 1);
                }
                w.WriteEndObject();
                return;
            case IEnumerable seq:
                var element = t.IsArray ? t.GetElementType()! : t.GetGenericArguments()[0];
                w.WriteStartArray();
                foreach (var item in seq) WriteValue(w, item, element, depth + 1);
                w.WriteEndArray();
                return;
            default:
                WriteObject(w, v, v.GetType(), depth);
                return;
        }
    }

    // ---- reading -----------------------------------------------------------

    static void ReadMembers(JsonElement json, object target, Type type, string path, int depth)
    {
        if (json.ValueKind != JsonValueKind.Object) throw new JsonException($"{path}: expected an object");
        var members = MembersOf(type).Where(m => Supported(m.Type, 0)).ToDictionary(m => m.Name);
        foreach (var property in json.EnumerateObject())
        {
            if (!members.TryGetValue(property.Name, out var m))
            {
                Bridge.Warn($"{path}: field `{property.Name}` no longer exists, its value is dropped");
                continue;
            }
            try
            {
                m.Set(target, ReadValue(property.Value, m.Type, $"{path}.{m.Name}", depth + 1));
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

    static object? ReadValue(JsonElement e, Type t, string path, int depth)
    {
        if (depth > MaxDepth) throw new JsonException($"{path}: nests too deeply");
        if (e.ValueKind == JsonValueKind.Null)
            return t.IsValueType ? Activator.CreateInstance(t) : null;
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
        if (t == typeof(Entity)) return new Entity(e.GetUInt64());
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
            var items = e.EnumerateArray().Select((x, i) => ReadValue(x, element, $"{path}[{i}]", depth + 1)).ToArray();
            var array = Array.CreateInstance(element, items.Length);
            for (var i = 0; i < items.Length; i++) array.SetValue(items[i], i);
            return array;
        }
        if (t.IsGenericType && t.GetGenericTypeDefinition() == typeof(List<>))
        {
            var element = t.GetGenericArguments()[0];
            var list = (IList)Activator.CreateInstance(t)!;
            var i = 0;
            foreach (var x in e.EnumerateArray()) list.Add(ReadValue(x, element, $"{path}[{i++}]", depth + 1));
            return list;
        }
        if (t.IsGenericType && t.GetGenericTypeDefinition() == typeof(Dictionary<,>))
        {
            var value = t.GetGenericArguments()[1];
            var dict = (IDictionary)Activator.CreateInstance(t)!;
            foreach (var p in e.EnumerateObject()) dict[p.Name] = ReadValue(p.Value, value, $"{path}.{p.Name}", depth + 1);
            return dict;
        }
        var obj = Activator.CreateInstance(t)!;
        ReadMembers(e, obj, t, path, depth);
        return obj;
    }
}
