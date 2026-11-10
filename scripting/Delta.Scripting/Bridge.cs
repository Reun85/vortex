using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

namespace Delta;

/// <summary>Entry points the Rust host resolves by name after loading this assembly.</summary>
public static unsafe class Bridge
{
    internal static long LiveHandleCount;
    static string lastError = "";
    static readonly System.Collections.Generic.List<string> Warnings = [];

    /// <summary>Records a non-fatal problem for the host to report.</summary>
    internal static void Warn(string message)
    {
        lock (Warnings) Warnings.Add(message);
    }

    /// <summary>Smoke test: proves the host can call managed code and get a value back.</summary>
    [UnmanagedCallersOnly]
    public static int Add(int a, int b) => a + b;

    /// <summary>
    /// Exchanges function tables: stores the host's, fills in ours. Returns 0
    /// on success.
    /// </summary>
    [UnmanagedCallersOnly]
    public static int Initialize(nint host, nint script)
    {
        try
        {
            var hostApi = (HostApi*)host;
            if (hostApi->Version != Native.ApiVersion || hostApi->Size != (uint)sizeof(HostApi))
                return 1;
            // The two mirrors of FieldDescFfi must agree on size, or every field would be misread.
            if (sizeof(FieldDescFfi) != Native.FieldDescSize)
                return 2;

            Host.Api = (HostApi*)NativeMemory.Alloc((nuint)sizeof(HostApi));
            *Host.Api = *hostApi;

            var api = (ScriptApi*)script;
            api->Version = Native.ApiVersion;
            api->Size = (uint)sizeof(ScriptApi);
            api->LoadAssembly = &LoadAssembly;
            api->RunSystem = &RunSystem;
            api->FreeHandle = &FreeHandle;
            api->LiveHandles = &LiveHandles;
            api->LastError = &LastError;
            api->UnloadAssembly = &UnloadAssembly;
            api->AssemblyAlive = &AssemblyAlive;
            api->SerializeBehaviour = &SerializeBehaviour;
            api->DeserializeBehaviour = &DeserializeBehaviour;
            api->TakeWarnings = &TakeWarnings;
            api->BehaviourEvent = &BehaviourEvent;
            api->BehaviourToJson = &BehaviourToJson;
            api->BehaviourFromJson = &BehaviourFromJson;
            api->BehaviourGetMember = &BehaviourGetMember;
            api->BehaviourSetMember = &BehaviourSetMember;
            api->BehaviourSchema = &BehaviourSchema;
            api->BehaviourDefaultJson = &BehaviourDefaultJson;
            api->BehaviourRemapEntity = &BehaviourRemapEntity;
            api->TakeResult = &TakeResult;
            Host.Script = api;
            return 0;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine(e);
            return 2;
        }
    }

    [UnmanagedCallersOnly]
    static int LoadAssembly(nint world, byte* path, nuint len, uint* scriptId)
    {
        try
        {
            var count = Scripts.Load(world, Encoding.UTF8.GetString(path, (int)len), out var id);
            *scriptId = id;
            return count;
        }
        catch (Exception e)
        {
            lastError = e.ToString();
            return -1;
        }
    }

    [UnmanagedCallersOnly]
    static void RunSystem(uint id, nint world)
    {
        // An exception must never unwind into native code.
        try { Scripts.Run(id, world); }
        catch (Exception e) { Host.Log(3, $"script system {id} threw: {e}"); }
    }

    /// <summary>Called by the host when a managed component value is dropped.</summary>
    [UnmanagedCallersOnly]
    static void FreeHandle(ulong handle)
    {
        try
        {
            GCHandle.FromIntPtr((nint)handle).Free();
            Interlocked.Decrement(ref LiveHandleCount);
        }
        catch (Exception e) { Host.Log(3, $"freeing handle {handle} failed: {e}"); }
    }

    [UnmanagedCallersOnly]
    static ulong LiveHandles() => (ulong)Interlocked.Read(ref LiveHandleCount);

    [UnmanagedCallersOnly]
    static nuint LastError(byte* buffer, nuint capacity)
    {
        var bytes = Encoding.UTF8.GetBytes(lastError);
        var n = (int)Math.Min((nuint)bytes.Length, capacity);
        bytes.AsSpan(0, n).CopyTo(new Span<byte>(buffer, n));
        return (nuint)bytes.Length;
    }

    [UnmanagedCallersOnly]
    static void UnloadAssembly(uint id)
    {
        try { Scripts.Unload(id); }
        catch (Exception e) { Host.Log(3, $"unloading script {id} failed: {e}"); }
    }

    [UnmanagedCallersOnly]
    static byte AssemblyAlive(uint id) => Scripts.IsAlive(id) ? (byte)1 : (byte)0;

    /// <summary>Saves a behaviour for a reload, then lets it tear itself down. Returns its size, or -1.</summary>
    [UnmanagedCallersOnly]
    static nuint SerializeBehaviour(ulong handle, byte* buffer, nuint capacity)
    {
        try
        {
            var behaviour = Scripts.Behaviour((nint)handle);
            if (behaviour is null) return nuint.MaxValue;
            var bytes = BehaviourSerializer.Save(behaviour, SerializeMode.Snapshot);
            if ((nuint)bytes.Length <= capacity)
            {
                bytes.AsSpan().CopyTo(new Span<byte>(buffer, bytes.Length));
                // Only the call that actually delivers the state tears down.
                behaviour.Unload();
            }
            return (nuint)bytes.Length;
        }
        catch (Exception e)
        {
            Warn($"saving a behaviour failed: {e.Message}");
            return nuint.MaxValue;
        }
    }

    /// <summary>Builds a behaviour of the component's class from saved state; returns a new handle or 0.</summary>
    [UnmanagedCallersOnly]
    static ulong DeserializeBehaviour(uint component, byte* json, nuint len)
    {
        try
        {
            if (!Scripts.TryBehaviourType(component, out var type)) return 0;
            var behaviour = BehaviourSerializer.Restore(type, new ReadOnlySpan<byte>(json, (int)len), SerializeMode.Snapshot);
            Interlocked.Increment(ref LiveHandleCount);
            return (ulong)GCHandle.ToIntPtr(GCHandle.Alloc(behaviour, GCHandleType.Normal));
        }
        catch (Exception e)
        {
            Warn($"restoring a behaviour failed: {e.Message}");
            return 0;
        }
    }

    [UnmanagedCallersOnly]
    static void BehaviourEvent(nint world, ulong handle, ulong entity, uint evt)
    {
        try { Scripts.Deliver(world, (nint)handle, new Entity(entity), evt); }
        catch (Exception e) { Host.Log(3, $"behaviour event {evt} failed: {e}"); }
    }

    // ---- behaviours as editor data ------------------------------------------
    // Each call stores its JSON result and returns the length (or
    // nuint.MaxValue with the reason in lastError); the host then fetches it
    // with TakeResult.

    static byte[] result = [];

    static nuint Store(byte[] bytes)
    {
        result = bytes;
        return (nuint)bytes.Length;
    }

    static nuint Failed(Exception e)
    {
        lastError = e.Message;
        return nuint.MaxValue;
    }

    [UnmanagedCallersOnly]
    static nuint BehaviourToJson(nint world, ulong handle, uint mode)
    {
        try
        {
            World.Current = World.For(world);
            var behaviour = Scripts.Behaviour((nint)handle) ?? throw new InvalidOperationException("no such behaviour");
            return Store(BehaviourSerializer.Save(behaviour, (SerializeMode)mode));
        }
        catch (Exception e) { return Failed(e); }
    }

    [UnmanagedCallersOnly]
    static ulong BehaviourFromJson(nint world, uint component, byte* json, nuint len)
    {
        try
        {
            World.Current = World.For(world);
            if (!Scripts.TryBehaviourType(component, out var type))
                throw new InvalidOperationException($"component {component} is not a behaviour class");
            var behaviour = BehaviourSerializer.Restore(type, new ReadOnlySpan<byte>(json, (int)len), SerializeMode.Persist);
            Interlocked.Increment(ref LiveHandleCount);
            return (ulong)GCHandle.ToIntPtr(GCHandle.Alloc(behaviour, GCHandleType.Normal));
        }
        catch (Exception e)
        {
            lastError = e.Message;
            return 0;
        }
    }

    [UnmanagedCallersOnly]
    static nuint BehaviourGetMember(nint world, ulong handle, byte* path, nuint pathLen)
    {
        try
        {
            World.Current = World.For(world);
            var behaviour = Scripts.Behaviour((nint)handle) ?? throw new InvalidOperationException("no such behaviour");
            return Store(BehaviourSerializer.GetMember(behaviour, Encoding.UTF8.GetString(path, (int)pathLen)));
        }
        catch (Exception e) { return Failed(e); }
    }

    [UnmanagedCallersOnly]
    static nuint BehaviourSetMember(nint world, ulong handle, byte* path, nuint pathLen, byte* json, nuint jsonLen)
    {
        try
        {
            World.Current = World.For(world);
            var behaviour = Scripts.Behaviour((nint)handle) ?? throw new InvalidOperationException("no such behaviour");
            return Store(BehaviourSerializer.SetMember(behaviour, Encoding.UTF8.GetString(path, (int)pathLen),
                new ReadOnlySpan<byte>(json, (int)jsonLen)));
        }
        catch (Exception e) { return Failed(e); }
    }

    [UnmanagedCallersOnly]
    static nuint BehaviourSchema(uint component)
    {
        try
        {
            if (!Scripts.TryBehaviourType(component, out var type))
                throw new InvalidOperationException($"component {component} is not a behaviour class");
            return Store(BehaviourSerializer.Schema(type));
        }
        catch (Exception e) { return Failed(e); }
    }

    [UnmanagedCallersOnly]
    static nuint BehaviourDefaultJson(nint world, uint component)
    {
        try
        {
            World.Current = World.For(world);
            if (!Scripts.TryBehaviourType(component, out var type))
                throw new InvalidOperationException($"component {component} is not a behaviour class");
            return Store(BehaviourSerializer.Default(type));
        }
        catch (Exception e) { return Failed(e); }
    }

    [UnmanagedCallersOnly]
    static uint BehaviourRemapEntity(nint world, ulong handle, ulong from, ulong to)
    {
        try
        {
            World.Current = World.For(world);
            var behaviour = Scripts.Behaviour((nint)handle);
            return behaviour is null ? 0 : (uint)BehaviourSerializer.RemapEntities(behaviour, from, to);
        }
        catch (Exception e)
        {
            lastError = e.Message;
            return 0;
        }
    }

    [UnmanagedCallersOnly]
    static nuint TakeResult(byte* buffer, nuint capacity)
    {
        var n = (int)Math.Min((nuint)result.Length, capacity);
        result.AsSpan(0, n).CopyTo(new Span<byte>(buffer, n));
        return (nuint)n;
    }

    [UnmanagedCallersOnly]
    static nuint TakeWarnings(byte* buffer, nuint capacity)
    {
        string text;
        lock (Warnings)
        {
            text = string.Join('\n', Warnings);
            Warnings.Clear();
        }
        var bytes = Encoding.UTF8.GetBytes(text);
        var n = (int)Math.Min((nuint)bytes.Length, capacity);
        bytes.AsSpan(0, n).CopyTo(new Span<byte>(buffer, n));
        return (nuint)bytes.Length;
    }
}
