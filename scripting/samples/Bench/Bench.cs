// Phase 5 measurement: what does one component access cost from C#?
using System.Diagnostics;
using Delta;

namespace Bench;

public struct Result : IComponent
{
    public double GetNs, RefNs, ChunkNs;
}

[System(Stage.Update)]
public sealed class Measure : ScriptSystem
{
    const int N = 10_000;
    const int Rounds = 50;
    Entity[]? entities;
    ComponentRef<Transform>[]? refs;
    Query query = null!;
    Entity result;

    protected override void OnInitialize(World world) =>
        query = world.Query().Write<Transform>().Build();

    protected override void OnRun(World world)
    {
        if (entities is null)
        {
            // Spawns are deferred: build the population first, measure next frame.
            entities = new Entity[N];
            for (var i = 0; i < N; i++)
            {
                entities[i] = world.Spawn();
                world.Insert(entities[i], new Transform { Scale = System.Numerics.Vector3.One, Rotation = System.Numerics.Quaternion.Identity });
            }
            result = world.Spawn();
            world.Insert(result, new Result());
            return;
        }
        if (refs is not null) return;
        refs = new ComponentRef<Transform>[N];
        for (var i = 0; i < N; i++) refs[i] = world.Ref<Transform>(entities[i]);

        double Time(System.Action body)
        {
            body(); // warm up
            var sw = Stopwatch.StartNew();
            for (var r = 0; r < Rounds; r++) body();
            return sw.Elapsed.TotalMilliseconds * 1e6 / (Rounds * N);
        }
        var get = Time(() => { for (var i = 0; i < N; i++) world.Get<Transform>(entities[i]).Position.X += 1f; });
        var rf = Time(() => { for (var i = 0; i < N; i++) refs[i].Write.Position.X += 1f; });
        var chunk = Time(() =>
        {
            foreach (var c in query)
            {
                var t = c.Write<Transform>();
                for (var i = 0; i < t.Length; i++) t[i].Position.X += 1f;
            }
        });
        world.Insert(result, new Result { GetNs = get, RefNs = rf, ChunkNs = chunk });
    }
}
