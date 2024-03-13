use std::time::Duration;

use bevy::{app::{App, Plugin, PostStartup}, ecs::{component::{Component, ComponentId, StorageType}, entity::Entity, event::ManualEventReader, removal_detection::{RemovedComponentEntity, RemovedComponentEvents}, system::{Commands, Local, Query, Res, ResMut, Resource, SystemChangeTick}, world::World}, utils::HashMap};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use tokio::sync::mpsc;
use wtransport::{Certificate, ClientConfig, Endpoint, ServerConfig};

use crate::replication::Replication;

pub struct ServerPlugin;

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        let listen = move |mut commands: Commands| {
            let (incoming_connection_tx, incoming_connection_rx) = mpsc::unbounded_channel();
            commands.insert_resource(IncomingConnectionRx(incoming_connection_rx));

            tokio::spawn(async move {
                let server_config = ServerConfig::builder()
                        .with_bind_default(4433)
                        .with_certificate(Certificate::self_signed(["localhost"]))
                        .keep_alive_interval(Some(Duration::from_secs(3)))
                        .build();
                let endpoint = Endpoint::server(server_config).unwrap();

                loop {
                    let incoming_session = endpoint.accept().await;

                    let incoming_connection_tx = incoming_connection_tx.clone();
                    tokio::spawn(async move {
                        let session_request = incoming_session.await.unwrap();
                        let connection = session_request.accept().await.unwrap();

                        let (rx_tx, rx_rx) = mpsc::unbounded_channel();
                        let (tx_tx, mut tx_rx) = mpsc::unbounded_channel();
                        incoming_connection_tx
                            .send(Connection { rx: rx_rx, tx: tx_tx })
                            .unwrap();

                        loop {
                            tokio::select! {
                                message = connection.receive_datagram() => {
                                    if let Ok(message) = message {
                                        let _ = rx_tx.send(message.to_vec());
                                    } else {
                                        break;
                                    }
                                }
                                message = tx_rx.recv() => {
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

                        tx_rx.close();
                        let _ = connection.close(0u8.into(), &[]);
                    });
                }
            });
        };

        app.add_systems(PostStartup, listen);
    }
}

pub struct ClientPlugin;

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        let (incoming_connection_tx, incoming_connection_rx) = mpsc::unbounded_channel();
        app.insert_resource(IncomingConnectionRx(incoming_connection_rx));
        app.insert_resource(Client { incoming_connection_tx });
    }
}

#[derive(Resource)]
pub struct Client {
    incoming_connection_tx: mpsc::UnboundedSender<Connection>
}

impl Client {
    pub fn connect(&self, url: String) {
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

            let (rx_tx, rx_rx) = mpsc::unbounded_channel();
            let (tx_tx, mut tx_rx) = mpsc::unbounded_channel();
            incoming_connection_tx
                .send(Connection { rx: rx_rx, tx: tx_tx })
                .unwrap();

            loop {
                tokio::select! {
                    message = connection.receive_datagram() => {
                        if let Ok(message) = message {
                            let _ = rx_tx.send(message.to_vec());
                        } else {
                            break;
                        }
                    }
                    message = tx_rx.recv() => {
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

            tx_rx.close();
            let _ = connection.close(0u8.into(), &[]);
        });
    }
}

#[derive(Resource)]
struct IncomingConnectionRx(mpsc::UnboundedReceiver<Connection>);

#[derive(Component)]
pub struct Connection {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    tx: mpsc::UnboundedSender<Vec<u8>>,
}

fn spawn_incoming_connections(mut commands: Commands, mut incoming_connection_rx: ResMut<IncomingConnectionRx>) {
    while let Ok(connection) = incoming_connection_rx.0.try_recv() {
        commands.spawn(connection);
    }
}

fn recv_updates(world: &mut World) {
    let unsafe_world_cell = world.as_unsafe_world_cell();

    let replication = unsafe { unsafe_world_cell.get_resource::<Replication>() }.unwrap();
    let mut connections = unsafe { unsafe_world_cell.world_mut() }.query::<(Entity, &mut Connection)>();
    for (entity, mut connection) in connections.iter_mut(unsafe { unsafe_world_cell.world_mut() }) {
        while let Ok(update) = connection.rx.try_recv() {s
            let mut update = update.as_slice();

            // Read all events.
            let mut type_hash = update.read_u32::<LittleEndian>().unwrap();
            while type_hash != 0 {
                if let Some(deserializer) = replication.recv_resource.get(&type_hash) {
                    deserializer(unsafe { unsafe_world_cell.world_mut() }, &mut update);
                } else if let Some(deserializer) = replication.recv_event.get(&type_hash) {
                    deserializer(unsafe { unsafe_world_cell.world_mut() }, &mut update, entity);
                } else if let Some((deserializer, remover)) = replication
                    .recv_component
                    .get(&type_hash)
                {
                    if update.read_u8().unwrap() == 0 {
                        let mut entity = update.read_u32::<LittleEndian>().unwrap();
                        while entity != 0 {
                            let mut entity_world = if let Some(entity) =
                                connection.entity_links.get(&entity)
                            {
                                unsafe { unsafe_world_cell.world_mut() }.entity_mut(*entity)
                            } else {
                                let entity_world = unsafe { unsafe_world_cell.world_mut() }.spawn(());
                                connection.entity_links.insert(entity, entity_world.id());
                                unsafe { unsafe_world_cell.world_mut() }.send_event(LinkEntityEvent {
                                    remote: entity,
                                    local: entity_world.id(),
                                });
                                entity_world
                            };
                            deserializer(&mut entity_world, &mut update);
    
                            entity = update.read_u32::<LittleEndian>().unwrap();
                        }
                    } else {
                        let mut entity = update.read_u32::<LittleEndian>().unwrap();
                        while entity != 0 {
                            if let Some(entity) = connection.entity_links.get(&entity) {
                                let mut entity_world =
                                    unsafe { unsafe_world_cell.world_mut() }.entity_mut(*entity);
                                remover(&mut entity_world);
                            }
                            entity = update.read_u32::<LittleEndian>().unwrap();
                        }
                    }
                }
                type_hash = update.read_u32::<LittleEndian>().unwrap();
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
        let mut update = Vec::new();

        // Write all resources.
        for (component_id, send_resource_data) in replication.send_resource.iter() {
            let resource_data = world.storages().resources.get(*component_id).unwrap();
            let resource = unsafe { resource_data.get_data().unwrap_unchecked() };

            update
                .write_u32::<LittleEndian>(send_resource_data.type_hash)
                .unwrap();
            let update_pos = update.len();
            (send_resource_data.serializer)(resource, &mut update);

            // Check if any event was written.
            if update_pos == update.len() {
                // Drop type hash.
                update.truncate(update_pos - std::mem::size_of::<u32>());
            }
        }

        // Write all events.
        for (component_id, send_event_data) in replication.send_event.iter() {
            let resource_data = world.storages().resources.get(*component_id).unwrap();
            let resource = unsafe { resource_data.get_data().unwrap_unchecked() };
            let event_reader = event_readers
                .0
                .entry(send_event_data.type_hash)
                .or_default();

            update
                .write_u32::<LittleEndian>(send_event_data.type_hash)
                .unwrap();
            let update_pos = update.len();
            (send_event_data.serializer)(resource, &mut update, event_reader);

            // Check if any event was written.
            if update_pos == update.len() {
                // Drop type hash.
                update.truncate(update_pos - std::mem::size_of::<u32>());
            }
        }

        // Write all component remove events (bundle together with events as they have
        // the same reliability requirements).
        for (component_id, send_component_data) in &replication.send_component {
            let Some(events) = component_remove_events.get(*component_id) else {
                continue;
            };
            let event_reader = component_remove_event_readers
                .entry(*component_id)
                .or_default();

            update
                .write_u32::<LittleEndian>(send_component_data.type_hash)
                .unwrap();
            let update_pos = update.len();
            for event in event_reader.read(events).cloned() {
                update
                    .write_u32::<LittleEndian>(Entity::from(event).index())
                    .unwrap();
            }

            // Check if any entity was written.
            if update_pos == update.len() {
                // Drop type hash.
                update.truncate(update_pos - std::mem::size_of::<u32>());
            } else {
                // Null-terminated.
                update.write_u32::<LittleEndian>(0).unwrap();
            }
        }

        // Write all components added/updated.
        for archetype in world.archetypes().iter() {
            // SAFETY: The archetype was obtained from this world and always
            // has an table.
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

                update
                    .write_u32::<LittleEndian>(send_component_data.type_hash)
                    .unwrap();
                let mut update_pos = update.len();

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

                            let update_pos_inner = update.len();
                            update
                                .write_u32::<LittleEndian>(archetype_entity.id().index())
                                .unwrap();
                            (send_component_data.serializer)(component, &mut update);

                            if update.len() > 1346 {
                                {
                                    let mut update_ow = &mut update[update_pos_inner..][..4];
                                    update_ow.write_u32::<LittleEndian>(0);
                                }
                                connection.tx.send(update.clone()).unwrap();
                                update.clear();
                                update.write_u8(0);
                                update.write_u32::<LittleEndian>(0);

                                update
                                    .write_u32::<LittleEndian>(send_component_data.type_hash)
                                    .unwrap();
                                update_pos = update.len();
                                update
                                    .write_u32::<LittleEndian>(archetype_entity.id().index())
                                    .unwrap();
                                (send_component_data.serializer)(component, &mut update);
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

                            let update_pos_inner = update.len();
                            update
                                .write_u32::<LittleEndian>(archetype_entity.id().index())
                                .unwrap();
                            (send_component_data.serializer)(component, &mut update);

                            if update.len() > 1346 {
                                {
                                    let mut update_ow = &mut update[update_pos_inner..][..4];
                                    update_ow.write_u32::<LittleEndian>(0);
                                }
                                connection.tx.send(update.clone()).unwrap();
                                update.clear();
                                update.write_u8(0);
                                update.write_u32::<LittleEndian>(0);

                                update
                                    .write_u32::<LittleEndian>(send_component_data.type_hash)
                                    .unwrap();
                                update_pos = update.len();
                                update
                                    .write_u32::<LittleEndian>(archetype_entity.id().index())
                                    .unwrap();
                                (send_component_data.serializer)(component, &mut update);
                            }
                        }
                    }
                }

                // Check if any component was written.
                if update_pos == update.len() {
                    // Drop type hash.
                    update.truncate(update_pos - std::mem::size_of::<u32>());
                } else {
                    // Null-terminated.
                    update.write_u32::<LittleEndian>(0).unwrap();
                }
            }
        }

        // Null-terminated.
        update.write_u32::<LittleEndian>(0).unwrap();

        // Skip if update is empty.
        if update.len() == 1 + 4 + 4 {
            continue;
        }
        connection.tx.send(update).unwrap();
    }
}
