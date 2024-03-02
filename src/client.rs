use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use bevy::{prelude::*, tasks::futures_lite::StreamExt};
use bytes::BytesMut;
use futures::SinkExt;
use tokio::{net::{TcpSocket, TcpStream}, sync::mpsc};
use tokio_util::codec::{BytesCodec, Framed};

use crate::{Connection, NewConnectionRx};

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

            tokio::spawn(async move {
                        info!("Connecting to {}", address);

                        let socket = TcpStream::connect(address).await.unwrap();
                        handle_new_connection(
                            socket,
                            address,
                            new_connection_tx.clone(),
                        ).await;
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
    println!("connecting..");
    socket.set_nodelay(true).unwrap();

    let mut framed_socket = Framed::new(socket, BytesCodec::default());

    let (rx_packet_tx, rx_packet_rx) = mpsc::unbounded_channel();
    let (tx_packet_tx, mut tx_packet_rx) = mpsc::unbounded_channel();
    let _ = new_connection_tx.send(Connection {
        address,
        rx: rx_packet_rx,
        tx: tx_packet_tx,
    });

    tokio::spawn(async move {
        loop {
            info!("handling messages");
            tokio::select! {
                packet = framed_socket.next() => {
                    if let Some(Ok(packet)) = packet {
                        let _ = rx_packet_tx.send(packet.to_vec());
                    } else {
                        break;
                    }
                }
                packet = tx_packet_rx.recv() => {
                    if let Some(packet) = packet {
                        if framed_socket.send(BytesMut::from(packet.as_slice())).await.is_err() {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        }
        info!("finished messages");
        tx_packet_rx.close();
        //let _ = framed_socket.close().await;
    });
}
