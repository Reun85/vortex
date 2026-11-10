using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

namespace Delta;

/// <summary>Calls into the Rust host.</summary>
internal static unsafe class Host
{
    public static HostApi* Api;
    public static ScriptApi* Script;

    public static void Log(uint level, string message)
    {
        if (Api == null) { Console.Error.WriteLine(message); return; }
        var bytes = Encoding.UTF8.GetBytes(message);
        fixed (byte* p = bytes) Api->Log(level, p, (nuint)bytes.Length);
    }

    public static string LastError(nint world)
    {
        var buffer = new byte[1024];
        fixed (byte* p = buffer)
        {
            var len = (int)Math.Min(Api->LastError(world, p, (nuint)buffer.Length), (nuint)buffer.Length);
            return Encoding.UTF8.GetString(buffer, 0, len);
        }
    }

    /// <summary>Throws with the host's message when a call returned <see cref="Native.Invalid"/>.</summary>
    public static uint Check(nint world, uint id, string what)
    {
        if (id == Native.Invalid)
            throw new InvalidOperationException($"{what}: {LastError(world)}");
        return id;
    }

    /// <summary>The scene id text of an entity (live, or remembered after despawn), or null.</summary>
    public static string? EntityToSceneId(Entity e)
    {
        var world = World.Current.Ptr;
        var buffer = stackalloc byte[64];
        var n = (int)Api->EntityToSceneId(world, e.Bits, buffer, 64);
        return n == 0 ? null : Encoding.UTF8.GetString(buffer, Math.Min(n, 64));
    }

    /// <summary>The entity a scene id names, or <see cref="Entity.Null"/>.</summary>
    public static Entity SceneIdToEntity(string text)
    {
        var bytes = Encoding.UTF8.GetBytes(text);
        fixed (byte* p = bytes)
            return new Entity(Api->SceneIdToEntity(World.Current.Ptr, p, (nuint)bytes.Length));
    }

    public static uint ComponentId(nint world, string name)
    {
        var bytes = Encoding.UTF8.GetBytes(name);
        fixed (byte* p = bytes)
            return Check(world, Api->ComponentIdByName(world, p, (nuint)bytes.Length), $"binding component `{name}`");
    }

    public static uint ResourceId(nint world, string name)
    {
        var bytes = Encoding.UTF8.GetBytes(name);
        fixed (byte* p = bytes) return Api->ResourceIdByName(world, p, (nuint)bytes.Length);
    }

    /// <summary>Throws, with the host's description of the difference, unless the mirror matches.</summary>
    public static void CheckLayout(nint world, string name, nuint size, nuint align, FieldDescFfi[] fields, byte[][] fieldNames, byte[]?[] tooltips)
    {
        var nameBytes = Encoding.UTF8.GetBytes(name);
        var handles = new List<GCHandle>();
        try
        {
            fixed (byte* np = nameBytes)
            fixed (FieldDescFfi* fp = fields)
            {
                for (var i = 0; i < fieldNames.Length; i++)
                {
                    var h = GCHandle.Alloc(fieldNames[i], GCHandleType.Pinned);
                    handles.Add(h);
                    fp[i].Name = (byte*)h.AddrOfPinnedObject();
                    fp[i].NameLen = (nuint)fieldNames[i].Length;
                }
                if (Api->CheckLayout(world, np, (nuint)nameBytes.Length, size, align, fp, (nuint)fields.Length) == Native.Invalid)
                    throw new InvalidOperationException(LastError(world));
            }
        }
        finally
        {
            foreach (var h in handles) if (h.IsAllocated) h.Free();
        }
    }

    public static uint RegisterComponent(nint world, string name, nuint size, nuint align, uint kind, FieldDescFfi[] fields, byte[][] fieldNames, byte[]?[] tooltips, uint[]? requires = null)
    {
        var nameBytes = Encoding.UTF8.GetBytes(name);
        fixed (byte* np = nameBytes)
        fixed (FieldDescFfi* fp = fields)
        fixed (uint* rp = requires)
        {
            // Pin each field name for the duration of the call.
            var handles = new List<GCHandle>();
            try
            {
                for (var i = 0; i < fieldNames.Length; i++)
                {
                    var h = GCHandle.Alloc(fieldNames[i], GCHandleType.Pinned);
                    handles.Add(h);
                    fp[i].Name = (byte*)h.AddrOfPinnedObject();
                    fp[i].NameLen = (nuint)fieldNames[i].Length;
                    if (tooltips[i] is { } tip)
                    {
                        var th = GCHandle.Alloc(tip, GCHandleType.Pinned);
                        handles.Add(th);
                        fp[i].Tooltip = (byte*)th.AddrOfPinnedObject();
                        fp[i].TooltipLen = (nuint)tip.Length;
                    }
                }
                var desc = new ComponentDescFfi
                {
                    Name = np, NameLen = (nuint)nameBytes.Length,
                    Size = size, Align = align, Kind = kind,
                    FieldCount = (uint)fields.Length, Fields = fp,
                    Requires = new IdList { Ptr = rp, Len = (nuint)(requires?.Length ?? 0) },
                };
                return Check(world, Api->RegisterComponent(world, &desc), $"registering `{name}`");
            }
            finally
            {
                foreach (var h in handles) if (h.IsAllocated) h.Free();
            }
        }
    }
}
