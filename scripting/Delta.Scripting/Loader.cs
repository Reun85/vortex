using System;
using System.Collections.Generic;
using System.Linq;
using System.Reflection;
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

internal static unsafe class Scripts
{
    /// <summary>Script systems by the id the host assigned them.</summary>
    static readonly Dictionary<uint, ScriptSystem> Systems = new();
    static readonly List<ScriptLoadContext> Contexts = new();

    static bool IsComponentStruct(Type t) =>
        t.IsValueType && !t.IsEnum && !t.IsGenericTypeDefinition && typeof(IComponent).IsAssignableFrom(t);

    static bool IsBehaviour(Type t) =>
        t.IsClass && !t.IsAbstract && !t.IsGenericTypeDefinition && typeof(Behaviour).IsAssignableFrom(t);

    static bool IsScriptSystem(Type t) =>
        t.IsClass && !t.IsAbstract && !t.IsGenericTypeDefinition
        && typeof(ScriptSystem).IsAssignableFrom(t) && t != typeof(BehaviourSystem);

    static bool Overrides(Type t, string method) =>
        t.GetMethod(method, BindingFlags.Public | BindingFlags.Instance, Type.EmptyTypes)?.DeclaringType != typeof(Behaviour);

    static uint Register(nint world, string name, Stage stage, int order, ScriptSystem system)
    {
        var bytes = Encoding.UTF8.GetBytes(name);
        fixed (byte* p = bytes)
        {
            var desc = new SystemDescFfi { Name = p, NameLen = (nuint)bytes.Length, Stage = (uint)stage, Order = order };
            var id = Host.Check(world, Host.Api->RegisterSystem(world, &desc), $"registering system `{name}`");
            Systems[id] = system;
            return id;
        }
    }

    /// <summary>
    /// Loads a script assembly and registers everything it declares:
    /// component structs, Behaviour classes, systems, and one dispatcher per
    /// Behaviour magic method. Returns how many systems were registered.
    /// </summary>
    public static int Load(nint world, string path)
    {
        var context = new ScriptLoadContext(path);
        var assembly = context.LoadFromAssemblyPath(path);
        Contexts.Add(context);

        Type[] types;
        try { types = assembly.GetTypes(); }
        catch (ReflectionTypeLoadException e) { types = [.. e.Types.Where(t => t is not null)!]; }

        // Deterministic registration order, so component ids are stable run to run.
        foreach (var t in types.Where(IsComponentStruct).OrderBy(ComponentRegistry.NameOf, StringComparer.Ordinal))
            ComponentRegistry.RegisterStruct(world, t);
        foreach (var t in types.Where(IsBehaviour).OrderBy(ComponentRegistry.NameOf, StringComparer.Ordinal))
            ComponentRegistry.RegisterBehaviour(world, t);

        var count = 0;
        foreach (var t in types.Where(IsScriptSystem).OrderBy(t => t.FullName, StringComparer.Ordinal))
        {
            var attr = t.GetCustomAttribute<SystemAttribute>();
            var system = (ScriptSystem)Activator.CreateInstance(t)!;
            Register(world, t.FullName ?? t.Name, attr?.Stage ?? Stage.Update, attr?.Order ?? 0, system);
            count++;
        }

        foreach (var t in types.Where(IsBehaviour).OrderBy(ComponentRegistry.NameOf, StringComparer.Ordinal))
        {
            var component = (uint)typeof(ComponentType<>).MakeGenericType(t)
                .GetField("Value", BindingFlags.Static | BindingFlags.NonPublic)!.GetValue(null)!;
            var order = t.GetCustomAttribute<ExecutionOrderAttribute>()?.Order ?? 0;
            foreach (var (name, method, stage) in new[]
            {
                ("Update", BehaviourMethod.Update, Stage.Update),
                ("FixedUpdate", BehaviourMethod.FixedUpdate, Stage.FixedUpdate),
                ("LateUpdate", BehaviourMethod.LateUpdate, Stage.LateUpdate),
            })
            {
                if (!Overrides(t, name)) continue;
                Register(world, $"{t.FullName}.{name}", stage, order, new BehaviourSystem(component, method));
                count++;
            }
        }
        return count;
    }

    public static void Run(uint id, nint world) => Systems[id].Run(World.For(world));
}
