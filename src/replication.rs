use std::{
    any::type_name,
    hash::{DefaultHasher, Hash, Hasher},
};

use bevy::{
    app::{App, Plugin},
    ecs::{
        component::{Component, ComponentId},
        entity::Entity,
        event::{Event, Events, ManualEventReader},
        system::Resource,
        world::{EntityWorldMut, World},
    },
    ptr::Ptr,
    utils::HashMap,
};
use serde::{de::DeserializeOwned, Serialize};

/// Allows for replication of components and events between multiple Bevy instances.
///
/// Replication is opt-in, components or events that need to be sent or received have to be
/// registered through the send, recv event and component methods in the App.
///
/// For replication to work, it is also necessary to choose a transport plugin, as the replication
/// plugin is only reponsible for generating the updates.
pub struct ReplicationPlugin;

impl Plugin for ReplicationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Replication>();
    }
}

pub trait AppExt {
    // Sets up resources for sending, when a resource has been added or changed.
    fn send_resource<R: Resource + Serialize>(&mut self) -> &mut Self;

    // Sets up resources for receiving.
    fn recv_resource<R: Resource + DeserializeOwned>(&mut self) -> &mut Self;

    // Sets up events for sending, when new events arrive.
    fn send_event<E: Event + Serialize>(&mut self) -> &mut Self;

    // Sets up events for receiving.
    fn recv_event<E: Event + DeserializeOwned>(&mut self) -> &mut Self;

    // Sets up components for sending, when a component has been added, changed or removed.
    fn send_component<C: Component + Serialize>(&mut self) -> &mut Self;

    // Sets up components for receiving.
    fn recv_component<C: Component + DeserializeOwned>(&mut self) -> &mut Self;
}

impl AppExt for App {
    fn send_resource<R: Resource + Serialize>(&mut self) -> &mut Self {
        // Add resource and get component id.
        let component_id = self.world.components().resource_id::<R>().unwrap();

        // Generate a hash to uniquly identitfy resources of this type across instances.
        let mut hasher = DefaultHasher::new();
        type_name::<R>().hash(&mut hasher);
        let type_hash = hasher.finish();
        let type_hash = type_hash as u32 ^ (type_hash >> 32) as u32;

        // Add serializer for the given component id.
        let mut replication = self.world.resource_mut::<Replication>();
        replication.send_resource.insert(
            component_id,
            SendResourceData {
                type_hash,
                serializer: serialize::<R>,
            },
        );

        self
    }

    fn recv_resource<R: Resource + DeserializeOwned>(&mut self) -> &mut Self {
        // Generate a hash to uniquly identitfy resources of this type across instances.
        let mut hasher = DefaultHasher::new();
        type_name::<R>().hash(&mut hasher);
        let type_hash = hasher.finish();
        let type_hash = type_hash as u32 ^ (type_hash >> 32) as u32;

        // Add deserializer (incl. handler) for the given resource id.
        let mut replication = self.world.resource_mut::<Replication>();
        replication
            .recv_resource
            .insert(type_hash, deserialize_and_insert_resource::<R>);

        self
    }

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
        let type_hash = type_hash as u32 ^ (type_hash >> 32) as u32;

        // Add serializer for the given component id.
        let mut replication = self.world.resource_mut::<Replication>();
        replication.send_event.insert(
            component_id,
            SendEventData {
                type_hash,
                serializer: serialize_events::<E>,
            },
        );

        self
    }

    fn recv_event<E: Event + DeserializeOwned>(&mut self) -> &mut Self {
        // Add event. No component id is required as the event should be added to the collection by
        // the deserializer (it also acts as an handler).
        self.add_event::<Received<E>>();

        // Generate a hash to uniquly identitfy events of this type across instances.
        let mut hasher = DefaultHasher::new();
        type_name::<E>().hash(&mut hasher);
        let type_hash = hasher.finish();
        let type_hash = type_hash as u32 ^ (type_hash >> 32) as u32;

        // Add deserializer (incl. handler) for the given component id.
        let mut replication = self.world.resource_mut::<Replication>();
        replication
            .recv_event
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
        let type_hash = type_hash as u32 ^ (type_hash >> 32) as u32;

        // Add serializer for the given component id.
        let mut replication = self.world.resource_mut::<Replication>();
        replication.send_component.insert(
            component_id,
            SendComponentData {
                type_hash,
                serializer: serialize::<C>,
            },
        );

        self
    }

    fn recv_component<C: Component + DeserializeOwned>(&mut self) -> &mut Self {
        // Generate a hash to uniquly identitfy events of this type across instances.
        let mut hasher = DefaultHasher::new();
        type_name::<C>().hash(&mut hasher);
        let type_hash = hasher.finish();
        let type_hash = type_hash as u32 ^ (type_hash >> 32) as u32;

        // Add deserializer (incl. handler) for the given component id.
        let mut replication = self.world.resource_mut::<Replication>();
        replication.recv_component.insert(
            type_hash,
            (deserialize_and_insert_component::<C>, remove_component::<C>),
        );

        self
    }
}

#[derive(Resource, Default)]
pub(crate) struct Replication {
    pub(crate) send_resource: HashMap<ComponentId, SendResourceData>,
    pub(crate) recv_resource: HashMap<u32, ResourceDeserializer>,
    pub(crate) send_event: HashMap<ComponentId, SendEventData>,
    pub(crate) recv_event: HashMap<u32, EventDeserializer>,
    pub(crate) send_component: HashMap<ComponentId, SendComponentData>,
    pub(crate) recv_component: HashMap<u32, (ComponentDeserializer, ComponentRemover)>,
}

pub(crate) type ResourceSerializer = fn(Ptr, &mut Vec<u8>);

pub(crate) struct SendResourceData {
    pub(crate) type_hash: u32,
    pub(crate) serializer: ResourceSerializer,
}

pub(crate) type ResourceDeserializer = fn(&mut World, &mut &[u8]);

pub(crate) type EventSerializer = fn(Ptr, &mut Vec<u8>, &mut usize);

pub(crate) struct SendEventData {
    pub(crate) type_hash: u32,
    pub(crate) serializer: EventSerializer,
}

pub(crate) type EventDeserializer = fn(&mut World, &mut &[u8], Entity);

pub(crate) type ComponentSerializer = fn(Ptr, &mut Vec<u8>);

pub(crate) struct SendComponentData {
    pub(crate) type_hash: u32,
    pub(crate) serializer: ComponentSerializer,
}

pub(crate) type ComponentDeserializer = fn(&mut EntityWorldMut, &mut &[u8]);

pub(crate) type ComponentRemover = fn(&mut EntityWorldMut);

pub fn serialize<T: Serialize>(value: Ptr, output: &mut Vec<u8>) {
    use bincode::{DefaultOptions, Options};

    // SAFETY: serialize is always supplied with the correct type (component ids are unique).
    let value: &T = unsafe { value.deref() };
    DefaultOptions::new().serialize_into(output, value).unwrap();
}

pub fn serialize_events<E: Event + Serialize>(
    events: Ptr,
    output: &mut Vec<u8>,
    last_event_count: &mut usize,
) {
    use bincode::{DefaultOptions, Options};

    // SAFETY: serialize_events is always supplied with the correct type (component ids are unique).
    let events: &Events<E> = unsafe { events.deref() };
    // SAFETY: event_reader is the same size as last_event_count.
    let event_reader: &mut ManualEventReader<E> = unsafe { std::mem::transmute(last_event_count) };
    let events = event_reader.read(events).collect::<Vec<_>>();
    if !events.is_empty() {
        DefaultOptions::new()
            .serialize_into(output, &events)
            .unwrap();
    }
}

pub fn deserialize_and_insert_resource<R: Resource + DeserializeOwned>(
    world: &mut World,
    input: &mut &[u8],
) {
    use bincode::{DefaultOptions, Options};

    let resource: R = DefaultOptions::new().deserialize_from(input).unwrap();
    world.insert_resource(resource);
}

pub fn deserialize_and_send_events<E: Event + DeserializeOwned>(
    world: &mut World,
    input: &mut &[u8],
    source: Entity,
) {
    use bincode::{DefaultOptions, Options};

    let events: Vec<E> = DefaultOptions::new().deserialize_from(input).unwrap();
    world.send_event_batch(events.into_iter().map(|event| Received { source, event }));
}

pub fn deserialize_and_insert_component<C: Component + DeserializeOwned>(
    entity: &mut EntityWorldMut,
    input: &mut &[u8],
) {
    use bincode::{DefaultOptions, Options};

    let component: C = DefaultOptions::new().deserialize_from(input).unwrap();
    entity.insert(component);
}

pub fn remove_component<C: Component>(entity: &mut EntityWorldMut) {
    entity.remove::<C>();
}

#[derive(Event)]
pub struct Received<E: Event> {
    pub source: Entity,
    pub event: E,
}

#[derive(Event)]
pub struct Directed<E: Event> {
    pub target: Entity,
    pub event: E,
}
