use std::net::SocketAddr;

use bevy::{app::{App, Plugin, PostStartup, PostUpdate, PreUpdate}, ecs::{component::Component, entity::Entity, system::{Commands, ResMut, Resource}}, log::info, utils::HashMap};
use futures::{SinkExt, StreamExt};
use tokio::{net::{TcpListener, TcpStream}, sync::mpsc};
use tokio_util::{bytes::BytesMut, codec::{BytesCodec, Framed}};

use crate::registry::{recv_update, send_update, Registry};

pub struct ServerPlugin {
    pub address: SocketAddr
}

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        let address = self.address;

        app.init_resource::<Registry>();

        let listen = move |mut commands: Commands| {
            let (new_connection_tx, new_connection_rx) = mpsc::unbounded_channel();
            commands.insert_resource(Transport { new_connection_rx });

            tokio::spawn(async move {
                let listener = TcpListener::bind(address).await.unwrap();
                info!("Listening on {}", address);

                loop {
                    if let Ok((stream, _address)) = listener.accept().await {
                        info!("D");

                        let mut framed_stream = Framed::new(stream, BytesCodec::default());

                        let (rx_message_tx, rx_message_rx) = mpsc::unbounded_channel();
                        let (tx_message_tx, mut tx_message_rx) = mpsc::unbounded_channel();
                        let _ = new_connection_tx.send(Connection {
                            entity_map: Default::default(),
                            added_entities: Default::default(),
                            message_rx: rx_message_rx,
                            message_tx: tx_message_tx,
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
                    }
                }
            });
        };

        app
            .add_systems(PostStartup, listen)
            .add_systems(PreUpdate, (spawn, recv_update))
            .add_systems(PostUpdate, send_update);
    }
}

pub struct ClientPlugin {
    pub address: SocketAddr
}

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        let address = self.address;

        app.init_resource::<Registry>();

        let connect = move |mut commands: Commands| {
            let (new_connection_tx, new_connection_rx) = mpsc::unbounded_channel();
            commands.insert_resource(Transport { new_connection_rx });

            tokio::spawn(async move {
                info!("Connecting to {}", address);
                let stream = TcpStream::connect(address).await.unwrap();
                let mut framed_stream = Framed::new(stream, BytesCodec::default());

                let (rx_message_tx, rx_message_rx) = mpsc::unbounded_channel();
                let (tx_message_tx, mut tx_message_rx) = mpsc::unbounded_channel();
                let _ = new_connection_tx.send(Connection {
                    entity_map: Default::default(),
                    added_entities: Default::default(),
                    message_rx: rx_message_rx,
                    message_tx: tx_message_tx,
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
            .add_systems(PreUpdate, (spawn, recv_update))
            .add_systems(PostUpdate, send_update);
    }
}

#[derive(Resource)]
pub struct Transport {
    new_connection_rx: mpsc::UnboundedReceiver<Connection>,
}

#[derive(Component)]
pub struct Connection {
    pub entity_map: HashMap<u64, Entity>,
    pub added_entities: Vec<(u64, Entity)>,

    pub message_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pub message_tx: mpsc::UnboundedSender<Vec<u8>>,
}

fn spawn(
    mut commands: Commands,

    mut transport: ResMut<Transport>
) {
    while let Ok(connection) = transport.new_connection_rx.try_recv() {
        commands.spawn(connection);
    }
}
