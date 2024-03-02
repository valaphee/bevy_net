use std::net::SocketAddr;

use bevy::ecs::{component::Component, system::Resource};
use thiserror::Error;
use tokio::sync::mpsc;

pub mod client;
pub mod server;
pub mod codec;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error")]
    Io(#[from] std::io::Error),

    #[error("VarInt wider than {0}-bit")]
    VarIntTooWide(u8),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Component)]
pub struct Connection {
    address: SocketAddr,

    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    tx: mpsc::UnboundedSender<Vec<u8>>,
}

#[derive(Resource)]
struct NewConnectionRx(mpsc::UnboundedReceiver<Connection>);
