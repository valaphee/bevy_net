//! A simple 3D scene with light shining over a cube sitting on a plane.

use std::{f32::consts::{FRAC_PI_2, PI}, time::Duration};

use bevy::prelude::*;

#[tokio::main]
async fn main() {
    let mut app = App::new();

    #[cfg(feature = "client")]
    app
        .add_plugins((DefaultPlugins, bevy_replicate::client::ClientPlugin))
        .add_systems(Startup, setup)
        .add_systems(Update, (grab_mouse, process_input, shoot_ball));

    #[cfg(feature = "server")]
    app
        .add_plugins(
            (MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
                1.0 / 20.0,
            ))), bevy_replicate::server::ServerPlugin),
        )
        .add_plugins(bevy::log::LogPlugin::default());

    app
        .add_systems(Update, simulate_ball)
        .run();
}

/// set up a simple 3D scene
#[cfg(feature = "client")]
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // circular base
    commands.spawn(PbrBundle {
        mesh: meshes.add(Circle::new(4.0)),
        material: materials.add(LegacyColor::WHITE),
        transform: Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
        ..default()
    });
    // cube
    commands.spawn(PbrBundle {
        mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        material: materials.add(LegacyColor::rgb_u8(124, 144, 255)),
        transform: Transform::from_xyz(0.0, 0.5, 0.0),
        ..default()
    });
    // light
    commands.spawn(PointLightBundle {
        point_light: PointLight {
            shadows_enabled: true,
            ..default()
        },
        transform: Transform::from_xyz(4.0, 8.0, 4.0),
        ..default()
    });
    // camera
    commands.spawn((Camera3dBundle {
        transform: Transform::from_xyz(-2.5, 4.5, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
        ..default()
    }, Freecam));
}

#[derive(Component)]
struct Ball;

#[cfg(feature = "client")]
fn shoot_ball(
    mut commands: Commands,

    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mouse_input: Res<ButtonInput<MouseButton>>,

    freecams: Query<&Transform, With<Freecam>>,
) {
    let freecam = freecams.single();
    if mouse_input.just_pressed(MouseButton::Left) {
        commands.spawn((PbrBundle {
            mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
            material: materials.add(LegacyColor::rgb_u8(124, 144, 255)),
            transform: freecam.clone(),
            ..default()
        }, Ball));
    }
}

fn simulate_ball(
    time: Res<Time>,
    mut balls: Query<&mut Transform, With<Ball>>,
) {
    for mut ball in balls.iter_mut() {
        ball.translation += Vec3::X * time.elapsed_seconds() * 0.0125;
    }
}

#[derive(Component)]
struct Freecam;

#[cfg(feature = "client")]
fn grab_mouse(
    mouse_input: Res<ButtonInput<MouseButton>>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut windows: Query<&mut Window>,
) {
    let mut window = windows.single_mut();
    if mouse_input.just_pressed(bevy::input::mouse::MouseButton::Left) {
        window.cursor.visible = false;
        window.cursor.grab_mode = bevy::window::CursorGrabMode::Locked;
    }
    if keyboard_input.just_pressed(KeyCode::Escape) {
        window.cursor.visible = true;
        window.cursor.grab_mode = bevy::window::CursorGrabMode::None;
    }
}

#[cfg(feature = "client")]
fn process_input(
    time: Res<Time>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut mouse_motion_event_reader: EventReader<bevy::input::mouse::MouseMotion>,

    windows: Query<&Window>,
    mut transforms: Query<&mut Transform, With<Freecam>>,
) {
    let mut mouse_delta: Vec2 = Vec2::ZERO;
    let window = windows.single();
    if !window.cursor.visible {
        for event in mouse_motion_event_reader.read() {
            mouse_delta += event.delta;
        }
    }

    let time_delta = time.delta_seconds();

    for mut transform in transforms.iter_mut() {
        let mut move_x = 0.0;
        let mut move_y = 0.0;
        let mut move_z = 0.0;
        if keyboard_input.pressed(KeyCode::KeyW) {
            move_x += 1.0;
        }
        if keyboard_input.pressed(KeyCode::KeyS) {
            move_x -= 1.0;
        }
        if keyboard_input.pressed(KeyCode::Space) {
            move_y += 1.0;
        }
        if keyboard_input.pressed(KeyCode::ShiftLeft) {
            move_y -= 1.0;
        }
        if keyboard_input.pressed(KeyCode::KeyA) {
            move_z += 1.0;
        }
        if keyboard_input.pressed(KeyCode::KeyD) {
            move_z -= 1.0;
        }
        if move_x != 0.0 || move_y != 0.0 || move_z != 0.0 {
            let move_vec =
                transform.rotation * Vec3::new(-move_z, 0., -move_x) + Vec3::new(0., move_y, 0.);
            transform.translation += move_vec * time_delta * 20.0;
        }

        if mouse_delta.x.abs() > 1e-5 || mouse_delta.y.abs() > 1e-5 {
            let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
            transform.rotation = Quat::from_euler(
                EulerRot::YXZ,
                (yaw + (mouse_delta.x * -0.0005)) % (PI * 2.0),
                (pitch + (mouse_delta.y * -0.0005)).clamp(-FRAC_PI_2, FRAC_PI_2),
                0.0,
            );
        }
    }
}
