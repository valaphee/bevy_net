use std::{any::type_name, hash::{DefaultHasher, Hash, Hasher}};

use bevy::{app::App, ecs::{component::{Component, ComponentId, StorageType}, entity::Entity, event::{Event, Events}, system::{Query, Res, Resource, SystemChangeTick}, world::{EntityWorldMut, World}}, ptr::Ptr, utils::HashMap};
use bincode::{DefaultOptions, Options};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use serde::{de::DeserializeOwned, Serialize};

use crate::transport::Connection;

#[derive(Resource, Default)]
pub(crate) struct Registry {
    resource_serializers: HashMap<ComponentId, (u64, fn(Ptr, &mut Vec<u8>))>,
    resource_deserializers: HashMap<u64, fn(&mut World, &mut &[u8])>,

    component_serializers: HashMap<ComponentId, (u64, fn(Ptr, &mut Vec<u8>))>,
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

        let mut registry = self.world.resource_mut::<Registry>();
        registry.resource_serializers.insert(component_id, (type_hash, serialize_events::<E>));

        self
    }

    fn recv_event<E: Event + DeserializeOwned>(&mut self) -> &mut Self {
        self.add_event::<E>();

        let mut hasher = DefaultHasher::new();
        type_name::<E>().hash(&mut hasher);
        let type_hash = hasher.finish();

        let mut registry = self.world.resource_mut::<Registry>();
        registry.resource_deserializers.insert(type_hash, deserialize_and_send_events::<E>);

        self
    }

    fn send_component<C: Component + Serialize>(&mut self) -> &mut Self {
        let component_id = self.world.init_component::<C>();

        let mut hasher = DefaultHasher::new();
        type_name::<C>().hash(&mut hasher);
        let type_hash = hasher.finish();

        let mut registry = self.world.resource_mut::<Registry>();
        registry.component_serializers.insert(component_id, (type_hash, serialize::<C>));

        self
    }

    fn recv_component<C: Component + DeserializeOwned>(&mut self) -> &mut Self {
        let mut hasher = DefaultHasher::new();
        type_name::<C>().hash(&mut hasher);
        let type_hash = hasher.finish();

        let mut registry = self.world.resource_mut::<Registry>();
        registry.component_deserializers.insert(type_hash, deserialize_and_insert_component::<C>);

        self
    }
}

fn serialize<T: Serialize>(
    value: Ptr,
    output: &mut Vec<u8>,
) {
    let value: &T = unsafe { value.deref() };
    DefaultOptions::new().serialize_into(output, value).unwrap();
}

fn serialize_events<E: Event + Serialize>(
    events: Ptr,
    output: &mut Vec<u8>,
) {
    let events: &Events<E> = unsafe { events.deref() };
    let mut event_reader = events.get_reader();
    let events = event_reader.read(events).collect::<Vec<_>>();
    if !events.is_empty() {
        DefaultOptions::new().serialize_into(output, &events).unwrap();
    }
}

fn deserialize_and_send_events<E: Event + DeserializeOwned>(
    world: &mut World,
    mut input: &mut &[u8],
) {
    let events: Vec<E> = DefaultOptions::new().deserialize_from(&mut input).unwrap();
    world.send_event_batch(events);
}

fn deserialize_and_insert_component<C: Component + DeserializeOwned>(
    entity: &mut EntityWorldMut,
    mut input: &mut &[u8],
) {
    let component: C = DefaultOptions::new().deserialize_from(&mut input).unwrap();
    entity.insert(component);
}

pub(crate) fn send_update(
    world: &World,
    change_tick: SystemChangeTick,
    registry: Res<Registry>,
    connections: Query<&Connection>,
) {
    for connection in connections.iter() {
        let mut update = Vec::new();
        let mut should_update = false;

        for archetype in world.archetypes().iter() {
            let table = unsafe { world.storages().tables.get(archetype.table_id()).unwrap_unchecked() };
            for component_id in archetype.components() {
                let Some((type_hash, serialize)) = registry.component_serializers.get(&component_id) else {
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

                            println!("Entity {type_hash} {}", archetype_entity.id().to_bits());
                            update.write_u64::<LittleEndian>(archetype_entity.id().to_bits()).unwrap();
                            serialize(component, &mut update);
                            should_update = true;
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

                            println!("Entity {type_hash} {}", archetype_entity.id().to_bits());
                            update.write_u64::<LittleEndian>(archetype_entity.id().to_bits()).unwrap();
                            serialize(component, &mut update);
                            should_update = true;
                        }
                        update.write_u64::<LittleEndian>(0).unwrap();
                    },
                }
            }
        }
        for (component_id, (type_hash, serialize)) in registry.resource_serializers.iter() {
            let resource_data = world.storages().resources.get(*component_id).unwrap();
            let resource = unsafe { resource_data.get_data().unwrap_unchecked() };
            /*let ticks = unsafe { resource_data.get_ticks().unwrap_unchecked() };
            if !ticks.is_added(change_tick.last_run(), change_tick.this_run()) || !ticks.is_changed(change_tick.last_run(), change_tick.this_run()) {
                continue;
            }*/

            update.write_u64::<LittleEndian>(*type_hash).unwrap();
            let update_before = update.len();
            serialize(resource, &mut update);
            if update_before != update.len() {
                println!("Event type hash {type_hash}");
                should_update = true;
            } else {
                unsafe { update.set_len(update_before - 8) };
            }
        }
        update.write_u64::<LittleEndian>(0).unwrap();

        update.write_u8(connection.added_entities.len() as u8).unwrap();
        for (remote_entity, local_entity) in &connection.added_entities {
            update.write_u64::<LittleEndian>(*remote_entity).unwrap();
            update.write_u64::<LittleEndian>(local_entity.to_bits()).unwrap();
            println!("Add {remote_entity} {}", local_entity.to_bits());
            should_update = true;
        }
   
        if should_update {
            println!("Send {} bytes", update.len());
            connection.message_tx.send(update).unwrap();
        }
    }
}

pub(crate) fn recv_update(
    world: &mut World,
) {
    let unsafe_world_cell = world.as_unsafe_world_cell();

    let registry = unsafe { unsafe_world_cell.get_resource::<Registry>() }.unwrap();
    let mut connections = unsafe { unsafe_world_cell.world_mut() }.query::<&mut Connection>();
    for mut connection in connections.iter_mut(unsafe { unsafe_world_cell.world_mut() }) {
        connection.added_entities.clear();

        while let Ok(update) = connection.message_rx.try_recv() {
            let mut update = update.as_slice();
            println!("Recv {} bytes", update.len());

            let mut type_hash = update.read_u64::<LittleEndian>().unwrap();
            println!("Recv type hash {type_hash}");
            while type_hash != 0 {
                if let Some(deserialize) = registry.component_deserializers.get(&type_hash) {
                    let mut entity = update.read_u64::<LittleEndian>().unwrap();
                    println!("Recv entity {entity}");
                    while entity != 0 {
                        let mut entity_world = if let Some(entity) = connection.entity_map.get(&entity) {
                            unsafe { unsafe_world_cell.world_mut() }.entity_mut(*entity)
                        } else {
                            let entity_world = unsafe { unsafe_world_cell.world_mut() }.spawn(());
                            connection.entity_map.insert(entity, entity_world.id());
                            connection.added_entities.push((entity, entity_world.id()));
                            entity_world
                        };
                        deserialize(&mut entity_world, &mut update);

                        entity = update.read_u64::<LittleEndian>().unwrap();
                    }
                } else if let Some(deserialize) = registry.resource_deserializers.get(&type_hash) {
                    println!("Recv event");
                    deserialize(unsafe { unsafe_world_cell.world_mut() }, &mut update);
                }

                type_hash = update.read_u64::<LittleEndian>().unwrap();
            }

            for _ in 0..update.read_u8().unwrap() {
                let local_entity = update.read_u64::<LittleEndian>().unwrap();
                let remote_entity = update.read_u64::<LittleEndian>().unwrap();
                println!("Recv add entity {local_entity} {remote_entity}");
                connection.entity_map.insert(remote_entity, Entity::from_bits(local_entity));
            }
        }
    }
}
