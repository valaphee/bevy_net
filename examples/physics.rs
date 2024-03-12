#![allow(clippy::unnecessary_cast)]

use bevy::prelude::*;
use bevy_xpbd_3d::{math::*, prelude::*};

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, PhysicsPlugins::default()))
        .insert_resource(ClearColor(Color::rgb(0.05, 0.05, 0.1)))
        .insert_resource(Msaa::Sample4)
        .add_systems(Startup, setup)
        .add_systems(Update, (movement, attract))
        .run();
}

#[derive(Component)]
struct Pawn;

#[derive(Component)]
struct PawnCamera;

#[derive(Component)]
struct Cube;

fn setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let cube_mesh = meshes.add(Cuboid::default());

    // Ground
    commands.spawn((
        PbrBundle {
            mesh: cube_mesh.clone(),
            material: materials.add(Color::DARK_GRAY),
            transform: Transform { translation: Vec3::new(0.0, -1.0, 0.0), scale: Vec3::new(100.0, 1.0, 100.0), ..Default::default() },
            ..default()
        },
        RigidBody::Static,
        Collider::cuboid(1.0, 1.0, 1.0),
    ));

    // Directional light
    commands.spawn(DirectionalLightBundle {
        directional_light: DirectionalLight {
            illuminance: 5000.0,
            shadows_enabled: true,
            ..default()
        },
        transform: Transform::default().looking_at(Vec3::new(-1.0, -2.5, -1.5), Vec3::Y),
        ..default()
    });

    // Pawn
    commands.spawn((
        PbrBundle {
            mesh: cube_mesh.clone(),
            material: materials.add(Color::RED),
            transform: Transform { translation: Vec3::new(0.0, 2.0, 0.0), scale: Vec3::splat(2.0), ..Default::default() },
            ..default()
        },
        RigidBody::Dynamic,
        Collider::cuboid(1.0, 1.0, 1.0),
        Pawn,
    ));

    // Camera
    commands.spawn((Camera3dBundle {
        transform: Transform::default().looking_at(Vec3::new(-25.0, -15.0, 0.0), Vec3::Y),
        ..default()
    }, PawnCamera));

    // Cubes
    for x in -10..10 {
        for z in -10..10 {
            let position = Vec3::new(x as f32 * 2.0, 0.0, z as f32 * 2.0);
            commands.spawn((
                PbrBundle {
                    mesh: cube_mesh.clone(),
                    material: materials.add(Color::GRAY),
                    transform: Transform { translation: position, scale: Vec3::splat(0.5), ..Default::default() },
                    ..default()
                },
                RigidBody::Dynamic,
                Collider::cuboid(1.0, 1.0, 1.0),
                Cube,
            ));
        }
    }
}

fn movement(
    time: Res<Time>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut pawn: Query<(&Transform, &mut AngularVelocity), With<Pawn>>,
    mut pawn_camera: Query<&mut Transform, (With<PawnCamera>, Without<Pawn>)>,
) {
    let Ok((transform, mut angular_velocity)) = pawn.get_single_mut() else {
        return;
    };
    let Ok(mut camera_transform) = pawn_camera.get_single_mut() else {
        return;
    };

    let up = keyboard_input.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp]);
    let down = keyboard_input.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]);
    let left = keyboard_input.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft]);
    let right = keyboard_input.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]);

    let horizontal = left as i8 - right as i8;
    let vertical = up as i8 - down as i8;
    let direction =
        Vector::new(horizontal as Scalar, 0.0, vertical as Scalar).normalize_or_zero();

    if direction != Vector::ZERO {
        let delta_time = time.delta_seconds_f64().adjust_precision();
        angular_velocity.x = direction.x * delta_time * 250.0;
        angular_velocity.z = direction.z * delta_time * 250.0;
    }

    camera_transform.translation = transform.translation + Vec3::new(25.0, 15.0, 0.0);
}

fn attract(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    pawn: Query<&Transform, With<Pawn>>,
    mut cubes: Query<(&Transform, &mut LinearVelocity), With<Cube>>,
) {
    let Ok(pawn_transform) = pawn.get_single() else {
        return;
    };

    let attract = keyboard_input.pressed(KeyCode::Space);
    if !attract {
        return;
    }

    for (transform, mut linear_velocity) in cubes.iter_mut() {
        if pawn_transform.translation.distance_squared(transform.translation) >= 2.5 * 2.5 {
            continue;
        }
        let velocity = (pawn_transform.translation - transform.translation).normalize_or_zero();
        linear_velocity.x += velocity.x;
        linear_velocity.y += velocity.y;
        linear_velocity.z += velocity.z;
    }
}
