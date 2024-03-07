use bevy::{
    ecs::{
        component::{ComponentId, StorageType},
        entity::Entity,
        event::ManualEventReader,
        removal_detection::{RemovedComponentEntity, RemovedComponentEvents},
        system::{Local, Query, Res, SystemChangeTick},
        world::World,
    },
    utils::HashMap,
};
use byteorder::{LittleEndian, WriteBytesExt};

use super::{Connection, Replication};

/// Last send event count, needed to not send events multiple times.
#[derive(Default)]
pub(super) struct EventReaders(HashMap<u32, usize>);

/// Gathers and sends all updates.
pub(super) fn send_updates(
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

        // Write all entity links.
        update
            .write_u8(connection.entity_links_new.len() as u8)
            .unwrap();
        for (remote_entity, local_entity) in &connection.entity_links_new {
            update.write_u32::<LittleEndian>(*remote_entity).unwrap();
            update
                .write_u32::<LittleEndian>(local_entity.index())
                .unwrap();
        }

        // Write all events.
        for (component_id, send_event_data) in replication.send_event_data.iter() {
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
        for (component_id, send_component_data) in &replication.send_component_data {
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

        // Null-terminated.
        update.write_u32::<LittleEndian>(0).unwrap();

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
                let Some(send_component_data) = replication.send_component_data.get(&component_id)
                else {
                    // No component data found, skip.
                    continue;
                };

                update
                    .write_u32::<LittleEndian>(send_component_data.type_hash)
                    .unwrap();
                let update_pos = update.len();

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
                                .write_u32::<LittleEndian>(archetype_entity.id().index())
                                .unwrap();
                            (send_component_data.serializer)(component, &mut update);
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
                                .write_u32::<LittleEndian>(archetype_entity.id().index())
                                .unwrap();
                            (send_component_data.serializer)(component, &mut update);
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
        connection.packet_tx.send(update).unwrap();
    }
}
