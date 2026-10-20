using System;
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

    public static uint RegisterComponent(nint world, string name, nuint size, nuint align, uint kind, FieldDescFfi[] fields, byte[][] fieldNames)
    {
        var nameBytes = Encoding.UTF8.GetBytes(name);
        fixed (byte* np = nameBytes)
        fixed (FieldDescFfi* fp = fields)
        {
            // Pin each field name for the duration of the call.
            var handles = new GCHandle[fieldNames.Length];
            try
            {
                for (var i = 0; i < fieldNames.Length; i++)
                {
                    handles[i] = GCHandle.Alloc(fieldNames[i], GCHandleType.Pinned);
                    fp[i].Name = (byte*)handles[i].AddrOfPinnedObject();
                    fp[i].NameLen = (nuint)fieldNames[i].Length;
                }
                var desc = new ComponentDescFfi
                {
                    Name = np, NameLen = (nuint)nameBytes.Length,
                    Size = size, Align = align, Kind = kind,
                    FieldCount = (uint)fields.Length, Fields = fp,
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
