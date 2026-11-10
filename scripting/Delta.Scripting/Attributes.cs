using System;

namespace Delta;

/// <summary>Marker for plain-data components (blittable structs).</summary>
public interface IComponent { }

/// <summary>The frame stages, in run order. Matches <c>delta::Stage</c>.</summary>
public enum Stage : uint
{
    PreUpdate = 0,
    FixedUpdate = 1,
    Update = 2,
    LateUpdate = 3,
    PostUpdate = 4,
    Extract = 5,
}

/// <summary>Overrides the default component name (the type's full name).</summary>
[AttributeUsage(AttributeTargets.Class | AttributeTargets.Struct)]
public sealed class ComponentNameAttribute(string name) : Attribute
{
    public string Name { get; } = name;
}

/// <summary>Where and when a <see cref="ScriptSystem"/> runs.</summary>
[AttributeUsage(AttributeTargets.Class)]
public sealed class SystemAttribute(Stage stage) : Attribute
{
    public Stage Stage { get; } = stage;
    /// <summary>Lower runs first; ties break by type name.</summary>
    public int Order { get; set; }
}

/// <summary>Orders a <see cref="Behaviour"/> class's magic methods against others.</summary>
[AttributeUsage(AttributeTargets.Class)]
public sealed class ExecutionOrderAttribute(int order) : Attribute
{
    public int Order { get; } = order;
}

/// <summary>
/// Components that must exist whenever this one is added. When missing they
/// are inserted zero-initialized in the same step, before any
/// <see cref="Behaviour.Awake"/> runs. Data components and tags only.
/// </summary>
[AttributeUsage(AttributeTargets.Class | AttributeTargets.Struct)]
public sealed class RequiresAttribute(params Type[] components) : Attribute
{
    public Type[] Components { get; } = components;
}

/// <summary>Saves a private field in scenes and shows it in the inspector.</summary>
[AttributeUsage(AttributeTargets.Field)]
public sealed class SerializeFieldAttribute : Attribute { }

/// <summary>Limits a numeric field in the inspector.</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
public sealed class RangeAttribute(double min, double max) : Attribute
{
    public double Min { get; } = min;
    public double Max { get; } = max;
}

/// <summary>Help text shown next to a field in the inspector.</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
public sealed class TooltipAttribute(string text) : Attribute
{
    public string Text { get; } = text;
}

/// <summary>Keeps a field out of the inspector.</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
public sealed class HideInInspectorAttribute : Attribute { }

/// <summary>Shows a field in the inspector but refuses edits.</summary>
[AttributeUsage(AttributeTargets.Field | AttributeTargets.Property)]
public sealed class ReadOnlyAttribute : Attribute { }
