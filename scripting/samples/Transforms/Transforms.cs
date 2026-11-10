// Scripts working on the engine's own Transform component.
using Delta;

namespace Transforms;

public struct Frozen : IComponent { }

/// <summary>The chunk tier on a Rust component: moves every unfrozen Transform.</summary>
[System(Stage.Update)]
public sealed class MoveAll : ScriptSystem
{
    Query query = null!;

    protected override void OnInitialize(World world) =>
        query = world.Query().Write<Transform>().Without<Frozen>().Build();

    protected override void OnRun(World world)
    {
        foreach (var chunk in query)
        {
            var transforms = chunk.Write<Transform>();
            for (var i = 0; i < transforms.Length; i++) transforms[i].Position.X += 1f;
        }
    }
}

/// <summary>The per-entity tier: Get&lt;Transform&gt;() returns a ref into the Rust column.</summary>
public sealed class Walker : Behaviour
{
    public override void Update() => Get<Transform>().Position.X += 1f;
}

[System(Stage.PreUpdate)]
public sealed class AddWalker : ScriptSystem
{
    bool done;

    protected override void OnRun(World world)
    {
        if (done) return;
        done = true;
        var e = world.Spawn();
        world.Insert(e, new Transform { Scale = System.Numerics.Vector3.One, Rotation = System.Numerics.Quaternion.Identity });
        world.Insert(e, new Frozen());
        world.Add<Walker>(e);
    }
}
