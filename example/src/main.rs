use std::{
    f32::consts::FRAC_PI_2,
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};

use bevy::{log::LogPlugin, prelude::*};

use bevy_net::{
    replication::{AppExt, ReplicationPlugin},
    transport::{ClientPlugin, ServerPlugin},
};
use freecam::{Freecam, FreecamPlugin};
use serde::{Deserialize, Serialize};

mod freecam;

#[tokio::main]
async fn main() {
    let mut app = App::new();

    #[cfg(feature = "client")]
    app.add_plugins((
        DefaultPlugins,
        FreecamPlugin,
        ReplicationPlugin,
        ClientPlugin,
    ))
    .add_systems(Startup, setup)
    .add_systems(Update, (spawn_ball_client, shoot_ball, sync_ball))
    .recv_component::<Ball>()
    .recv_component::<BallPosition>()
    .send_event::<ShootBall>()
    .insert_resource(Hello { test: 5 })
    .recv_resource::<Hello>();

    #[cfg(feature = "server")]
    app.add_plugins((
        MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(
            Duration::from_secs_f64(1.0 / 20.0),
        )),
        LogPlugin::default(),
        ReplicationPlugin,
        ServerPlugin,
    ))
    .add_systems(Update, spawn_ball_server)
    .send_component::<Ball>()
    .send_component::<BallPosition>()
    .recv_event::<ShootBall>()
    .insert_resource(Hello { test: 5 })
    .send_resource::<Hello>();

    app.add_systems(Update, update_ball).run();
}

#[derive(Component, Default, Serialize, Deserialize)]
struct Ball {
    color: Color,
}

#[derive(Resource, Default, Serialize, Deserialize)]
struct Hello {
    test: u32,
}

#[derive(Component, Default, Serialize, Deserialize)]
struct BallPosition {
    position: Vec3,
}

#[derive(Event, Default, Serialize, Deserialize)]
struct ShootBall {
    color: Color,
}

fn update_ball(time: Res<Time>, mut balls: Query<&mut BallPosition>) {
    for mut ball in balls.iter_mut() {
        ball.position += Vec3::X * time.delta_seconds();
    }
}

#[cfg(feature = "server")]
fn spawn_ball_server(mut commands: Commands, time: Res<Time>,
    mut hello: ResMut<Hello>, mut shoot_ball_events: EventReader<bevy_net::replication::Received<ShootBall>>) {
        hello.test = time.elapsed_seconds() as u32;
   
    for event in shoot_ball_events.read() {
        println!("{:?}", event.source);
        commands.spawn((
            Ball { color: event.event.color },
            BallPosition {
                position: Vec3::ZERO,
            },
        ));
    }
}

#[cfg(feature = "client")]
fn setup(mut commands: Commands, client: Res<bevy_net::transport::Client>) {
    commands.spawn(PointLightBundle {
        point_light: PointLight {
            shadows_enabled: true,
            ..default()
        },
        transform: Transform::from_xyz(4.0, 8.0, 4.0),
        ..default()
    });
    commands.spawn((
        Camera3dBundle {
            transform: Transform::from_xyz(-2.5, 4.5, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
            ..default()
        },
        Freecam,
    ));
    client.connect();
}

#[cfg(feature = "client")]
fn shoot_ball(
    mut shoot_ball_events: EventWriter<ShootBall>,
    time: Res<Time>,
    mut hello: ResMut<Hello>,
    mut timer: Local<Timer>,
) {
    use rand::{thread_rng, Rng};
    //println!("{}", hello.test);
    if timer.finished() {
        shoot_ball_events.send(ShootBall {
            color: Color::hsv(thread_rng().gen_range(0.0..360.0), 1.0, 1.0),
        });
        timer.set_duration(Duration::from_secs(1));
        timer.reset();
    }
    timer.tick(time.delta());

   
}

#[cfg(feature = "client")]
fn spawn_ball_client(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    balls: Query<(Entity, &Ball), Added<Ball>>,
) {
    for (entity, ball) in balls.iter() {
        commands.entity(entity).insert(PbrBundle {
            mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
            material: materials.add(ball.color),
            ..default()
        });
    }
}

#[cfg(feature = "client")]
fn sync_ball(mut balls: Query<(&BallPosition, &mut Transform)>) {
    for (ball, mut transform) in balls.iter_mut() {
        transform.translation = ball.position;
    }
}
