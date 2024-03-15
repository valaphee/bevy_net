#![allow(clippy::unnecessary_cast)]

use bevy::prelude::*;
use bevy_net::{
    replication::{AppExt, ReplicationPlugin},
    transport::wt::{Client, ClientPlugin, ServerPlugin},
};
use bevy_xpbd_3d::{math::*, prelude::*};
use clap::Parser;
use serde::{Deserialize, Serialize};

#[derive(Parser)]
enum Mode {
    Standalone,
    Client,
    ListenServer,
    DedicatedServer,
}

fn main() {
    let mut app = App::new();
    app.add_plugins((DefaultPlugins, PhysicsPlugins::default(), ReplicationPlugin))
        .insert_resource(ClearColor(Color::rgb(0.05, 0.05, 0.1)))
        .insert_resource(Msaa::Sample4)
        .add_systems(Startup, setup)
        .add_systems(Update, (movement, attract, spawn_players));

    match Mode::parse() {
        Mode::Standalone => {}
        Mode::Client => {
            app.add_plugins(ClientPlugin)
                .recv_component::<Transform>()
                .add_systems(PostStartup, connect);
        }
        Mode::ListenServer => {
            app.add_plugins(ServerPlugin).send_component::<Transform>();
        }
        Mode::DedicatedServer => {
            todo!()
        }
    }

    app.run();
}

fn setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let cube_mesh = meshes.add(Cuboid::default());

    // Ground
    commands.spawn((
        // Rendering
        PbrBundle {
            mesh: cube_mesh.clone(),
            material: materials.add(Color::DARK_GRAY),
            transform: Transform {
                translation: Vec3::new(0.0, -1.0, 0.0),
                scale: Vec3::new(100.0, 1.0, 100.0),
                ..Default::default()
            },
            ..default()
        },
        // Physics
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

    // Camera
    commands.spawn((
        // Rendering
        Camera3dBundle {
            transform: Transform::default().looking_at(Vec3::new(-25.0, -15.0, 0.0), Vec3::Y),
            ..default()
        },
        // Game Logic
        PlayerCamera,
    ));

    // Cubes
    for x in -10..10 {
        for z in -10..10 {
            let position = Vec3::new(x as f32 * 2.0, 0.0, z as f32 * 2.0);
            commands.spawn((
                // Rendering
                PbrBundle {
                    mesh: cube_mesh.clone(),
                    material: materials.add(Color::GRAY),
                    transform: Transform {
                        translation: position,
                        scale: Vec3::splat(0.5),
                        ..Default::default()
                    },
                    ..default()
                },
                // Physics
                RigidBody::Dynamic,
                Collider::cuboid(1.0, 1.0, 1.0),
                Friction::new(5.0),
                // Game Logic
                Attractable,
            ));
        }
    }
}

fn connect(client: Res<Client>) {
    client.connect("https://[::1]:4433".to_string());
}

fn spawn_players(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    new_players: Query<Entity, Added<Player>>,
) {
    let cube_mesh = meshes.add(Cuboid::default());

    for player in new_players.iter() {
        commands.entity(player).insert((
            // Rendering
            PbrBundle {
                mesh: cube_mesh.clone(),
                material: materials.add(Color::RED),
                transform: Transform {
                    translation: Vec3::new(0.0, 2.0, 0.0),
                    scale: Vec3::splat(2.0),
                    ..Default::default()
                },
                ..default()
            },
            // Physics
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            Friction::new(5.0),
            Mass(10.0),
            // Game Logic
            Player,
        ));
    }
}

fn movement(
    time: Res<Time>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut player: Query<(&Transform, &mut LinearVelocity), With<Player>>,
    mut player_camera: Query<&mut Transform, (With<PlayerCamera>, Without<Player>)>,
) {
    let Ok((transform, mut linear_velocity)) = player.get_single_mut() else {
        return;
    };
    let Ok(mut camera_transform) = player_camera.get_single_mut() else {
        return;
    };

    let up = keyboard_input.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp]);
    let down = keyboard_input.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]);
    let left = keyboard_input.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft]);
    let right = keyboard_input.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]);

    let horizontal = left as i8 - right as i8;
    let vertical = down as i8 - up as i8;
    let direction = Vector::new(horizontal as Scalar, 0.0, vertical as Scalar).normalize_or_zero();

    if direction != Vector::ZERO {
        let delta_time = time.delta_seconds_f64().adjust_precision();
        linear_velocity.x += direction.z * delta_time * 10.0;
        linear_velocity.z += direction.x * delta_time * 10.0;
    }

    camera_transform.translation = transform.translation + Vec3::new(25.0, 15.0, 0.0);
}

fn attract(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    pawn: Query<&Transform, With<Player>>,
    mut attractables: Query<(&Transform, &mut LinearVelocity), With<Attractable>>,
) {
    let Ok(pawn_transform) = pawn.get_single() else {
        return;
    };

    let attract = keyboard_input.pressed(KeyCode::Space);
    if !attract {
        return;
    }

    for (transform, mut linear_velocity) in attractables.iter_mut() {
        if pawn_transform
            .translation
            .distance_squared(transform.translation)
            >= 2.5 * 2.5
        {
            continue;
        }

        let velocity = pawn_transform.translation - transform.translation;
        linear_velocity.x += velocity.x;
        linear_velocity.y += velocity.y;
        linear_velocity.z += velocity.z;
    }
}

#[derive(Component)]
struct Player;

#[derive(Component)]
struct PlayerCamera;

#[derive(Component)]
struct Attractable;
