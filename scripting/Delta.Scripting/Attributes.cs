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
