use std::net::SocketAddr;

use bevy::{app::{App, Plugin, PostStartup, PostUpdate, PreUpdate}, ecs::{component::Component, entity::Entity, system::{Commands, ResMut, Resource}}, log::info, utils::HashMap};
use futures::{SinkExt, StreamExt};
use tokio::{net::{TcpListener, TcpStream}, sync::mpsc};
use tokio_util::{bytes::BytesMut, codec::{BytesCodec, Framed}};

use crate::replication::{recv_updates, send_updates, spawn_new_connections, Connection, NewConnectionRx, Replication};

pub struct ServerPlugin {
    pub address: SocketAddr
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
                        let mut framed_stream = Framed::new(stream, BytesCodec::default());

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
                                            let _ = rx_message_tx.send(message.to_vec());
                                        } else {
                                            break;
                                        }
                                    }
                                    message = tx_message_rx.recv() => {
                                        if let Some(message) = message {
                                            if framed_stream.send(BytesMut::from(message.as_slice())).await.is_err() {
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

        app
            .add_systems(PostStartup, listen)
            .add_systems(PreUpdate, (spawn_new_connections, recv_updates))
            .add_systems(PostUpdate, send_updates);
    }
}

pub struct ClientPlugin {
    pub address: SocketAddr
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
                let mut framed_stream = Framed::new(stream, BytesCodec::default());

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
                                    let _ = rx_message_tx.send(message.to_vec());
                                } else {
                                    info!("A");
                                    break;
                                }
                            }
                            message = tx_message_rx.recv() => {
                                if let Some(message) = message {
                                    if framed_stream.send(BytesMut::from(message.as_slice())).await.is_err() {
                                        info!("C");
                                        break;
                                    }
                                } else {
                                    info!("B");
                                    break;
                                }
                            }
                        }
                    }
                    tx_message_rx.close();
                });
            });
        };

        app
            .add_systems(PostStartup, connect)
            .add_systems(PreUpdate, (spawn_new_connections, recv_updates))
            .add_systems(PostUpdate, send_updates);
    }
}
