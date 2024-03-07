use std::{io, sync::Arc};

use bevy::{
    app::{App, Plugin, PostStartup},
    ecs::system::Commands,
};
use tokio::sync::{mpsc, Mutex};
use webrtc::{
    api::APIBuilder,
    data_channel::data_channel_init::RTCDataChannelInit,
    ice_transport::{
        ice_candidate::{RTCIceCandidate, RTCIceCandidateInit},
        ice_server::RTCIceServer,
    },
    peer_connection::{
        configuration::RTCConfiguration, sdp::session_description::RTCSessionDescription,
    },
};

use crate::replication::{Connection, NewConnectionRx};

pub struct ServerPlugin;

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        let listen = move |mut commands: Commands| {
            let (new_connection_tx, new_connection_rx) = mpsc::unbounded_channel();
            commands.insert_resource(NewConnectionRx::new(new_connection_rx));

            tokio::spawn(async move {
                let api = APIBuilder::new().build();

                loop {
                    must_read_stdin().unwrap();
                    let line = std::fs::read_to_string("offer.json").unwrap();
                    let offer = serde_json::from_str::<RTCSessionDescription>(&line).unwrap();

                    let config = RTCConfiguration {
                        ice_servers: vec![RTCIceServer {
                            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                            ..Default::default()
                        }],
                        ..Default::default()
                    };
                    let peer_connection = api.new_peer_connection(config).await.unwrap();

                    let (rx_packet_tx, rx_packet_rx) = mpsc::unbounded_channel();
                    let (tx_packet_tx, tx_packet_rx) = mpsc::unbounded_channel();
                    new_connection_tx
                        .send(Connection::new(rx_packet_rx, tx_packet_tx))
                        .unwrap();
                    let tx_packet_rx = Arc::new(Mutex::new(tx_packet_rx));
                    peer_connection.on_data_channel(Box::new(move |data_channel| {
                        let rx_packet_tx = rx_packet_tx.clone();
                        let tx_packet_rx = tx_packet_rx.clone();
                        Box::pin(async move {
                            let data_channel_2 = data_channel.clone();
                            data_channel.on_open(Box::new(move || {
                                let tx_packet_rx = tx_packet_rx.clone();
                                Box::pin(async move {
                                    let mut tx_packet_rx = tx_packet_rx.lock().await;
                                    while let Some(message) = tx_packet_rx.recv().await {
                                        data_channel_2.send(&message.into()).await.unwrap();
                                    }
                                })
                            }));
                            data_channel.on_message(Box::new(move |message| {
                                let rx_packet_tx = rx_packet_tx.clone();
                                Box::pin(async move {
                                    let _ = rx_packet_tx.send(message.data.to_vec());
                                })
                            }));
                        })
                    }));

                    peer_connection.set_remote_description(offer).await.unwrap();

                    let answer = peer_connection.create_answer(None).await.unwrap();
                    let mut gather_complete = peer_connection.gathering_complete_promise().await;
                    peer_connection.set_local_description(answer).await.unwrap();
                    let _ = gather_complete.recv().await;

                    if let Some(local_desc) = peer_connection.local_description().await {
                        let json_str = serde_json::to_string(&local_desc).unwrap();
                        println!("{json_str}");
                        std::fs::write("answer.json", json_str).unwrap();
                    } else {
                        println!("generate local_description failed!");
                    }
                }
            });
        };

        app.add_systems(PostStartup, listen);
    }
}

pub struct ClientPlugin;

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        let connect = move |mut commands: Commands| {
            let (new_connection_tx, new_connection_rx) = mpsc::unbounded_channel();
            commands.insert_resource(NewConnectionRx::new(new_connection_rx));

            tokio::spawn(async move {
                let api = APIBuilder::new().build();

                let config = RTCConfiguration {
                    ice_servers: vec![RTCIceServer {
                        urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                        ..Default::default()
                    }],
                    ..Default::default()
                };
                let peer_connection = api.new_peer_connection(config).await.unwrap();

                let (rx_packet_tx, rx_packet_rx) = mpsc::unbounded_channel();
                let (tx_packet_tx, tx_packet_rx) = mpsc::unbounded_channel();
                new_connection_tx
                    .send(Connection::new(rx_packet_rx, tx_packet_tx))
                    .unwrap();
                let tx_packet_rx = Arc::new(Mutex::new(tx_packet_rx));
                let data_channel = peer_connection
                    .create_data_channel(
                        "data",
                        Some(RTCDataChannelInit {
                            ordered: Some(false),
                            max_retransmits: Some(0),
                            ..Default::default()
                        }),
                    )
                    .await
                    .unwrap();
                let data_channel_2 = data_channel.clone();
                data_channel.on_open(Box::new(move || {
                    let tx_packet_rx = tx_packet_rx.clone();
                    Box::pin(async move {
                        let mut tx_packet_rx = tx_packet_rx.lock().await;
                        while let Some(message) = tx_packet_rx.recv().await {
                            data_channel_2.send(&message.into()).await.unwrap();
                        }
                    })
                }));
                data_channel.on_message(Box::new(move |message| {
                    let rx_packet_tx = rx_packet_tx.clone();
                    Box::pin(async move {
                        let _ = rx_packet_tx.send(message.data.to_vec());
                    })
                }));

                let offer = peer_connection.create_offer(None).await.unwrap();
                let mut gather_complete = peer_connection.gathering_complete_promise().await;
                peer_connection.set_local_description(offer).await.unwrap();
                let _ = gather_complete.recv().await;

                if let Some(local_desc) = peer_connection.local_description().await {
                    let json_str = serde_json::to_string(&local_desc).unwrap();
                    println!("{json_str}");
                    std::fs::write("offer.json", json_str).unwrap();
                } else {
                    println!("generate local_description failed!");
                }

                must_read_stdin().unwrap();
                let line = std::fs::read_to_string("answer.json").unwrap();
                let answer = serde_json::from_str::<RTCSessionDescription>(&line).unwrap();
                peer_connection
                    .set_remote_description(answer)
                    .await
                    .unwrap();
            });
        };

        app.add_systems(PostStartup, connect);
    }
}

fn must_read_stdin() -> io::Result<String> {
    let mut line = String::new();

    std::io::stdin().read_line(&mut line)?;
    line = line.trim().to_owned();
    println!();

    Ok(line)
}
