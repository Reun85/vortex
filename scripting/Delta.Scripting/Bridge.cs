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
    static int LoadAssembly(nint world, byte* path, nuint len)
    {
        try
        {
            return Scripts.Load(world, Encoding.UTF8.GetString(path, (int)len));
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
}
