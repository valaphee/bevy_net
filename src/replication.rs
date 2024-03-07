use std::{
    any::type_name,
    hash::{DefaultHasher, Hash, Hasher},
    io::Write,
};

use bevy::{
    app::{App, Plugin, PostUpdate, PreUpdate},
    ecs::{
        component::{Component, ComponentId, StorageType},
        entity::Entity,
        event::{Event, Events, ManualEventReader},
        removal_detection::{RemovedComponentEntity, RemovedComponentEvents},
        system::{Commands, Local, Query, Res, ResMut, Resource, SystemChangeTick},
        world::{EntityWorldMut, World},
    },
    ptr::Ptr,
    utils::HashMap,
};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::mpsc;

/// Allows for replication of components and events between multiple Bevy
/// instances.
///
/// Replication is opt-in, components or events that need to be sent or received
/// have to be registered through the send, recv event and component methods in
/// the App.
///
/// For replication to work, it is also necessary to choose a transport plugin,
/// as the replication plugin is only reponsible for generating the updates.
pub struct ReplicationPlugin;

impl Plugin for ReplicationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Replication>();

        app.add_systems(PreUpdate, (spawn_new_connections, recv_updates))
            .add_systems(PostUpdate, send_updates);
    }
}

pub trait AppExt {
    // Sets up the event for sending, when new events arrive.
    fn send_event<E: Event + Serialize>(&mut self) -> &mut Self;

    // Sets up the event for receiving.
    fn recv_event<E: Event + DeserializeOwned>(&mut self) -> &mut Self;

    // Sets up the component for sending, when a component has been added, changed
    // or removed.
    fn send_component<C: Component + Serialize>(&mut self) -> &mut Self;

    // Sets up the component for receiving.
    fn recv_component<C: Component + DeserializeOwned>(&mut self) -> &mut Self;
}

impl AppExt for App {
    fn send_event<E: Event + Serialize>(&mut self) -> &mut Self {
        // Add event and get component id of the event collection.
        self.add_event::<E>();
        // SAFETY: Add event always adds the Events<E> resource.
        let component_id = unsafe {
            self.world
                .components()
                .resource_id::<Events<E>>()
                .unwrap_unchecked()
        };

        // Generate a hash to uniquly identitfy events of this type across instances.
        let mut hasher = DefaultHasher::new();
        type_name::<E>().hash(&mut hasher);
        let type_hash = hasher.finish();

        // Add serializer for the given component id.
        let mut replication = self.world.resource_mut::<Replication>();
        replication
            .event_serializers
            .insert(component_id, (type_hash, serialize_events::<E>));

        self
    }

    fn recv_event<E: Event + DeserializeOwned>(&mut self) -> &mut Self {
        // Add event. No component id is required as the event should be added to the
        // collection by the deserializer (it also acts as an handler).
        self.add_event::<E>();

        // Generate a hash to uniquly identitfy events of this type across instances.
        let mut hasher = DefaultHasher::new();
        type_name::<E>().hash(&mut hasher);
        let type_hash = hasher.finish();

        // Add deserializer (incl. handler) for the given component id.
        let mut replication = self.world.resource_mut::<Replication>();
        replication
            .event_deserializers
            .insert(type_hash, deserialize_and_send_events::<E>);

        self
    }

    fn send_component<C: Component + Serialize>(&mut self) -> &mut Self {
        // Add component and get component id.
        let component_id = self.world.init_component::<C>();

        // Generate a hash to uniquly identitfy events of this type across instances.
        let mut hasher = DefaultHasher::new();
        type_name::<C>().hash(&mut hasher);
        let type_hash = hasher.finish();

        // Add serializer for the given component id.
        let mut replication = self.world.resource_mut::<Replication>();
        replication
            .component_serializers
            .insert(component_id, (type_hash, serialize::<C>));

        self
    }

    fn recv_component<C: Component + DeserializeOwned>(&mut self) -> &mut Self {
        // Generate a hash to uniquly identitfy events of this type across instances.
        let mut hasher = DefaultHasher::new();
        type_name::<C>().hash(&mut hasher);
        let type_hash = hasher.finish();

        // Add deserializer (incl. handler) for the given component id.
        let mut replication = self.world.resource_mut::<Replication>();
        replication
            .component_deserializers
            .insert(type_hash, deserialize_and_insert_component::<C>);

        self
    }
}

fn serialize<T: Serialize>(value: Ptr, output: &mut Vec<u8>) {
    use bincode::{DefaultOptions, Options};

    // SAFETY: serialize is always supplied with the correct type (component ids are
    // unique).
    let value: &T = unsafe { value.deref() };
    DefaultOptions::new().serialize_into(output, value).unwrap();
}

fn serialize_events<E: Event + Serialize>(
    events: Ptr,
    output: &mut Vec<u8>,
    last_event_count: &mut usize,
) {
    use bincode::{DefaultOptions, Options};

    // SAFETY: serialize_events is always supplied with the correct type (component
    // ids are unique).
    let events: &Events<E> = unsafe { events.deref() };
    // SAFETY: event_reader is the same size as last_event_count.
    // TODO: find safe method to initialize ManualEventReader at the right
    // position.
    let event_reader: &mut ManualEventReader<E> = unsafe { std::mem::transmute(last_event_count) };
    let events = event_reader.read(events).collect::<Vec<_>>();
    if !events.is_empty() {
        DefaultOptions::new()
            .serialize_into(output, &events)
            .unwrap();
    }
}

fn deserialize_and_send_events<E: Event + DeserializeOwned>(world: &mut World, input: &mut &[u8]) {
    use bincode::{DefaultOptions, Options};

    let events: Vec<E> = DefaultOptions::new().deserialize_from(input).unwrap();
    world.send_event_batch(events);
}

fn deserialize_and_insert_component<C: Component + DeserializeOwned>(
    entity: &mut EntityWorldMut,
    input: &mut &[u8],
) {
    use bincode::{DefaultOptions, Options};

    let component: C = DefaultOptions::new().deserialize_from(input).unwrap();
    entity.insert(component);
}

/// Serializer, deserializers (incl. handlers) necessary for replication.
#[derive(Resource, Default)]
struct Replication {
    event_serializers: HashMap<ComponentId, (u64, fn(Ptr, &mut Vec<u8>, &mut usize))>,
    event_deserializers: HashMap<u64, fn(&mut World, &mut &[u8])>,
    component_serializers: HashMap<ComponentId, (u64, fn(Ptr, &mut Vec<u8>))>,
    component_deserializers: HashMap<u64, fn(&mut EntityWorldMut, &mut &[u8])>,
}

/// Receiver for receiving newly connected clients, which should be spawned.
#[derive(Resource)]
pub struct NewConnectionRx(mpsc::UnboundedReceiver<Connection>);

impl NewConnectionRx {
    /// Creates a "new connection" receiver.
    pub fn new(rx: mpsc::UnboundedReceiver<Connection>) -> Self {
        Self(rx)
    }
}

#[derive(Component)]
pub struct Connection {
    /// Inbound packets
    packet_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    /// Outbound packets
    packet_tx: mpsc::UnboundedSender<Vec<u8>>,

    /// All known remote entities
    entities: HashMap<u64, Entity>,
    /// All entities which have been added because of a component update. Needs
    /// to be sent to the remote, to create an association between those
    /// entities.
    entities_added: Vec<(u64, Entity)>,
}

impl Connection {
    /// Creates a new connection
    pub fn new(
        packet_rx: mpsc::UnboundedReceiver<Vec<u8>>,
        packet_tx: mpsc::UnboundedSender<Vec<u8>>,
    ) -> Self {
        Self {
            packet_rx,
            packet_tx,
            entities: Default::default(),
            entities_added: Default::default(),
        }
    }
}

/// Spawns new connections
fn spawn_new_connections(mut commands: Commands, mut new_connection_rx: ResMut<NewConnectionRx>) {
    while let Ok(connection) = new_connection_rx.0.try_recv() {
        commands.spawn(connection);
    }
}

/// Last send event count, needed to not send events multiple times.
#[derive(Default)]
struct EventReaders(HashMap<u64, usize>);

/// Gathers and sends all updates.
fn send_updates(
    change_tick: SystemChangeTick,
    mut event_readers: Local<EventReaders>,
    mut component_remove_event_readers: Local<
        HashMap<ComponentId, ManualEventReader<RemovedComponentEntity>>,
    >,
    world: &World,
    replication: Res<Replication>,
    connections: Query<&Connection>,
    component_remove_events: &RemovedComponentEvents,
) {
    for connection in connections.iter() {
        let mut update = Vec::new();

        // Write all added entities, which are needed for properly associating entities
        // between peers
        update
            .write_u8(connection.entities_added.len() as u8)
            .unwrap();
        for (remote_entity, local_entity) in &connection.entities_added {
            update.write_u64::<LittleEndian>(*remote_entity).unwrap();
            update
                .write_u64::<LittleEndian>(local_entity.to_bits())
                .unwrap();
        }

        // Write all modified components, go through archetypes as some are stored in
        // tables
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
                let Some((type_hash, serialize)) =
                    replication.component_serializers.get(&component_id)
                else {
                    // No serializer found, skip.
                    continue;
                };

                update.write_u64::<LittleEndian>(*type_hash).unwrap();

                let update_len = update.len();
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

                            update
                                .write_u64::<LittleEndian>(archetype_entity.id().to_bits())
                                .unwrap();
                            serialize(component, &mut update);
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

                            update
                                .write_u64::<LittleEndian>(archetype_entity.id().to_bits())
                                .unwrap();
                            serialize(component, &mut update);
                        }
                    }
                }

                // Check if any component was written.
                if update_len == update.len() {
                    // Drop type hash.
                    update.truncate(update_len - 8);
                } else {
                    // Null-terminated.
                    update.write_u64::<LittleEndian>(0).unwrap();
                }
            }
        }

        // Write all events.
        for (component_id, (type_hash, serialize)) in replication.event_serializers.iter() {
            update.write_u64::<LittleEndian>(*type_hash).unwrap();

            let resource_data = world.storages().resources.get(*component_id).unwrap();
            let resource = unsafe { resource_data.get_data().unwrap_unchecked() };

            let update_len = update.len();
            let last_event_count = event_readers.0.entry(*type_hash).or_default();
            serialize(resource, &mut update, last_event_count);

            // Check if resource is empty.
            if update_len == update.len() {
                // Drop type hash.
                update.truncate(update_len - 8);
            }
        }

        // Null-terminated.
        update.write_u64::<LittleEndian>(0).unwrap();

        for (component_id, _) in &replication.component_serializers {
            let Some(events) = component_remove_events.get(*component_id) else {
                continue;
            };
            let event_reader = component_remove_event_readers
                .entry(*component_id)
                .or_default();
        }

        // Skip if update is empty.
        // TODO: Remove magic number
        if update.len() == 9 {
            continue;
        }
        connection.packet_tx.send(update).unwrap();
    }
}

/// Processes all incoming updates.
fn recv_updates(world: &mut World) {
    let unsafe_world_cell = world.as_unsafe_world_cell();

    let replication = unsafe { unsafe_world_cell.get_resource::<Replication>() }.unwrap();
    let mut connections = unsafe { unsafe_world_cell.world_mut() }.query::<&mut Connection>();
    for mut connection in connections.iter_mut(unsafe { unsafe_world_cell.world_mut() }) {
        // Clear out entities added.
        connection.entities_added.clear();

        while let Ok(update) = connection.packet_rx.try_recv() {
            let mut update = update.as_slice();

            // Read entities added.
            for _ in 0..update.read_u8().unwrap() {
                let local_entity = update.read_u64::<LittleEndian>().unwrap();
                let remote_entity = update.read_u64::<LittleEndian>().unwrap();
                connection
                    .entities
                    .insert(remote_entity, Entity::from_bits(local_entity));
            }

            // Read data.
            let mut type_hash = update.read_u64::<LittleEndian>().unwrap();
            while type_hash != 0 {
                if let Some(deserialize) = replication.component_deserializers.get(&type_hash) {
                    let mut entity = update.read_u64::<LittleEndian>().unwrap();
                    while entity != 0 {
                        let mut entity_world = if let Some(entity) =
                            connection.entities.get(&entity)
                        {
                            unsafe { unsafe_world_cell.world_mut() }.entity_mut(*entity)
                        } else {
                            let entity_world = unsafe { unsafe_world_cell.world_mut() }.spawn(());
                            connection.entities.insert(entity, entity_world.id());
                            connection.entities_added.push((entity, entity_world.id()));
                            entity_world
                        };
                        deserialize(&mut entity_world, &mut update);

                        entity = update.read_u64::<LittleEndian>().unwrap();
                    }
                } else if let Some(deserialize) = replication.event_deserializers.get(&type_hash) {
                    deserialize(unsafe { unsafe_world_cell.world_mut() }, &mut update);
                }

                type_hash = update.read_u64::<LittleEndian>().unwrap();
            }
        }
    }
}
