// Scripts that read engine data: Time and Input.
using Delta;

namespace EngineData;

public struct TimeLog : IComponent
{
    public float UpdateDt;
    public float FixedDt;
    public int Updates;
    public int Fixeds;
}

public struct InputLog : IComponent
{
    public float Axis;
    public int HeldFrames;
    public int DownFrames;
    public int UpFrames;
}

[Requires(typeof(TimeLog))]
public sealed class TimeProbe : Behaviour
{
    public override void Update()
    {
        ref var log = ref Get<TimeLog>();
        log.UpdateDt = Time.DeltaTime;
        log.Updates++;
    }

    public override void FixedUpdate()
    {
        ref var log = ref Get<TimeLog>();
        log.FixedDt = Time.DeltaTime;
        log.Fixeds++;
    }
}

[Requires(typeof(InputLog))]
public sealed class InputProbe : Behaviour
{
    public override void Update()
    {
        ref var log = ref Get<InputLog>();
        log.Axis = Input.Axis("Vertical");
        if (Input.GetKey(KeyCode.Space)) log.HeldFrames++;
        if (Input.GetKeyDown(KeyCode.Space)) log.DownFrames++;
        if (Input.GetKeyUp(KeyCode.Space)) log.UpFrames++;
    }
}

/// <summary>Spawns one of each probe on the first frame.</summary>
[System(Stage.PreUpdate)]
public sealed class SpawnProbes : ScriptSystem
{
    bool done;

    protected override void OnRun(World world)
    {
        if (done) return;
        done = true;
        world.Add<TimeProbe>(world.Spawn());
        world.Add<InputProbe>(world.Spawn());
    }
}
