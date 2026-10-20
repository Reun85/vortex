using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Runtime.Loader;
using System.Text;

namespace Delta;

/// <summary>
/// One script assembly's load context. Collectible, so a reload can unload
/// the old code; shares <c>Delta.Scripting</c> with the host so script types
/// and the engine's types are the same types.
/// </summary>
internal sealed class ScriptLoadContext(string path)
    : AssemblyLoadContext("script:" + System.IO.Path.GetFileNameWithoutExtension(path), isCollectible: true)
{
    readonly AssemblyDependencyResolver resolver = new(path);

    protected override Assembly? Load(AssemblyName name)
    {
        if (name.Name == typeof(Bridge).Assembly.GetName().Name) return typeof(Bridge).Assembly;
        var resolved = resolver.ResolveAssemblyToPath(name);
        return resolved is null ? null : LoadFromAssemblyPath(resolved);
    }
}

/// <summary>Everything one loaded script assembly registered, so it can be undone.</summary>
internal sealed class LoadedScript
{
    public uint Id;
    /// <summary>Strong while loaded; cleared on unload so only the weak reference remains.</summary>
    public ScriptLoadContext? Context;
    public WeakReference ContextRef = null!;
    public readonly List<uint> SystemIds = [];
    public readonly List<Type> ComponentTypes = [];
}

internal static unsafe class Scripts
{
    /// <summary>Script systems by the id the host assigned them.</summary>
    static readonly Dictionary<uint, ScriptSystem> Systems = new();
    static readonly Dictionary<uint, LoadedScript> Loaded = new();
    /// <summary>Behaviour classes by component id, to build instances on restore.</summary>
    static readonly Dictionary<uint, Type> BehaviourTypes = new();
    static uint nextId = 1;

    static bool IsComponentStruct(Type t) =>
        t.IsValueType && !t.IsEnum && !t.IsGenericTypeDefinition && typeof(IComponent).IsAssignableFrom(t);

    static bool IsBehaviour(Type t) =>
        t.IsClass && !t.IsAbstract && !t.IsGenericTypeDefinition && typeof(Behaviour).IsAssignableFrom(t);

    static bool IsScriptSystem(Type t) =>
        t.IsClass && !t.IsAbstract && !t.IsGenericTypeDefinition
        && typeof(ScriptSystem).IsAssignableFrom(t) && t != typeof(BehaviourSystem);

    static bool Overrides(Type t, string method) =>
        t.GetMethod(method, BindingFlags.Public | BindingFlags.Instance, Type.EmptyTypes)?.DeclaringType != typeof(Behaviour);

    static void Register(nint world, LoadedScript script, string name, Stage stage, int order, ScriptSystem system)
    {
        var bytes = Encoding.UTF8.GetBytes(name);
        fixed (byte* p = bytes)
        {
            var desc = new SystemDescFfi { Name = p, NameLen = (nuint)bytes.Length, Stage = (uint)stage, Order = order };
            var id = Host.Check(world, Host.Api->RegisterSystem(world, &desc), $"registering system `{name}`");
            Systems[id] = system;
            script.SystemIds.Add(id);
        }
    }

    /// <summary>
    /// Loads a script assembly and registers everything it declares:
    /// component structs, Behaviour classes, systems, and one dispatcher per
    /// Behaviour magic method. Returns how many systems were registered.
    /// </summary>
    public static int Load(nint world, string path, out uint scriptId)
    {
        var context = new ScriptLoadContext(path);
        var script = new LoadedScript { Id = nextId++, Context = context, ContextRef = new WeakReference(context) };
        scriptId = script.Id;
        var assembly = context.LoadFromAssemblyPath(path);
        Loaded[script.Id] = script;

        Type[] types;
        try { types = assembly.GetTypes(); }
        catch (ReflectionTypeLoadException e) { types = [.. e.Types.Where(t => t is not null)!]; }

        // Deterministic registration order, so component ids are stable run to run.
        foreach (var t in types.Where(IsComponentStruct).OrderBy(ComponentRegistry.NameOf, StringComparer.Ordinal))
        {
            ComponentRegistry.RegisterStruct(world, t);
            script.ComponentTypes.Add(t);
        }
        foreach (var t in types.Where(IsBehaviour).OrderBy(ComponentRegistry.NameOf, StringComparer.Ordinal))
        {
            var id = ComponentRegistry.RegisterBehaviour(world, t);
            BehaviourTypes[id] = t;
            script.ComponentTypes.Add(t);
        }

        var count = 0;
        foreach (var t in types.Where(IsScriptSystem).OrderBy(t => t.FullName, StringComparer.Ordinal))
        {
            var attr = t.GetCustomAttribute<SystemAttribute>();
            var system = (ScriptSystem)Activator.CreateInstance(t)!;
            Register(world, script, t.FullName ?? t.Name, attr?.Stage ?? Stage.Update, attr?.Order ?? 0, system);
            count++;
        }

        foreach (var t in types.Where(IsBehaviour).OrderBy(ComponentRegistry.NameOf, StringComparer.Ordinal))
        {
            var component = ComponentIdOf(t);
            var order = t.GetCustomAttribute<ExecutionOrderAttribute>()?.Order ?? 0;
            foreach (var (name, method, stage) in new[]
            {
                ("Update", BehaviourMethod.Update, Stage.Update),
                ("FixedUpdate", BehaviourMethod.FixedUpdate, Stage.FixedUpdate),
                ("LateUpdate", BehaviourMethod.LateUpdate, Stage.LateUpdate),
            })
            {
                if (!Overrides(t, name)) continue;
                Register(world, script, $"{t.FullName}.{name}", stage, order, new BehaviourSystem(component, method));
                count++;
            }
        }
        return count;
    }

    static uint ComponentIdOf(Type t) =>
        (uint)typeof(ComponentType<>).MakeGenericType(t)
            .GetField("Value", BindingFlags.Static | BindingFlags.NonPublic)!.GetValue(null)!;

    public static void Run(uint id, nint world) => Systems[id].Run(World.For(world));

    /// <summary>
    /// Undoes a load: lets systems clean up, drops every table entry that
    /// refers to the assembly's types, and unloads its context. Kept out of
    /// line so no local here keeps the context alive afterwards.
    /// </summary>
    [MethodImpl(MethodImplOptions.NoInlining)]
    public static void Unload(uint scriptId)
    {
        if (!Loaded.TryGetValue(scriptId, out var script) || script.Context is null) return;
        foreach (var id in script.SystemIds)
        {
            try { Systems[id].Unload(); }
            catch (Exception e) { Bridge.Warn($"OnUnload of a system threw: {e.Message}"); }
            Systems.Remove(id);
        }
        script.SystemIds.Clear();
        foreach (var t in script.ComponentTypes)
        {
            var component = ComponentIdOf(t);
            BehaviourTypes.Remove(component);
            ComponentRegistry.Forget(t);
        }
        script.ComponentTypes.Clear();
        var context = script.Context;
        script.Context = null;
        ComponentRegistry.ForgetAssembliesOf(context);
        context.Unload();
    }

    /// <summary>
    /// Collects garbage and reports whether the context is still alive. A
    /// context that survives has something still pointing at its code.
    /// </summary>
    [MethodImpl(MethodImplOptions.NoInlining)]
    public static bool IsAlive(uint scriptId)
    {
        if (!Loaded.TryGetValue(scriptId, out var script)) return false;
        for (var i = 0; i < 10 && script.ContextRef.IsAlive; i++)
        {
            GC.Collect();
            GC.WaitForPendingFinalizers();
        }
        return script.ContextRef.IsAlive;
    }

    public static Behaviour? Behaviour(nint handle) => GCHandle.FromIntPtr(handle).Target as Behaviour;

    public static bool TryBehaviourType(uint component, out Type type) => BehaviourTypes.TryGetValue(component, out type!);
}
