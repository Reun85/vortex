using System;
using System.Numerics;

namespace Delta;

/// <summary>Unity's <c>Mathf</c>, for the scalar helpers scripts reach for.</summary>
public static class Mathf
{
    public const float PI = MathF.PI;
    public const float Deg2Rad = MathF.PI / 180f;
    public const float Rad2Deg = 180f / MathF.PI;
    public const float Epsilon = 1e-6f;

    public static float Abs(float v) => MathF.Abs(v);
    public static float Sign(float v) => v >= 0f ? 1f : -1f;
    public static float Min(float a, float b) => MathF.Min(a, b);
    public static float Max(float a, float b) => MathF.Max(a, b);
    public static float Sqrt(float v) => MathF.Sqrt(v);
    public static float Sin(float v) => MathF.Sin(v);
    public static float Cos(float v) => MathF.Cos(v);
    public static float Atan2(float y, float x) => MathF.Atan2(y, x);
    public static float Clamp(float v, float lo, float hi) => v < lo ? lo : v > hi ? hi : v;
    public static float Clamp01(float v) => Clamp(v, 0f, 1f);
    public static float Lerp(float a, float b, float t) => a + (b - a) * Clamp01(t);

    /// <summary>Moves <paramref name="current"/> toward <paramref name="target"/> by at most <paramref name="maxDelta"/>.</summary>
    public static float MoveTowards(float current, float target, float maxDelta) =>
        MathF.Abs(target - current) <= maxDelta ? target : current + Sign(target - current) * maxDelta;

    public static bool Approximately(float a, float b) =>
        MathF.Abs(a - b) < MathF.Max(1e-6f * MathF.Max(MathF.Abs(a), MathF.Abs(b)), Epsilon * 8f);

    /// <summary>Shortest signed difference between two angles in degrees.</summary>
    public static float DeltaAngle(float current, float target)
    {
        var d = (target - current) % 360f;
        if (d > 180f) d -= 360f;
        if (d < -180f) d += 360f;
        return d;
    }
}

/// <summary>Vector and rotation helpers with Unity's names, on <c>System.Numerics</c> types.</summary>
public static class VectorMath
{
    public static Vector3 MoveTowards(Vector3 current, Vector3 target, float maxDistanceDelta)
    {
        var to = target - current;
        var distance = to.Length();
        if (distance <= maxDistanceDelta || distance < 1e-6f) return target;
        return current + to / distance * maxDistanceDelta;
    }

    public static Vector3 ClampMagnitude(Vector3 v, float maxLength)
    {
        var sqr = v.LengthSquared();
        return sqr > maxLength * maxLength ? v / MathF.Sqrt(sqr) * maxLength : v;
    }

    public static Vector3 ProjectOnPlane(Vector3 v, Vector3 planeNormal)
    {
        var sqr = planeNormal.LengthSquared();
        return sqr < 1e-12f ? v : v - planeNormal * (Vector3.Dot(v, planeNormal) / sqr);
    }

    /// <summary>Angle between two rotations in radians.</summary>
    public static float Angle(Quaternion a, Quaternion b) =>
        2f * MathF.Acos(MathF.Min(MathF.Abs(Quaternion.Dot(a, b)), 1f));

    public static Quaternion RotateTowards(Quaternion from, Quaternion to, float maxRadiansDelta)
    {
        var angle = Angle(from, to);
        if (angle < 1e-6f) return to;
        return Quaternion.Slerp(from, to, MathF.Min(1f, maxRadiansDelta / angle));
    }

    /// <summary>A rotation whose forward points along <paramref name="forward"/> (<c>-Z</c> in System.Numerics' right-handed space).</summary>
    public static Quaternion LookRotation(Vector3 forward, Vector3 up)
    {
        if (forward.LengthSquared() < 1e-12f) return Quaternion.Identity;
        var world = Matrix4x4.CreateWorld(Vector3.Zero, Vector3.Normalize(forward), up);
        return Quaternion.Normalize(Quaternion.CreateFromRotationMatrix(world));
    }
}
