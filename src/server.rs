use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use bevy::prelude::*;
use bytes::BytesMut;
use futures::{SinkExt, StreamExt};
use tokio::{net::{TcpListener, TcpStream}, sync::mpsc};
use tokio_util::codec::{BytesCodec, Framed};

use crate::{Connection, NewConnectionRx};

pub struct ServerPlugin {
    pub address: SocketAddr
}

impl Default for ServerPlugin {
    fn default() -> Self {
        Self { address: SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 1337).into() }
    }
}

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        let address = self.address;

        let listen = move |mut commands: Commands| {
            let (new_connection_tx, new_connection_rx) = mpsc::unbounded_channel();

            commands.insert_resource(NewConnectionRx(new_connection_rx));

            std::thread::spawn(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async move {
                        let listener = TcpListener::bind(address).await.unwrap();

                        info!("Listening on {}", address);

                        loop {
                            if let Ok((socket, address)) = listener.accept().await {
                                tokio::spawn(handle_new_connection(
                                    socket,
                                    address,
                                    new_connection_tx.clone(),
                                ));
                            }
                        }
                    })
            });
        };

        app
            .add_systems(PostStartup, listen);
    }
}

async fn handle_new_connection(
    socket: TcpStream,
    address: SocketAddr,
    new_connection_tx: mpsc::UnboundedSender<Connection>,
) {
    println!("listening..");
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
                        println!("A {:?}", packet);
                        break;
                    }
                }
                packet = tx_packet_rx.recv() => {
                    if let Some(packet) = packet {
                        if framed_socket.send(BytesMut::from(packet.as_slice())).await.is_err() {
                            break;
                        }
                    } else {
                        println!("B {:?}", packet);
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
