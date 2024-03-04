use std::net::SocketAddr;

use bevy::{
    app::{App, Plugin, PostStartup, PostUpdate, PreUpdate},
    ecs::system::Commands,
    log::info,
};
use futures::{SinkExt, StreamExt};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tokio_util::{
    bytes::{Buf, BytesMut},
    codec::{Decoder, Encoder, Framed},
};

use crate::replication::{
    recv_updates, send_updates, spawn_new_connections, Connection, NewConnectionRx, Replication,
};

pub struct ServerPlugin {
    pub address: SocketAddr,
}

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        let address = self.address;

        app.init_resource::<Replication>();

        let listen = move |mut commands: Commands| {
            let (new_connection_tx, new_connection_rx) = mpsc::unbounded_channel();
            commands.insert_resource(NewConnectionRx(new_connection_rx));

            tokio::spawn(async move {
                let listener = TcpListener::bind(address).await.unwrap();

                info!("Listening on {}", address);

                loop {
                    if let Ok((stream, _address)) = listener.accept().await {
                        stream.set_nodelay(true).unwrap();

                        let mut framed_stream = Framed::new(stream, LengthFieldCodec::default());

                        let (rx_message_tx, rx_message_rx) = mpsc::unbounded_channel();
                        let (tx_message_tx, mut tx_message_rx) = mpsc::unbounded_channel();
                        let _ = new_connection_tx.send(Connection {
                            message_rx: rx_message_rx,
                            message_tx: tx_message_tx,
                            entities: Default::default(),
                            entities_added: Default::default(),
                        });

                        tokio::spawn(async move {
                            loop {
                                tokio::select! {
                                    message = framed_stream.next() => {
                                        if let Some(Ok(message)) = message {
                                            let _ = rx_message_tx.send(message);
                                        } else {
                                            break;
                                        }
                                    }
                                    message = tx_message_rx.recv() => {
                                        if let Some(message) = message {
                                            if framed_stream.send(message).await.is_err() {
                                                break;
                                            }
                                        } else {
                                            break;
                                        }
                                    }
                                }
                            }
                            tx_message_rx.close();
                        });
                    }
                }
            });
        };

        app.add_systems(PostStartup, listen)
            .add_systems(PreUpdate, (spawn_new_connections, recv_updates))
            .add_systems(PostUpdate, send_updates);
    }
}

pub struct ClientPlugin {
    pub address: SocketAddr,
}

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        let address = self.address;

        app.init_resource::<Replication>();

        let connect = move |mut commands: Commands| {
            let (new_connection_tx, new_connection_rx) = mpsc::unbounded_channel();
            commands.insert_resource(NewConnectionRx(new_connection_rx));

            tokio::spawn(async move {
                info!("Connecting to {}", address);

                let stream = TcpStream::connect(address).await.unwrap();
                stream.set_nodelay(true).unwrap();

                let mut framed_stream = Framed::new(stream, LengthFieldCodec::default());

                let (rx_message_tx, rx_message_rx) = mpsc::unbounded_channel();
                let (tx_message_tx, mut tx_message_rx) = mpsc::unbounded_channel();
                let _ = new_connection_tx.send(Connection {
                    message_rx: rx_message_rx,
                    message_tx: tx_message_tx,
                    entities: Default::default(),
                    entities_added: Default::default(),
                });

                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            message = framed_stream.next() => {
                                if let Some(Ok(message)) = message {
                                    let _ = rx_message_tx.send(message);
                                } else {
                                    break;
                                }
                            }
                            message = tx_message_rx.recv() => {
                                if let Some(message) = message {
                                    if framed_stream.send(message).await.is_err() {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                    tx_message_rx.close();
                });
            });
        };

        app.add_systems(PostStartup, connect)
            .add_systems(PreUpdate, (spawn_new_connections, recv_updates))
            .add_systems(PostUpdate, send_updates);
    }
}

#[derive(Default)]
struct LengthFieldCodec;

const MAX: usize = 8 * 1024 * 1024;

impl Decoder for LengthFieldCodec {
    type Item = Vec<u8>;

    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            return Ok(None);
        }

        let mut length_bytes = [0u8; 4];
        length_bytes.copy_from_slice(&src[..4]);
        let length = u32::from_le_bytes(length_bytes) as usize;

        if length > MAX {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Frame of length {} is too large.", length),
            ));
        }
        if src.len() < 4 + length {
            src.reserve(4 + length - src.len());
            return Ok(None);
        }

        let data = src[4..4 + length].to_vec();
        src.advance(4 + length);
        Ok(Some(data))
    }
}

impl Encoder<Vec<u8>> for LengthFieldCodec {
    type Error = std::io::Error;

    fn encode(&mut self, item: Vec<u8>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        if item.len() > MAX {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Frame of length {} is too large.", item.len()),
            ));
        }

        let len_slice = u32::to_le_bytes(item.len() as u32);
        dst.reserve(4 + item.len());
        dst.extend_from_slice(&len_slice);
        dst.extend_from_slice(&item);
        Ok(())
    }
}
