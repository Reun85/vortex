// The Unity PlayerMovement example, ported with mechanical renames.
using System;
using System.Numerics;
using System.Runtime.CompilerServices;
using Delta;

namespace Player;

/// <summary>What happened to a PlayerMovement, for the test to read.</summary>
public struct PlayerEvents : IComponent
{
    public int Awakes;
    public int Starts;
    public int Updates;
    public int StartedBeforeFirstUpdate;
    public int CameraAlive;
    public int InstanceHash;
}

/// <summary>Spawned when a PlayerMovement is destroyed.</summary>
public struct Destroyed : IComponent { public int Count; }

[Requires(typeof(Transform), typeof(CharacterBody), typeof(PlayerEvents))]
public sealed class PlayerMovement : Behaviour
{
    [SerializeField, Range(0, 20)] float walkSpeed = 5f;   // private, inspector-editable
    [SerializeField] float sprintSpeed = 8f, rotationSpeed = 720f, acceleration = 20f;
    [SerializeField] float jumpHeight = 1.2f, gravity = -20f, groundedStickForce = -2f;
    [SerializeField] Entity cameraEntity = Entity.Null;                 // saved in scenes as a SceneId

    Vector3 horizontalVelocity;                             // private runtime state
    float verticalVelocity;

    protected override void Awake() => Get<PlayerEvents>().Awakes++;

    protected override void Start() => Get<PlayerEvents>().Starts++;

    protected override void OnDestroy()
    {
        var e = World.Spawn();
        World.Insert(e, new Destroyed { Count = 1 });
    }

    public override void Update()
    {
        float dt = Time.DeltaTime;                          // engine resource
        Vector2 input = Input.Axis("Horizontal", "Vertical");
        ref var self = ref Get<Transform>();                // Rust-native component
        ref var body = ref Get<CharacterBody>();
        ref var events = ref Get<PlayerEvents>();
        events.Updates++;
        if (events.Updates == 1) events.StartedBeforeFirstUpdate = events.Starts;
        events.InstanceHash = RuntimeHelpers.GetHashCode(this);
        events.CameraAlive = World.IsAlive(cameraEntity) ? 1 : 0;

        // Move relative to the camera, flattened onto the ground plane.
        var forward = -Vector3.UnitZ;
        var right = Vector3.UnitX;
        if (World.IsAlive(cameraEntity) && World.TryGet<Transform>(cameraEntity, out var camera))
        {
            forward = VectorMath.ProjectOnPlane(Vector3.Transform(-Vector3.UnitZ, camera.Rotation), Vector3.UnitY);
            right = VectorMath.ProjectOnPlane(Vector3.Transform(Vector3.UnitX, camera.Rotation), Vector3.UnitY);
            forward = forward.LengthSquared() > 1e-6f ? Vector3.Normalize(forward) : -Vector3.UnitZ;
            right = right.LengthSquared() > 1e-6f ? Vector3.Normalize(right) : Vector3.UnitX;
        }
        var wish = VectorMath.ClampMagnitude(forward * input.Y + right * input.X, 1f);

        float speed = Input.GetKey(KeyCode.LeftShift) ? sprintSpeed : walkSpeed;
        horizontalVelocity = VectorMath.MoveTowards(horizontalVelocity, wish * speed, acceleration * dt);

        if (wish.LengthSquared() > 1e-6f)
        {
            var target = VectorMath.LookRotation(wish, Vector3.UnitY);
            self.Rotation = VectorMath.RotateTowards(self.Rotation, target, rotationSpeed * Mathf.Deg2Rad * dt);
        }

        if (body.Grounded && verticalVelocity < 0f)
        {
            verticalVelocity = groundedStickForce;
            if (Input.GetKeyDown(KeyCode.Space))
                verticalVelocity = MathF.Sqrt(jumpHeight * -2f * gravity);
        }
        verticalVelocity += gravity * dt;

        body.Velocity = horizontalVelocity + Vector3.UnitY * verticalVelocity;
    }
}
