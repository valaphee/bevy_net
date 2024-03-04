use std::{net::{Ipv4Addr, SocketAddrV4}, time::Duration};
use std::f32::consts::FRAC_PI_2;

use bevy::{log::LogPlugin, prelude::*};

use bevy_net::{replication::AppExt, transport::{ClientPlugin, ServerPlugin}};
use freecam::{Freecam, FreecamPlugin};
use serde::{Deserialize, Serialize};

mod freecam;

#[tokio::main]
async fn main() {
    let mut app = App::new();

    #[cfg(feature = "client")]
    app
        .add_plugins((DefaultPlugins, FreecamPlugin, ClientPlugin {
            address: SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 13371).into(),
        }))
        .add_systems(Startup, setup)
        .add_systems(Update, (setup_ball_client, shoot_ball_client))
        .recv_component::<Ball>()
        .send_event::<ShootBall>();

    #[cfg(feature = "server")]
    app
        .add_plugins((MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
            1.0 / 20.0,
        ))), LogPlugin::default(), ServerPlugin {
            address: SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 13371).into(),
        }))
        .add_systems(Update, shoot_ball_server)
        .send_component::<Ball>()
        .recv_event::<ShootBall>();

    app
        .add_systems(Update, simulate_ball)
        .run();
}

#[derive(Component, Default, Serialize, Deserialize)]
struct Ball;

#[derive(Event, Default, Serialize, Deserialize)]
struct ShootBall;

#[cfg(feature = "client")]
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn(PbrBundle {
        mesh: meshes.add(Circle::new(4.0)),
        material: materials.add(Color::WHITE),
        transform: Transform::from_rotation(Quat::from_rotation_x(-FRAC_PI_2)),
        ..default()
    });
    commands.spawn(PointLightBundle {
        point_light: PointLight {
            shadows_enabled: true,
            ..default()
        },
        transform: Transform::from_xyz(4.0, 8.0, 4.0),
        ..default()
    });
    commands.spawn((Camera3dBundle {
        transform: Transform::from_xyz(-2.5, 4.5, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
        ..default()
    }, Freecam));
}

#[cfg(feature = "client")]
fn shoot_ball_client(
    mouse_input: Res<ButtonInput<MouseButton>>,
    mut shoot_ball_events: EventWriter<ShootBall>,
) {
    if mouse_input.just_pressed(MouseButton::Left) {
        shoot_ball_events.send(ShootBall);
        println!("Shoot ball");
    }
}

#[cfg(feature = "server")]
fn shoot_ball_server(
    mut commands: Commands,
    mut shoot_ball_events: EventReader<ShootBall>,
) {
    for _ in shoot_ball_events.read() {
        commands.spawn((Ball, Transform::default()));
        println!("Spawn ball");
    }
}

#[cfg(feature = "client")]
fn setup_ball_client(
    mut commands: Commands,
    
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,

    balls: Query<Entity, Added<Ball>>,
) {
    for ball in balls.iter() {
        println!("Spawn ball");
        commands.entity(ball).insert(PbrBundle {
            mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
            material: materials.add(Color::srgb_u8(124, 144, 255)),
            ..default()
        });
    }
}

fn simulate_ball(
    time: Res<Time>,
    mut balls: Query<&mut Transform, With<Ball>>,
) {
    for mut ball in balls.iter_mut() {
        ball.translation += Vec3::X * time.elapsed_seconds() * 0.0125;
        //println!("Simulate ball");
    }
}
