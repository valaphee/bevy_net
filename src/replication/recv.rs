use bevy::ecs::{entity::Entity, world::World};
use byteorder::{LittleEndian, ReadBytesExt};

use super::{Connection, Replication};

/// Processes all incoming updates.
pub(super) fn recv_updates(world: &mut World) {
    let unsafe_world_cell = world.as_unsafe_world_cell();

    let replication = unsafe { unsafe_world_cell.get_resource::<Replication>() }.unwrap();
    let mut connections = unsafe { unsafe_world_cell.world_mut() }.query::<&mut Connection>();
    for mut connection in connections.iter_mut(unsafe { unsafe_world_cell.world_mut() }) {
        // Clear out new entity links.
        connection.entity_links_new.clear();

        while let Ok(update) = connection.packet_rx.try_recv() {
            let mut update = update.as_slice();

            // Read all entity links.
            for _ in 0..update.read_u8().unwrap() {
                let local_entity = update.read_u32::<LittleEndian>().unwrap();
                let remote_entity = update.read_u32::<LittleEndian>().unwrap();
                connection
                    .entity_links
                    .insert(remote_entity, Entity::from_raw(local_entity));
            }

            // Read all events.
            let mut type_hash = update.read_u32::<LittleEndian>().unwrap();
            while type_hash != 0 {
                if let Some(deserializer) = replication.event_deserializers.get(&type_hash) {
                    deserializer(unsafe { unsafe_world_cell.world_mut() }, &mut update);
                } else if let Some((_, remover)) = replication
                    .component_deserializers_and_removers
                    .get(&type_hash)
                {
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
                type_hash = update.read_u32::<LittleEndian>().unwrap();
            }

            // Read all components added/updated.
            let mut type_hash = update.read_u32::<LittleEndian>().unwrap();
            while type_hash != 0 {
                if let Some((deserializer, _)) = replication
                    .component_deserializers_and_removers
                    .get(&type_hash)
                {
                    let mut entity = update.read_u32::<LittleEndian>().unwrap();
                    while entity != 0 {
                        let mut entity_world = if let Some(entity) =
                            connection.entity_links.get(&entity)
                        {
                            unsafe { unsafe_world_cell.world_mut() }.entity_mut(*entity)
                        } else {
                            let entity_world = unsafe { unsafe_world_cell.world_mut() }.spawn(());
                            connection.entity_links.insert(entity, entity_world.id());
                            connection
                                .entity_links_new
                                .push((entity, entity_world.id()));
                            entity_world
                        };
                        deserializer(&mut entity_world, &mut update);

                        entity = update.read_u32::<LittleEndian>().unwrap();
                    }
                }
                type_hash = update.read_u32::<LittleEndian>().unwrap();
            }
        }
    }
}
