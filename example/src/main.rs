//! A simple 3D scene with light shining over a cube sitting on a plane.

use std::{any::{Any, TypeId}, f32::consts::{FRAC_PI_2, PI}, time::Duration};

use bevy::{prelude::*, reflect::{serde::{ReflectSerializer, TypedReflectSerializer, UntypedReflectDeserializer}, FromType, TypeData, TypeRegistry}, scene::{ron::Deserializer, serde::SceneMapSerializer, serialize_ron}, utils::HashMap};
use bevy_replicate::{Connection, NewConnectionRx};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use serde::de::DeserializeSeed;

#[tokio::main]
async fn main() {
    let mut app = App::new();

    app.add_event::<ShootBall>()
        .register_type::<ShootBall>()
        .register_type::<Ball>();

    #[cfg(feature = "client")]
    app
        .add_plugins((DefaultPlugins, bevy_replicate::client::ClientPlugin::default()))
        .add_systems(Startup, setup)
        .add_systems(Update, (grab_mouse, process_input, shoot_ball_client, spawn_ball_client))
        .add_systems(PreUpdate, (network_recv, spawn_conn))
        .add_systems(PostUpdate, shoot_ball_send);

    #[cfg(feature = "server")]
    app
        .add_plugins(
            (MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
                1.0 / 20.0,
            ))), bevy_replicate::server::ServerPlugin::default()),
        )
        .add_plugins(bevy::log::LogPlugin::default())
        .add_systems(Update, shoot_ball_server)
        .add_systems(PreUpdate, (network_recv, spawn_conn))
        .add_systems(PostUpdate, ball_send);

    app
        .add_systems(Update, simulate_ball)
        .run();
}

fn spawn_conn(mut commands: Commands, mut new_connection_rx: ResMut<NewConnectionRx>) {
    while let Ok(connection) = new_connection_rx.0.try_recv() {
        info!("Player connected");

        commands.spawn(connection);
    }
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
        material: materials.add(Color::WHITE),
        transform: Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
        ..default()
    });
    // cube
    commands.spawn(PbrBundle {
        mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        material: materials.add(Color::srgb_u8(124, 144, 255)),
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

#[derive(Event, Reflect, Default)]
#[reflect(Event)]
struct ShootBall;

#[derive(Default)]
struct EntityMapper {
    remote_to_local: HashMap<u64, Entity>
}

fn network_recv(
    mut world: &mut World,
    mut mapper: Local<EntityMapper>,
) {
    let mut connections = world.query::<&mut Connection>();

    let unsafe_world_cell = world.as_unsafe_world_cell();
    let type_registry = unsafe { unsafe_world_cell.get_resource_mut::<AppTypeRegistry>() }.unwrap();
    let type_reg = type_registry.read(); 
    for mut connection in connections.iter_mut(unsafe { unsafe_world_cell.world_mut() }) {
        while let Ok(value) = connection.rx.try_recv() {
            let mut valuere = &value[..];
            let typ = valuere.read_u8().unwrap();
            println!("Raw {:?}", valuere);
            let pl = valuere.read_u64::<LittleEndian>().unwrap();
            println!("Raw {:?}", valuere);
            let ron = std::str::from_utf8(&valuere).unwrap();
            println!("Recv {} {} {}", typ, pl, ron);
            let deserializer =  UntypedReflectDeserializer::new(&type_reg);
            let mut deser = Deserializer::from_str(&ron).unwrap();
            let ev = deserializer.deserialize(&mut deser).unwrap();
            if typ == 0x01 {
                let mut entitymutator = if let Some(entity) = mapper.remote_to_local.get(&pl) {
                    unsafe { unsafe_world_cell.world_mut() }.entity_mut(*entity)
                } else {
                    let usa = unsafe { unsafe_world_cell.world_mut() }.spawn(());
                    mapper.remote_to_local.insert(pl, usa.id());
                    usa
                };
                type_reg.get(ev.get_represented_type_info().unwrap().type_id()).unwrap().data::<ReflectComponent>().unwrap().apply_or_insert(&mut entitymutator, ev.as_ref(), &type_reg);
            } else {
                type_reg.get(ev.get_represented_type_info().unwrap().type_id()).unwrap().data::<ReflectEvent>().unwrap().send(unsafe { unsafe_world_cell.world_mut() }, ev.as_ref(), &type_reg);
            }
        }
    }
}

fn shoot_ball_send(
    type_registry: Res<AppTypeRegistry>,
    mut shoot_ball_events: EventReader<ShootBall>,

    connections: Query<&Connection>,
) {
    for connection in connections.iter() {
        for event in shoot_ball_events.read() {
            let ron = serialize_ron(ReflectSerializer::new(event, &type_registry.read())).unwrap();
            let mut rb = vec![0x00u8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
            rb.extend_from_slice(ron.as_bytes());
            connection.tx.send(rb).unwrap();
        }
    }
}

fn ball_send(
    type_registry: Res<AppTypeRegistry>,
    balls: Query<(Entity, &Ball), Changed<Ball>>,

    connections: Query<&Connection>,
) {
    for connection in connections.iter() {
        for (ent, ball) in balls.iter() {
            let ron = serialize_ron(ReflectSerializer::new(ball, &type_registry.read())).unwrap();
            let mut rb = vec![0x01u8];
            rb.write_u64::<LittleEndian>(ent.to_bits()).unwrap();
            rb.extend_from_slice(ron.as_bytes());
            connection.tx.send(rb).unwrap();
        }
    }
}

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Ball;

#[cfg(feature = "client")]
fn shoot_ball_client(
    mouse_input: Res<ButtonInput<MouseButton>>,

    mut shoot_ball_events: EventWriter<ShootBall>,
) {
    if mouse_input.just_pressed(MouseButton::Left) {
        shoot_ball_events.send(ShootBall);
    }
}

#[cfg(feature = "server")]
fn shoot_ball_server(
    mut commands: Commands,

    mut shoot_ball_events: EventReader<ShootBall>,
) {
    for _ in shoot_ball_events.read() {
        println!("spawn ball");
        commands.spawn(Ball);
    }
}

#[cfg(feature = "client")]
fn spawn_ball_client(
    mut commands: Commands,
    
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,

    balls: Query<Entity, Added<Ball>>,
) {
    for ball in balls.iter() {
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
