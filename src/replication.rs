use std::{any::type_name, hash::{DefaultHasher, Hash, Hasher}};

use bevy::{app::App, ecs::{component::{Component, ComponentId, StorageType}, entity::Entity, event::{Event, Events}, system::{Commands, Query, Res, ResMut, Resource, SystemChangeTick}, world::{EntityWorldMut, World}}, ptr::Ptr, utils::HashMap};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::mpsc;

#[derive(Resource, Default)]
pub struct Replication {
    resource_serializers: HashMap<ComponentId, (u64, fn(Ptr, &mut Vec<u8>))>,
    component_serializers: HashMap<ComponentId, (u64, fn(Ptr, &mut Vec<u8>))>,
    resource_deserializers: HashMap<u64, fn(&mut World, &mut &[u8])>,
    component_deserializers: HashMap<u64, fn(&mut EntityWorldMut, &mut &[u8])>,
}

pub trait AppExt {
    fn send_event<E: Event + Serialize>(&mut self) -> &mut Self;

    fn recv_event<E: Event + DeserializeOwned>(&mut self) -> &mut Self;

    fn send_component<C: Component + Serialize>(&mut self) -> &mut Self;

    fn recv_component<C: Component + DeserializeOwned>(&mut self) -> &mut Self;
}

impl AppExt for App {
    fn send_event<E: Event + Serialize>(&mut self) -> &mut Self {
        self.add_event::<E>();
        let component_id = unsafe { self.world.components().resource_id::<Events<E>>().unwrap_unchecked() };

        let mut hasher = DefaultHasher::new();
        type_name::<E>().hash(&mut hasher);
        let type_hash = hasher.finish();

        let mut replication = self.world.resource_mut::<Replication>();
        replication.resource_serializers.insert(component_id, (type_hash, serialize_events::<E>));

        self
    }

    fn recv_event<E: Event + DeserializeOwned>(&mut self) -> &mut Self {
        self.add_event::<E>();

        let mut hasher = DefaultHasher::new();
        type_name::<E>().hash(&mut hasher);
        let type_hash = hasher.finish();

        let mut replication = self.world.resource_mut::<Replication>();
        replication.resource_deserializers.insert(type_hash, deserialize_and_send_events::<E>);

        self
    }

    fn send_component<C: Component + Serialize>(&mut self) -> &mut Self {
        let component_id = self.world.init_component::<C>();

        let mut hasher = DefaultHasher::new();
        type_name::<C>().hash(&mut hasher);
        let type_hash = hasher.finish();

        let mut replication = self.world.resource_mut::<Replication>();
        replication.component_serializers.insert(component_id, (type_hash, serialize::<C>));

        self
    }

    fn recv_component<C: Component + DeserializeOwned>(&mut self) -> &mut Self {
        let mut hasher = DefaultHasher::new();
        type_name::<C>().hash(&mut hasher);
        let type_hash = hasher.finish();

        let mut replication = self.world.resource_mut::<Replication>();
        replication.component_deserializers.insert(type_hash, deserialize_and_insert_component::<C>);

        self
    }
}

pub fn serialize<T: Serialize>(
    value: Ptr,
    output: &mut Vec<u8>,
) {
    use bincode::{DefaultOptions, Options};

    let value: &T = unsafe { value.deref() };
    DefaultOptions::new().serialize_into(output, value).unwrap();
}

pub fn serialize_events<E: Event + Serialize>(
    events: Ptr,
    output: &mut Vec<u8>,
) {
    use bincode::{DefaultOptions, Options};

    let events: &Events<E> = unsafe { events.deref() };
    let mut event_reader = events.get_reader();
    let events = event_reader.read(events).collect::<Vec<_>>();
    if !events.is_empty() {
        DefaultOptions::new().serialize_into(output, &events).unwrap();
    }
}

pub fn deserialize_and_send_events<E: Event + DeserializeOwned>(
    world: &mut World,
    mut input: &mut &[u8],
) {
    use bincode::{DefaultOptions, Options};

    let events: Vec<E> = DefaultOptions::new().deserialize_from(&mut input).unwrap();
    world.send_event_batch(events);
}

pub fn deserialize_and_insert_component<C: Component + DeserializeOwned>(
    entity: &mut EntityWorldMut,
    mut input: &mut &[u8],
) {
    use bincode::{DefaultOptions, Options};

    let component: C = DefaultOptions::new().deserialize_from(&mut input).unwrap();
    entity.insert(component);
}

#[derive(Resource)]
pub struct NewConnectionRx(pub(crate) mpsc::UnboundedReceiver<Connection>);

#[derive(Component)]
pub struct Connection {
    pub(crate) message_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pub(crate) message_tx: mpsc::UnboundedSender<Vec<u8>>,

    pub(crate) entities: HashMap<u64, Entity>,
    pub(crate) entities_added: Vec<(u64, Entity)>,
}

pub(crate) fn spawn_new_connections(
    mut commands: Commands,
    mut new_connection_rx: ResMut<NewConnectionRx>
) {
    while let Ok(connection) = new_connection_rx.0.try_recv() {
        commands.spawn(connection);
    }
}

pub(crate) fn send_updates(
    world: &World,
    change_tick: SystemChangeTick,
    replication: Res<Replication>,
    connections: Query<&Connection>,
) {
    for connection in connections.iter() {
        let mut update = Vec::new();

        for archetype in world.archetypes().iter() {
            let table = unsafe { world.storages().tables.get(archetype.table_id()).unwrap_unchecked() };
            for component_id in archetype.components() {
                let Some((type_hash, serialize)) = replication.component_serializers.get(&component_id) else {
                    continue;
                };
                update.write_u64::<LittleEndian>(*type_hash).unwrap();

                let storage_type = unsafe { archetype.get_storage_type(component_id).unwrap_unchecked() };
                match storage_type {
                    StorageType::Table => {
                        let column = unsafe { table.get_column(component_id).unwrap_unchecked() };
                        for archetype_entity in archetype.entities() {
                            let component = unsafe { column.get_data_unchecked(archetype_entity.table_row()) };
                            let ticks = unsafe { column.get_ticks_unchecked(archetype_entity.table_row()) };
                            if !ticks.is_added(change_tick.last_run(), change_tick.this_run()) || !ticks.is_changed(change_tick.last_run(), change_tick.this_run()) {
                                continue;
                            }

                            update.write_u64::<LittleEndian>(archetype_entity.id().to_bits()).unwrap();
                            serialize(component, &mut update);
                        }
                        update.write_u64::<LittleEndian>(0).unwrap();
                    },
                    StorageType::SparseSet => {
                        let sparse_set = unsafe { world.storages().sparse_sets.get(component_id).unwrap_unchecked() };
                        for archetype_entity in archetype.entities() {
                            let component = unsafe { sparse_set.get(archetype_entity.id()).unwrap_unchecked() };
                            let ticks = unsafe { sparse_set.get_ticks(archetype_entity.id()).unwrap_unchecked() };
                            if !ticks.is_added(change_tick.last_run(), change_tick.this_run()) || !ticks.is_changed(change_tick.last_run(), change_tick.this_run()) {
                                continue;
                            }

                            update.write_u64::<LittleEndian>(archetype_entity.id().to_bits()).unwrap();
                            serialize(component, &mut update);
                        }
                        update.write_u64::<LittleEndian>(0).unwrap();
                    },
                }
            }
        }
        for (component_id, (type_hash, serialize)) in replication.resource_serializers.iter() {
            let resource_data = world.storages().resources.get(*component_id).unwrap();
            let resource = unsafe { resource_data.get_data().unwrap_unchecked() };
            /*let ticks = unsafe { resource_data.get_ticks().unwrap_unchecked() };
            if !ticks.is_added(change_tick.last_run(), change_tick.this_run()) || !ticks.is_changed(change_tick.last_run(), change_tick.this_run()) {
                continue;
            }*/

            update.write_u64::<LittleEndian>(*type_hash).unwrap();
            let update_before = update.len();
            serialize(resource, &mut update);
            if update_before == update.len() {
                unsafe { update.set_len(update_before - 8) };
            }
        }
        update.write_u64::<LittleEndian>(0).unwrap();

        update.write_u8(connection.entities_added.len() as u8).unwrap();
        for (remote_entity, local_entity) in &connection.entities_added {
            update.write_u64::<LittleEndian>(*remote_entity).unwrap();
            update.write_u64::<LittleEndian>(local_entity.to_bits()).unwrap();
        }

        if update.len() == 9 {
            continue;
        }
        connection.message_tx.send(update).unwrap();
    }
}

pub(crate) fn recv_updates(
    world: &mut World,
) {
    let unsafe_world_cell = world.as_unsafe_world_cell();

    let replication = unsafe { unsafe_world_cell.get_resource::<Replication>() }.unwrap();
    let mut connections = unsafe { unsafe_world_cell.world_mut() }.query::<&mut Connection>();
    for mut connection in connections.iter_mut(unsafe { unsafe_world_cell.world_mut() }) {
        connection.entities_added.clear();

        while let Ok(update) = connection.message_rx.try_recv() {
            let mut update = update.as_slice();

            let mut type_hash = update.read_u64::<LittleEndian>().unwrap();
            while type_hash != 0 {
                if let Some(deserialize) = replication.component_deserializers.get(&type_hash) {
                    let mut entity = update.read_u64::<LittleEndian>().unwrap();
                    while entity != 0 {
                        let mut entity_world = if let Some(entity) = connection.entities.get(&entity) {
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
                } else if let Some(deserialize) = replication.resource_deserializers.get(&type_hash) {
                    deserialize(unsafe { unsafe_world_cell.world_mut() }, &mut update);
                }

                type_hash = update.read_u64::<LittleEndian>().unwrap();
            }

            for _ in 0..update.read_u8().unwrap() {
                let local_entity = update.read_u64::<LittleEndian>().unwrap();
                let remote_entity = update.read_u64::<LittleEndian>().unwrap();
                connection.entities.insert(remote_entity, Entity::from_bits(local_entity));
            }
        }
    }
}
