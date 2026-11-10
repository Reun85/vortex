using System;
using System.Runtime.InteropServices;
using System.Text.Json.Serialization;

namespace Delta;

/// <summary>
/// A system written in C#. The chunk tier: build a <see cref="Query"/> in
/// <see cref="OnInitialize"/> and loop over spans in <see cref="OnRun"/>.
/// Decorate with <see cref="SystemAttribute"/> to choose the stage.
/// </summary>
public abstract class ScriptSystem
{
    bool initialized;

    /// <summary>Called once, before the first run, with all components registered.</summary>
    protected virtual void OnInitialize(World world) { }

    protected abstract void OnRun(World world);

    /// <summary>
    /// Called when this script assembly is about to be unloaded for a reload.
    /// Undo everything that keeps host objects pointing at script code, such
    /// as static event subscriptions, or the old code cannot be unloaded.
    /// </summary>
    protected virtual void OnUnload() { }

    internal void Unload() => OnUnload();

    internal void Run(World world)
    {
        if (!initialized)
        {
            initialized = true;
            OnInitialize(world);
        }
        OnRun(world);
    }
}

/// <summary>
/// The Unity-style per-entity tier. A subclass is a component: each entity
/// holds its own instance, and the magic methods below run for every
/// instance, class by class, in a stable order.
/// </summary>
public abstract class Behaviour
{
    [JsonIgnore] public Entity Entity { get; internal set; }
    [JsonIgnore] public World World { get; internal set; } = null!;

    // ---- lifecycle ---------------------------------------------------------
    // Awake and OnDestroy are driven by component hooks in the host, so they
    // run at the sync point where the component is added or removed, never in
    // the middle of a system's iteration. Start runs before the instance's
    // first Update/FixedUpdate/LateUpdate. A hot reload runs none of these:
    // the rebuilt instance gets OnAfterReload.

    /// <summary>The instance exists and its <c>Entity</c> and <c>World</c> are set. Runs once.</summary>
    protected virtual void Awake() { }

    /// <summary>Runs once, before this instance's first Update, FixedUpdate or LateUpdate.</summary>
    protected virtual void Start() { }

    /// <summary>The component is being removed or its entity despawned; its state is still readable.</summary>
    protected virtual void OnDestroy() { }

    /// <summary>The script was hot-reloaded and this instance was rebuilt from its saved state.</summary>
    protected virtual void OnAfterReload() { }

    /// <summary>Set once <see cref="Start"/> has run (or was skipped); survives a reload.</summary>
    [JsonIgnore] internal bool Started { get; set; }

    /// <summary>
    /// The instance was rebuilt from persisted data (a scene load, undo of a
    /// delete, leaving play mode). Runs before <see cref="Awake"/>.
    /// </summary>
    protected virtual void OnAfterDeserialize() { }

    internal void RaiseAfterDeserialize() => OnAfterDeserialize();
    internal void RaiseAwake() => Awake();
    internal void RaiseStart() => Start();
    internal void RaiseOnDestroy() => OnDestroy();
    internal void RaiseAfterReload() => OnAfterReload();

    public virtual void Update() { }
    public virtual void FixedUpdate() { }
    public virtual void LateUpdate() { }

    /// <summary>
    /// Called once, after this instance's state was saved, when its script
    /// assembly is being reloaded. See <see cref="ScriptSystem"/>'s hook.
    /// </summary>
    protected virtual void OnUnload() { }

    internal void Unload() => OnUnload();

    /// <summary>A component of this behaviour's entity. Writing through it marks it changed.</summary>
    protected ref T Get<T>() where T : unmanaged, IComponent => ref World.Get<T>(Entity);

    /// <summary>A reference to a component of this behaviour's entity.</summary>
    protected ComponentRef<T> Ref<T>() where T : unmanaged, IComponent => World.Ref<T>(Entity);

    protected void Despawn() => World.Despawn(Entity);
}

internal enum BehaviourMethod { None, Update, FixedUpdate, LateUpdate }

/// <summary>Runs one magic method for every instance of one Behaviour class.</summary>
internal sealed class BehaviourSystem(uint component, BehaviourMethod method, bool start) : ScriptSystem
{
    Query? query;
    Query? starting;

    protected override void OnInitialize(World world)
    {
        if (method != BehaviourMethod.None) query = world.Query().ReadRaw(component).Build();
        // Rows added since this system last ran: free once everything has started.
        if (start) starting = world.Query().ReadRaw(component).AddedRaw(component).Build();
    }

    void StartNew(World world)
    {
        foreach (var chunk in starting!)
        {
            var handles = chunk.RawHandles(component);
            var entities = chunk.Entities;
            for (var i = 0; i < handles.Length; i++)
            {
                var behaviour = (Behaviour)GCHandle.FromIntPtr((nint)handles[i]).Target!;
                if (behaviour.Started) continue;
                behaviour.Started = true;
                behaviour.Entity = entities[i];
                behaviour.World = world;
                try { behaviour.RaiseStart(); }
                catch (Exception e) { Host.Log(3, $"{behaviour.GetType().FullName}.Start on {entities[i]} threw: {e}"); }
            }
        }
    }

    protected override void OnRun(World world)
    {
        if (starting is not null) StartNew(world);
        if (query is null) return;
        foreach (var chunk in query)
        {
            var handles = chunk.RawHandles(component);
            var entities = chunk.Entities;
            for (var i = 0; i < handles.Length; i++)
            {
                var behaviour = (Behaviour)GCHandle.FromIntPtr((nint)handles[i]).Target!;
                behaviour.Entity = entities[i];
                behaviour.World = world;
                try
                {
                    switch (method)
                    {
                        case BehaviourMethod.Update: behaviour.Update(); break;
                        case BehaviourMethod.FixedUpdate: behaviour.FixedUpdate(); break;
                        default: behaviour.LateUpdate(); break;
                    }
                }
                catch (Exception e)
                {
                    Host.Log(3, $"{behaviour.GetType().FullName}.{method} on {entities[i]} threw: {e}");
                }
            }
        }
    }
}
