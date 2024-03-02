use std::net::SocketAddr;

use bevy::ecs::{component::Component, system::Resource};
use thiserror::Error;
use tokio::sync::mpsc;

pub mod client;
pub mod server;

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
    pub address: SocketAddr,

    pub rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pub tx: mpsc::UnboundedSender<Vec<u8>>,
}

#[derive(Resource)]
pub struct NewConnectionRx(pub mpsc::UnboundedReceiver<Connection>);
