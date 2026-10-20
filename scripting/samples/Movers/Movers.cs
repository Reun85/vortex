using Delta;

namespace Movers;

public struct Position : IComponent
{
    public float X;
    public float Y;
}

public struct Velocity : IComponent
{
    public float X;
    public float Y;
}

public struct Counter : IComponent
{
    public int Value;
}

/// <summary>A tag: no fields, no storage.</summary>
public struct Frozen : IComponent { }

/// <summary>Has an entity reference and a nested struct, to exercise reflection.</summary>
public struct Link : IComponent
{
    public Entity Target;
    public Position Offset;
    public ushort Strength;
}

/// <summary>The chunk tier: direct spans over the component columns.</summary>
[System(Stage.Update)]
public sealed class MoveSystem : ScriptSystem
{
    Query query = null!;

    protected override void OnInitialize(World world) =>
        query = world.Query().Write<Position>().Read<Velocity>().Without<Frozen>().Build();

    protected override void OnRun(World world)
    {
        foreach (var chunk in query)
        {
            var positions = chunk.Write<Position>();
            var velocities = chunk.Read<Velocity>();
            for (var i = 0; i < positions.Length; i++)
            {
                positions[i].X += velocities[i].X;
                positions[i].Y += velocities[i].Y;
            }
        }
    }
}

/// <summary>Spawns three Spinner entities on the first frame, through deferred commands.</summary>
[System(Stage.PreUpdate)]
public sealed class SpawnSpinners : ScriptSystem
{
    bool done;

    protected override void OnRun(World world)
    {
        if (done) return;
        done = true;
        for (var i = 0; i < 3; i++)
        {
            var e = world.Spawn();
            world.Insert(e, new Counter());
            world.Add<Spinner>(e);
        }
    }
}

/// <summary>The per-entity tier: one instance per entity, magic methods run for each.</summary>
public sealed class Spinner : Behaviour
{
    public int Ticks;

    public override void Update()
    {
        Ticks++;
        Get<Counter>().Value += 1;
    }

    public override void LateUpdate() => Get<Counter>().Value += 100;
}
