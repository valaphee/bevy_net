use std::net::SocketAddr;

use bevy::{app::{App, Plugin, PostStartup, PostUpdate, PreUpdate}, ecs::{component::Component, entity::Entity, reflect::{AppTypeRegistry, ReflectComponent, ReflectEvent}, system::{Commands, ResMut, Resource}, world::World}, log::info, reflect::serde::TypedReflectDeserializer, scene::ron, utils::HashMap};
use byteorder::{LittleEndian, ReadBytesExt};
use futures::{SinkExt, StreamExt};
use tokio::{net::{TcpListener, TcpStream}, sync::mpsc};
use tokio_util::{bytes::BytesMut, codec::{BytesCodec, Framed}};
use serde::de::DeserializeSeed;

pub struct ServerPlugin {
    pub address: SocketAddr
}

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        let address = self.address;

        let listen = move |mut commands: Commands| {
            let (new_connection_tx, new_connection_rx) = mpsc::unbounded_channel();
            commands.insert_resource(Transport { new_connection_rx });

            std::thread::spawn(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async move {
                        let listener = TcpListener::bind(address).await.unwrap();
                        info!("Listening on {}", address);

                        loop {
                            if let Ok((stream, _address)) = listener.accept().await {
                                let mut framed_stream = Framed::new(stream, BytesCodec::default());

                                let (rx_message_tx, rx_message_rx) = mpsc::unbounded_channel();
                                let (tx_message_tx, mut tx_message_rx) = mpsc::unbounded_channel();
                                let _ = new_connection_tx.send(Connection {
                                    entity_map: Default::default(),
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
                    })
            });
        };

        app
            .add_systems(PostStartup, listen)
            .add_systems(PreUpdate, spawn_connection)
            .add_systems(PostUpdate, handle_connection_recv);
    }
}

pub struct ClientPlugin {
    pub address: SocketAddr
}

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        let address = self.address;

        let connect = move |mut commands: Commands| {
            let (new_connection_tx, new_connection_rx) = mpsc::unbounded_channel();
            commands.insert_resource(Transport { new_connection_rx });

            std::thread::spawn(move || {
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async move {
                        info!("Connecting to {}", address);
                        let stream = TcpStream::connect(address).await.unwrap();
                        let mut framed_stream = Framed::new(stream, BytesCodec::default());

                        let (rx_message_tx, rx_message_rx) = mpsc::unbounded_channel();
                        let (tx_message_tx, mut tx_message_rx) = mpsc::unbounded_channel();
                        let _ = new_connection_tx.send(Connection {
                            entity_map: Default::default(),
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
                    })
            });
        };

        app
            .add_systems(PostStartup, connect)
            .add_systems(PreUpdate, spawn_connection)
            .add_systems(PostUpdate, handle_connection_recv);
    }
}

#[derive(Resource)]
pub struct Transport {
    new_connection_rx: mpsc::UnboundedReceiver<Connection>,
}

#[derive(Component)]
pub struct Connection {
    entity_map: HashMap<u64, Entity>,

    message_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    message_tx: mpsc::UnboundedSender<Vec<u8>>,
}

fn spawn_connection(
    mut commands: Commands,

    mut transport: ResMut<Transport>
) {
    while let Ok(connection) = transport.new_connection_rx.try_recv() {
        commands.spawn(connection);
    }
}

fn handle_connection_send(
    world: &World,
) {
}

fn handle_connection_recv(
    world: &mut World,
) {
    let mut connections = world.query::<&mut Connection>();
    let unsafe_world_cell = world.as_unsafe_world_cell();
    let type_registry = unsafe { unsafe_world_cell.get_resource_mut::<AppTypeRegistry>() }.unwrap();
    let type_registry = type_registry.read();
    for mut connection in connections.iter_mut(unsafe { unsafe_world_cell.world_mut() }) {
        while let Ok(message) = connection.message_rx.try_recv() {
            let mut message = &message[..];

            let entity_map = &mut connection.entity_map;

            let type_count = message.read_u8().unwrap();
            for _ in 0..type_count {
                let r#type = message.read_u128::<LittleEndian>().unwrap();
                let type_registration = type_registry.get(unsafe { std::mem::transmute(r#type) }).unwrap();
        
                let data_count = message.read_u8().unwrap();
                if let Some(reflect_component) = type_registration.data::<ReflectComponent>() {
                    for _ in 0..data_count {
                        let entity = message.read_u64::<LittleEndian>().unwrap();
                        let data_length = message.read_u16::<LittleEndian>().unwrap();
                        let (data, remaining_packet) = message.split_at(data_length as usize);
                        message = remaining_packet;
        
                        let mut ron_deserializer = ron::Deserializer::from_bytes(data).unwrap();
                        let component = TypedReflectDeserializer::new(type_registration, &type_registry).deserialize(&mut ron_deserializer).unwrap();
                        let mut entity_world = if let Some(&entity) = entity_map.get(&entity) {
                            unsafe { unsafe_world_cell.world_mut() }.entity_mut(entity)
                        } else {
                            let entity_world = unsafe { unsafe_world_cell.world_mut() }.spawn(());
                            entity_map.insert(entity, entity_world.id());
                            entity_world
                        };
                        reflect_component.apply_or_insert(&mut entity_world, component.as_ref(), &type_registry)
                    }
                } else if let Some(reflect_event) = type_registration.data::<ReflectEvent>() {
                    for _ in 0..data_count {
                        let data_length = message.read_u16::<LittleEndian>().unwrap();
                        let (data, remaining_packet) = message.split_at(data_length as usize);
                        message = remaining_packet;
        
                        let mut ron_deserializer = ron::Deserializer::from_bytes(data).unwrap();
                        let event = TypedReflectDeserializer::new(type_registration, &type_registry).deserialize(&mut ron_deserializer).unwrap();
                        reflect_event.send(unsafe { unsafe_world_cell.world_mut() }, event.as_ref(), &type_registry);
                    }
                }
            }

            let entity_count = message.read_u8().unwrap();
            for _ in 0..entity_count {
                let entity = message.read_u64::<LittleEndian>().unwrap();
                let Some(&entity) = entity_map.get(&entity) else {
                    continue;
                };
                let mut entity_world = unsafe { unsafe_world_cell.world_mut() }.entity_mut(entity);

                let type_count = message.read_u8().unwrap();
                if type_count == 0 {
                    entity_world.despawn();
                    continue;
                }

                for _ in 0..type_count {
                    let r#type = message.read_u128::<LittleEndian>().unwrap();
                    let type_registration = type_registry.get(unsafe { std::mem::transmute(r#type) }).unwrap();
        
                    if let Some(reflect_component) = type_registration.data::<ReflectComponent>() {
                        reflect_component.remove(&mut entity_world);
                    }
                }
            }
        }
    }
}
