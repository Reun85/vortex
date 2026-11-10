// A mirror of vortex::Time whose first two fields are swapped: loading this
// assembly must fail, and the error must name the field.
using Delta;

namespace BadMirror;

[EngineResource("vortex::Time")]
public struct WrongTime
{
    public float FixedDelta;
    public float Delta;
    public double Elapsed;
    public ulong Frame;
    public byte InFixed;
}
