use std::net::SocketAddr;

use bevy::{
    app::{App, Plugin, PostUpdate, PreUpdate, Update},
    ecs::{
        component::{Component, ComponentId, StorageType},
        entity::Entity,
        event::{Event, EventReader, ManualEventReader},
        removal_detection::{RemovedComponentEntity, RemovedComponentEvents},
        system::{Commands, Local, Query, Res, ResMut, Resource, SystemChangeTick},
        world::World,
    },
    utils::HashMap,
};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use serde::{Deserialize, Serialize};
use tokio::{
    runtime::{Handle, Runtime},
    sync::mpsc,
};
use wtransport::{Certificate, ClientConfig, Endpoint, ServerConfig};

use crate::replication::{Received, Replication};

pub struct NetworkPlugin;

impl Plugin for NetworkPlugin {
    fn build(&self, app: &mut App) {
        let tokio_runtime = Runtime::new().unwrap();
        let tokio_handle = tokio_runtime.handle().clone();
        let (incoming_connection_tx, incoming_connection_rx) = mpsc::unbounded_channel();
        app.insert_resource(Network {
            _tokio_runtime: tokio_runtime,
            tokio_handle,
            incoming_connection_tx,
            incoming_connection_rx,
        });

        app.add_systems(PreUpdate, (recv_updates, spawn_connections))
            .add_systems(Update, update_entity_links)
            .add_systems(PostUpdate, send_updates);
    }
}

#[derive(Resource)]
pub struct Network {
    _tokio_runtime: Runtime,
    tokio_handle: Handle,

    incoming_connection_rx: mpsc::UnboundedReceiver<Connection>,
    incoming_connection_tx: mpsc::UnboundedSender<Connection>,
}

impl Network {
    pub fn listen(&self, address: SocketAddr) {
        let _guard = self.tokio_handle.enter();

        let incoming_connection_tx = self.incoming_connection_tx.clone();
        tokio::spawn(async move {
            let server_config = ServerConfig::builder()
                .with_bind_address(address)
                .with_certificate(Certificate::self_signed(["localhost"]))
                .build();
            let endpoint = Endpoint::server(server_config).unwrap();

            loop {
                let incoming_session = endpoint.accept().await;

                let incoming_connection_tx = incoming_connection_tx.clone();
                tokio::spawn(async move {
                    let session_request = incoming_session.await.unwrap();
                    let connection = session_request.accept().await.unwrap();

                    handle_connection(incoming_connection_tx, connection).await;
                });
            }
        });
    }

    pub fn connect(&self, url: String) {
        let _guard = self.tokio_handle.enter();

        let incoming_connection_tx = self.incoming_connection_tx.clone();
        tokio::spawn(async move {
            let client_config = ClientConfig::builder()
                .with_bind_default()
                .with_no_cert_validation()
                .build();
            let connection = Endpoint::client(client_config)
                .unwrap()
                .connect(url)
                .await
                .unwrap();

            handle_connection(incoming_connection_tx, connection).await;
        });
    }
}

async fn handle_connection(
    incoming_connection_tx: mpsc::UnboundedSender<Connection>,
    connection: wtransport::connection::Connection,
) {
    let (rx_message_tx, rx_message_rx) = mpsc::unbounded_channel();
    let (tx_message_tx, mut tx_message_rx) = mpsc::unbounded_channel();
    incoming_connection_tx
        .send(Connection {
            message_rx: rx_message_rx,
            message_tx: tx_message_tx,
            entity_links: HashMap::default(),
        })
        .unwrap();

    loop {
        tokio::select! {
            message = connection.receive_datagram() => {
                if let Ok(message) = message {
                    let _ = rx_message_tx.send(message.to_vec());
                } else {
                    break;
                }
            }
            message = tx_message_rx.recv() => {
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

    tx_message_rx.close();
    let _ = connection.close(0u8.into(), &[]);
}

#[derive(Component)]
pub struct Connection {
    message_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    message_tx: mpsc::UnboundedSender<Vec<u8>>,

    entity_links: HashMap<u32, Entity>,
}

#[derive(Event, Serialize, Deserialize)]
pub struct EntityLinkEvent {
    local: u32,
    remote: u32,
}

fn spawn_connections(mut commands: Commands, mut network: ResMut<Network>) {
    while let Ok(connection) = network.incoming_connection_rx.try_recv() {
        commands.spawn(connection);
    }
}

fn update_entity_links(
    mut entity_link_events: EventReader<Received<EntityLinkEvent>>,
    mut connections: Query<&mut Connection>,
) {
    for event in entity_link_events.read() {
        connections
            .get_mut(event.source)
            .unwrap()
            .entity_links
            .insert(event.event.local, Entity::from_raw(event.event.remote));
    }
}

fn recv_updates(world: &mut World) {
    let unsafe_world_cell = world.as_unsafe_world_cell();

    let replication = unsafe { unsafe_world_cell.get_resource::<Replication>() }.unwrap();
    let mut connections =
        unsafe { unsafe_world_cell.world_mut() }.query::<(Entity, &mut Connection)>();
    for (entity, mut connection) in connections.iter_mut(unsafe { unsafe_world_cell.world_mut() }) {
        while let Ok(message) = connection.message_rx.try_recv() {
            let mut message = message.as_slice();

            let mut type_hash = message.read_u32::<LittleEndian>().unwrap();
            while type_hash != 0 {
                if let Some(deserializer) = replication.recv_resource.get(&type_hash) {
                    deserializer(unsafe { unsafe_world_cell.world_mut() }, &mut message);
                } else if let Some(deserializer) = replication.recv_event.get(&type_hash) {
                    deserializer(
                        unsafe { unsafe_world_cell.world_mut() },
                        &mut message,
                        entity,
                    );
                } else if let Some((deserializer, _remover)) =
                    replication.recv_component.get(&type_hash)
                {
                    let mut entity = message.read_u32::<LittleEndian>().unwrap();
                    while entity != 0 {
                        let mut entity_world = if let Some(entity) =
                            connection.entity_links.get(&entity)
                        {
                            unsafe { unsafe_world_cell.world_mut() }.entity_mut(*entity)
                        } else {
                            let entity_world = unsafe { unsafe_world_cell.world_mut() }.spawn(());
                            connection.entity_links.insert(entity, entity_world.id());
                            unsafe { unsafe_world_cell.world_mut() }.send_event(EntityLinkEvent {
                                remote: entity,
                                local: entity_world.id().index(),
                            });
                            entity_world
                        };
                        deserializer(&mut entity_world, &mut message);

                        entity = message.read_u32::<LittleEndian>().unwrap();
                    }
                }
                type_hash = message.read_u32::<LittleEndian>().unwrap();
            }
        }
    }
}

#[derive(Default)]
struct EventReaders(HashMap<u32, usize>);

fn send_updates(
    connections: Query<&Connection>,

    change_tick: SystemChangeTick,
    mut event_readers: Local<EventReaders>,
    mut component_remove_event_readers: Local<
        HashMap<ComponentId, ManualEventReader<RemovedComponentEntity>>,
    >,

    world: &World,
    component_remove_events: &RemovedComponentEvents,

    replication: Res<Replication>,
) {
    for connection in connections.iter() {
        let mut message = Vec::new();

        // Write all resources added/updated.
        for (component_id, send_resource_data) in replication.send_resource.iter() {
            let resource_data = world.storages().resources.get(*component_id).unwrap();
            let resource = unsafe { resource_data.get_data().unwrap_unchecked() };
            let ticks = unsafe { resource_data.get_ticks().unwrap_unchecked() };
            if !ticks.is_changed(change_tick.last_run(), change_tick.this_run()) {
                // Not changed since last update, skip.
                continue;
            }

            message
                .write_u32::<LittleEndian>(send_resource_data.type_hash)
                .unwrap();
            (send_resource_data.serializer)(resource, &mut message);
        }

        // Write all events.
        for (component_id, send_event_data) in replication.send_event.iter() {
            let resource_data = world.storages().resources.get(*component_id).unwrap();
            let resource = unsafe { resource_data.get_data().unwrap_unchecked() };
            let event_reader = event_readers
                .0
                .entry(send_event_data.type_hash)
                .or_default();

            message
                .write_u32::<LittleEndian>(send_event_data.type_hash)
                .unwrap();
            let message_data_begin = message.len();
            (send_event_data.serializer)(resource, &mut message, event_reader);

            // Check if any event was written.
            if message_data_begin == message.len() {
                // Drop type hash.
                message.truncate(message_data_begin - std::mem::size_of::<u32>());
            }
        }

        // Write all components added/updated.
        for archetype in world.archetypes().iter() {
            // SAFETY: The archetype was obtained from this world and always
            // has a table.
            let table = unsafe {
                world
                    .storages()
                    .tables
                    .get(archetype.table_id())
                    .unwrap_unchecked()
            };
            for component_id in archetype.components() {
                let Some(send_component_data) = replication.send_component.get(&component_id)
                else {
                    // No component data found, skip.
                    continue;
                };

                message
                    .write_u32::<LittleEndian>(send_component_data.type_hash)
                    .unwrap();
                let mut message_array_begin = message.len();

                // SAFETY: The component was obtained from this archetype and
                // always has a storage type.
                let storage_type =
                    unsafe { archetype.get_storage_type(component_id).unwrap_unchecked() };
                match storage_type {
                    StorageType::Table => {
                        // SAFETY: The storage type matches StorageType::Table
                        // and therefore is contained in the table of this
                        // archetype.
                        let column = unsafe { table.get_column(component_id).unwrap_unchecked() };
                        for archetype_entity in archetype.entities() {
                            // SAFETY: The entity is obtained from this
                            // archetype and therefore is contained in this
                            // archetypes table.
                            let component =
                                unsafe { column.get_data_unchecked(archetype_entity.table_row()) };
                            // SAFETY: See above.
                            let ticks =
                                unsafe { column.get_ticks_unchecked(archetype_entity.table_row()) };
                            if !ticks.is_changed(change_tick.last_run(), change_tick.this_run()) {
                                // Not changed since last update, skip.
                                continue;
                            }

                            let message_element_begin = message.len();
                            message
                                .write_u32::<LittleEndian>(archetype_entity.id().index())
                                .unwrap();
                            (send_component_data.serializer)(component, &mut message);

                            if message.len() > 1346 {
                                let mut message_array_end =
                                    &mut message[message_element_begin..][..4];
                                message_array_end.write_u32::<LittleEndian>(0).unwrap();
                                connection
                                    .message_tx
                                    .send(message[..message_element_begin + 4].to_vec())
                                    .unwrap();
                                message.clear();

                                message
                                    .write_u32::<LittleEndian>(send_component_data.type_hash)
                                    .unwrap();
                                message_array_begin = message.len();
                                message
                                    .write_u32::<LittleEndian>(archetype_entity.id().index())
                                    .unwrap();
                                (send_component_data.serializer)(component, &mut message);
                            }
                        }
                    }
                    StorageType::SparseSet => {
                        // SAFETY: The storage type matches
                        // StorageType::SparseSet and therefore has a sparse
                        // set in this world.
                        let sparse_set = unsafe {
                            world
                                .storages()
                                .sparse_sets
                                .get(component_id)
                                .unwrap_unchecked()
                        };
                        for archetype_entity in archetype.entities() {
                            // SAFETY: The entity is obtained from this
                            // archetype and therefore is contained in this
                            // archetypes components sparse set.
                            let component =
                                unsafe { sparse_set.get(archetype_entity.id()).unwrap_unchecked() };
                            // SAFETY: See above.
                            let ticks = unsafe {
                                sparse_set
                                    .get_ticks(archetype_entity.id())
                                    .unwrap_unchecked()
                            };
                            if !ticks.is_changed(change_tick.last_run(), change_tick.this_run()) {
                                // Not changed since last update, skip.
                                continue;
                            }

                            let message_element_begin = message.len();
                            message
                                .write_u32::<LittleEndian>(archetype_entity.id().index())
                                .unwrap();
                            (send_component_data.serializer)(component, &mut message);

                            if message.len() > 1346 {
                                let mut message_array_end =
                                    &mut message[message_element_begin..][..4];
                                message_array_end.write_u32::<LittleEndian>(0).unwrap();
                                connection
                                    .message_tx
                                    .send(message[..message_element_begin + 4].to_vec())
                                    .unwrap();
                                message.clear();

                                message
                                    .write_u32::<LittleEndian>(send_component_data.type_hash)
                                    .unwrap();
                                message_array_begin = message.len();
                                message
                                    .write_u32::<LittleEndian>(archetype_entity.id().index())
                                    .unwrap();
                                (send_component_data.serializer)(component, &mut message);
                            }
                        }
                    }
                }

                // Check if any component was written.
                if message_array_begin == message.len() {
                    // Drop type hash.
                    message.truncate(message_array_begin - std::mem::size_of::<u32>());
                } else {
                    // Null-terminated.
                    message.write_u32::<LittleEndian>(0).unwrap();
                }
            }
        }

        // Null-terminated.
        message.write_u32::<LittleEndian>(0).unwrap();

        // Skip if update is empty.
        if message.len() == 1 + 4 + 4 {
            continue;
        }
        connection.message_tx.send(message).unwrap();
    }
}
