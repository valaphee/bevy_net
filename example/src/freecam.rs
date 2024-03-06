use bevy::prelude::*;

pub struct FreecamPlugin;

impl Plugin for FreecamPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (grab_mouse, process_input));
    }
}

#[derive(Component)]
pub struct Freecam;

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

fn process_input(
    time: Res<Time>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut mouse_motion_event_reader: EventReader<bevy::input::mouse::MouseMotion>,
    windows: Query<&Window>,
    mut transforms: Query<&mut Transform, With<Freecam>>,
) {
    use std::f32::consts::{FRAC_PI_2, PI};

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
