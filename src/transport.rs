use std::time::Duration;

use bevy::{
    app::{App, Plugin, PostStartup},
    ecs::system::{Commands, Resource},
};
use tokio::sync::mpsc::{self, UnboundedSender};
use wtransport::{Certificate, ClientConfig, Endpoint, ServerConfig};

use crate::replication::{Connection, NewConnectionRx};

pub struct ServerPlugin;

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        let listen = move |mut commands: Commands| {
            let (new_connection_tx, new_connection_rx) = mpsc::unbounded_channel();
            commands.insert_resource(NewConnectionRx::new(new_connection_rx));

            tokio::spawn(async move {
                let config = ServerConfig::builder()
                    .with_bind_default(4433)
                    .with_certificate(Certificate::self_signed(["localhost"]))
                    .keep_alive_interval(Some(Duration::from_secs(3)))
                    .build();
                let server = Endpoint::server(config).unwrap();

                loop {
                    let incoming_session = server.accept().await;

                    let new_connection_tx = new_connection_tx.clone();
                    tokio::spawn(async move {
                        let session_request = incoming_session.await.unwrap();
                        let connection = session_request.accept().await.unwrap();

                        let (rx_packet_tx, rx_packet_rx) = mpsc::unbounded_channel();
                        let (tx_packet_tx, mut tx_packet_rx) = mpsc::unbounded_channel();
                        new_connection_tx
                            .send(Connection::new(rx_packet_rx, tx_packet_tx))
                            .unwrap();

                        loop {
                            tokio::select! {
                                message = connection.receive_datagram() => {
                                    if let Ok(message) = message {
                                        let _ = rx_packet_tx.send(message.to_vec());
                                    } else {
                                        break;
                                    }
                                }
                                message = tx_packet_rx.recv() => {
                                    if let Some(message) = message {
                                        if connection.send_datagram(message).is_err() {
                                            break;
                                        }
                                    } else {
                                        break;
                                    }
                                }
                            }
                        }
                        tx_packet_rx.close();
                        let _ = connection.close(0u8.into(), &[]);
                    });
                }
            });
        };

        app.add_systems(PostStartup, listen);
    }
}

#[derive(Resource)]
pub struct Client {
    new_connection_tx: UnboundedSender<Connection>
}

impl Client {
    pub fn connect(&self) {
        let new_connection_tx = self.new_connection_tx.clone();
        tokio::spawn(async move {
            let config = ClientConfig::builder()
                .with_bind_default()
                .with_no_cert_validation()
                .build();
            let connection = Endpoint::client(config)
                .unwrap()
                .connect("https://[::1]:4433")
                .await
                .unwrap();

            let (rx_packet_tx, rx_packet_rx) = mpsc::unbounded_channel();
            let (tx_packet_tx, mut tx_packet_rx) = mpsc::unbounded_channel();
            new_connection_tx
                .send(Connection::new(rx_packet_rx, tx_packet_tx))
                .unwrap();

            loop {
                tokio::select! {
                    message = connection.receive_datagram() => {
                        if let Ok(message) = message {
                            let _ = rx_packet_tx.send(message.to_vec());
                        } else {
                            break;
                        }
                    }
                    message = tx_packet_rx.recv() => {
                        if let Some(message) = message {
                            if connection.send_datagram(message).is_err() {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
            tx_packet_rx.close();
            let _ = connection.close(0u8.into(), &[]);
        });
    }
}

pub struct ClientPlugin;

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        let (new_connection_tx, new_connection_rx) = mpsc::unbounded_channel();
        app.insert_resource(NewConnectionRx::new(new_connection_rx));
        app.insert_resource(Client { new_connection_tx });
    }
}
