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

    protected void Despawn() => World.Despawn(Entity);
}

internal enum BehaviourMethod { Update, FixedUpdate, LateUpdate }

/// <summary>Runs one magic method for every instance of one Behaviour class.</summary>
internal sealed class BehaviourSystem(uint component, BehaviourMethod method) : ScriptSystem
{
    Query? query;

    protected override void OnInitialize(World world) =>
        query = world.Query().ReadRaw(component).Build();

    protected override void OnRun(World world)
    {
        foreach (var chunk in query!)
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
