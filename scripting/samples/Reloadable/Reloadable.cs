// One source, three "versions" of a game's scripts, selected with
// -p:Variant=V1|V2|V3. The hot-reload test builds all three and reloads
// from one to the next while the world keeps running.
//
//   V1  the baseline.
//   V2  Vec gains Z; Stats changes (Speed float -> double, Old removed,
//       Armor added); Counter swaps a field; Goner is removed; adds a system
//       that leaks (never undoes a static event subscription).
//   V3  Goner returns.
using System;
using System.Collections.Generic;
using Delta;

namespace Reloadable;

#if V1
public struct Vec { public float X; public float Y; }
#else
public struct Vec { public float X; public float Y; public float Z; }
#endif

public struct Body : IComponent { public Vec Pos; public int Mass; }

#if V1
public struct Stats : IComponent { public int Hp; public float Speed; public int Old; }
#else
public struct Stats : IComponent { public int Hp; public double Speed; public int Armor; }
#endif

public struct Report : IComponent { public int Version; public int StaticRuns; public int GonerSecret; }

public struct Tally : IComponent { public int Count; public int History; public int Extra; }

/// <summary>Reports which version is running and how often it ran, from a static counter.</summary>
[System(Stage.Update)]
public sealed class Probe : ScriptSystem
{
#if V1
    const int Version = 1;
#elif V2
    const int Version = 2;
#else
    const int Version = 3;
#endif

    // Statics belong to the loaded assembly: a reload starts them over.
    static int runs;

    Query query = null!;

    protected override void OnInitialize(World world) =>
        query = world.Query().Write<Report>().Build();

    protected override void OnRun(World world)
    {
        runs++;
        foreach (var chunk in query)
        {
            var reports = chunk.Write<Report>();
            for (var i = 0; i < reports.Length; i++)
            {
                reports[i].Version = Version;
                reports[i].StaticRuns = runs;
            }
        }
    }
}

public sealed class Counter : Behaviour
{
    public int Count;
    public List<int> History = new();
#if V1
    public string Label = "v1";
#else
    public int Extra = 5;
#endif

    public override void Update()
    {
        Count++;
        History.Add(Count);
        ref var tally = ref Get<Tally>();
        tally.Count = Count;
        tally.History = History.Count;
#if V1
        tally.Extra = -1;
#else
        tally.Extra = Extra;
#endif
    }
}

#if V1 || V3
public sealed class Goner : Behaviour
{
    public int Secret = 42;

    public override void Update()
    {
        Secret++;
        Get<Report>().GonerSecret = Secret;
    }
}
#endif

#if V1
[System(Stage.PreUpdate)]
public sealed class SpawnThings : ScriptSystem
{
    bool done;

    protected override void OnRun(World world)
    {
        if (done) return;
        done = true;
        for (var i = 0; i < 3; i++)
        {
            var e = world.Spawn();
            world.Insert(e, new Tally());
            world.Add<Counter>(e);
        }
        var goner = world.Spawn();
        world.Insert(goner, new Report());
        world.Add<Goner>(goner);
    }
}

/// <summary>Subscribes to a host-wide event and, correctly, undoes it when unloaded.</summary>
public sealed class Subscriber : ScriptSystem
{
    EventHandler? handler;

    protected override void OnInitialize(World world)
    {
        handler = (_, _) => { };
        AppDomain.CurrentDomain.ProcessExit += handler;
    }

    protected override void OnRun(World world) { }

    protected override void OnUnload()
    {
        if (handler is not null) AppDomain.CurrentDomain.ProcessExit -= handler;
    }
}
#endif

#if V2
/// <summary>The bug the reload must detect: a subscription that is never undone keeps the old code alive.</summary>
public sealed class Leaky : ScriptSystem
{
    protected override void OnInitialize(World world) =>
        AppDomain.CurrentDomain.ProcessExit += OnExit;

    static void OnExit(object? sender, EventArgs e) { }

    protected override void OnRun(World world) { }
}
#endif
