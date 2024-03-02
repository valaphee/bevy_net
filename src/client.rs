use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use bevy::{prelude::*, tasks::futures_lite::StreamExt};
use futures::SinkExt;
use tokio::{net::{TcpSocket, TcpStream}, sync::mpsc};
use tokio_util::codec::Framed;

use crate::{codec::Codec, Connection, NewConnectionRx};

pub struct ClientPlugin {
    pub address: SocketAddr
}

impl Default for ClientPlugin {
    fn default() -> Self {
        Self { address: SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 1337).into() }
    }
}

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        let address = self.address;

        let connect = move |mut commands: Commands| {
            let (new_connection_tx, new_connection_rx) = mpsc::unbounded_channel();

            commands.insert_resource(NewConnectionRx(new_connection_rx));

            std::thread::spawn(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async move {
                        let socket = TcpSocket::new_v4().unwrap();

                        info!("Connecting to {}", address);

                        let socket = socket.connect(address).await.unwrap();
                        tokio::spawn(handle_new_connection(
                            socket,
                            address,
                            new_connection_tx.clone(),
                        ));
                    })
            });
        };

        app
            .add_systems(PostStartup, connect);
    }
}

async fn handle_new_connection(
    socket: TcpStream,
    address: SocketAddr,
    new_connection_tx: mpsc::UnboundedSender<Connection>,
) {
    socket.set_nodelay(true).unwrap();

    let mut framed_socket = Framed::new(socket, Codec::default());

    let (rx_packet_tx, rx_packet_rx) = mpsc::unbounded_channel();
    let (tx_packet_tx, mut tx_packet_rx) = mpsc::unbounded_channel();
    let _ = new_connection_tx.send(Connection {
        address,
        rx: rx_packet_rx,
        tx: tx_packet_tx,
    });

    tokio::spawn(async move {
        loop {
            tokio::select! {
                packet = framed_socket.next() => {
                    if let Some(Ok(packet)) = packet {
                        let _ = rx_packet_tx.send(packet);
                    } else {
                        break;
                    }
                }
                packet = tx_packet_rx.recv() => {
                    if let Some(packet) = packet {
                        if framed_socket.send(&packet).await.is_err() {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        }
        tx_packet_rx.close();
        let _ = framed_socket.close().await;
    });
}
