// Behaviours that log every lifecycle call, so a test can compare the order
// with Unity's. The log is entities: each event spawns a JournalEntry, which
// itself exercises commands issued from inside Awake and OnDestroy.
using Delta;

namespace Lifecycle;

/// <summary>One logged event. Kind: 0 Awake, 1 Start, 2 Update, 3 OnDestroy, 4 OnAfterReload.</summary>
public struct JournalEntry : IComponent
{
    public int Seq;
    public int Frame;
    public int Kind;
    public int Who;
}

/// <summary>Tells the scripts what to do on the next frame. Cleared when done.</summary>
public struct Trigger : IComponent
{
    public int SpawnInPreUpdate;
    public int SpawnInUpdate;
    public Entity DespawnInLateUpdate;
    public int AddThenRemove;
    public int SpawnSpawner;
    public int SpawnNeedy;
}

public struct Marker : IComponent { public int Value; }
public struct Health : IComponent { public int Hp; }
public struct Mana : IComponent { public int Mp; }

static class Journal
{
    public static int Seq;
    public static int Frame;

    public static void Log(World world, int kind, Entity who)
    {
        var e = world.Spawn();
        world.Insert(e, new JournalEntry { Seq = Seq++, Frame = Frame, Kind = kind, Who = (int)who.Index });
    }
}

public sealed class Probe : Behaviour
{
    protected override void Awake() => Journal.Log(World, 0, Entity);
    protected override void Start() => Journal.Log(World, 1, Entity);
    public override void Update() => Journal.Log(World, 2, Entity);
    protected override void OnDestroy() => Journal.Log(World, 3, Entity);
    protected override void OnAfterReload() => Journal.Log(World, 4, Entity);
}

/// <summary>Spawns another entity from Awake: it must exist by the end of the same sync point.</summary>
public sealed class Spawner : Behaviour
{
    protected override void Awake()
    {
        var e = World.Spawn();
        World.Insert(e, new Marker { Value = 7 });
    }
}

[Requires(typeof(Health), typeof(Mana))]
public sealed class Needy : Behaviour
{
    protected override void Awake() => Journal.Log(World, 0, Entity);
}

/// <summary>Counts frames for the journal.</summary>
[System(Stage.PreUpdate, Order = -100)]
public sealed class FrameCounter : ScriptSystem
{
    protected override void OnRun(World world) => Journal.Frame++;
}

/// <summary>Base for the systems that act on a Trigger.</summary>
public abstract class TriggerSystem : ScriptSystem
{
    Query query = null!;

    protected override void OnInitialize(World world) => query = world.Query().Write<Trigger>().Build();

    protected override void OnRun(World world)
    {
        foreach (var chunk in query)
        {
            var triggers = chunk.Write<Trigger>();
            for (var i = 0; i < triggers.Length; i++) Act(world, ref triggers[i]);
        }
    }

    protected abstract void Act(World world, ref Trigger trigger);
}

[System(Stage.PreUpdate)]
public sealed class SpawnInPreUpdate : TriggerSystem
{
    protected override void Act(World world, ref Trigger t)
    {
        if (t.SpawnInPreUpdate != 0)
        {
            t.SpawnInPreUpdate = 0;
            world.Add<Probe>(world.Spawn());
        }
        if (t.SpawnSpawner != 0)
        {
            t.SpawnSpawner = 0;
            world.Add<Spawner>(world.Spawn());
        }
        if (t.SpawnNeedy != 0)
        {
            t.SpawnNeedy = 0;
            world.Add<Needy>(world.Spawn());
        }
    }
}

[System(Stage.Update, Order = 100)]
public sealed class SpawnInUpdate : TriggerSystem
{
    protected override void Act(World world, ref Trigger t)
    {
        if (t.SpawnInUpdate != 0)
        {
            t.SpawnInUpdate = 0;
            world.Add<Probe>(world.Spawn());
        }
        if (t.AddThenRemove != 0)
        {
            t.AddThenRemove = 0;
            var e = world.Spawn();
            world.Add<Probe>(e);
            world.RemoveBehaviour<Probe>(e);
        }
    }
}

[System(Stage.LateUpdate)]
public sealed class DespawnInLateUpdate : TriggerSystem
{
    protected override void Act(World world, ref Trigger t)
    {
        if (!t.DespawnInLateUpdate.IsNull)
        {
            world.Despawn(t.DespawnInLateUpdate);
            t.DespawnInLateUpdate = Entity.Null;
        }
    }
}
