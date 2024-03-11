use std::{
    any::type_name,
    hash::{DefaultHasher, Hash, Hasher},
};

use bevy::{
    app::{App, Plugin, PostUpdate, PreUpdate},
    ecs::{
        component::{Component, ComponentId},
        entity::Entity,
        event::{Event, Events, ManualEventReader},
        system::{Commands, ResMut, Resource},
        world::{EntityWorldMut, World},
    },
    ptr::Ptr,
    utils::HashMap,
};
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::mpsc;

use self::{recv::recv_updates, send::send_updates};

mod recv;
mod send;

#[derive(Event)]
pub struct Received<E: Event> {
    pub source: Entity,
    pub event: E,
}

pub struct Directed<E: Event> {
    pub target: Entity,
    pub event: E,
}

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
    fn send_resource<R: Resource + Serialize>(&mut self) -> &mut Self;

    fn recv_resource<R: Resource + DeserializeOwned>(&mut self) -> &mut Self;

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
        replication.send_resource_data.insert(
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
        replication.resource_deserializers.insert(
            type_hash,
            deserialize_and_insert_resource::<R>,
        );

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
        replication.send_event_data.insert(
            component_id,
            SendEventData {
                type_hash,
                serializer: serialize_events::<E>,
            },
        );

        self
    }

    fn recv_event<E: Event + DeserializeOwned>(&mut self) -> &mut Self {
        // Add event. No component id is required as the event should be added to the
        // collection by the deserializer (it also acts as an handler).
        self.add_event::<Received<E>>();

        // Generate a hash to uniquly identitfy events of this type across instances.
        let mut hasher = DefaultHasher::new();
        type_name::<E>().hash(&mut hasher);
        let type_hash = hasher.finish();
        let type_hash = type_hash as u32 ^ (type_hash >> 32) as u32;

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
        let type_hash = type_hash as u32 ^ (type_hash >> 32) as u32;

        // Add serializer for the given component id.
        let mut replication = self.world.resource_mut::<Replication>();
        replication.send_component_data.insert(
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
        replication.component_deserializers_and_removers.insert(
            type_hash,
            (deserialize_and_insert_component::<C>, remove_component::<C>),
        );

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

fn deserialize_and_insert_resource<R: Resource + DeserializeOwned>(
    world: &mut World,
    input: &mut &[u8],
) {
    use bincode::{DefaultOptions, Options};

    let resource: R = DefaultOptions::new().deserialize_from(input).unwrap();
    world.insert_resource(resource);
}

fn deserialize_and_send_events<E: Event + DeserializeOwned>(world: &mut World, input: &mut &[u8], source: Entity) {
    use bincode::{DefaultOptions, Options};

    let events: Vec<E> = DefaultOptions::new().deserialize_from(input).unwrap();
    world.send_event_batch(events.into_iter().map(|event| {
        Received {
            source,
            event,
        }
    }));
}

fn deserialize_and_insert_component<C: Component + DeserializeOwned>(
    entity: &mut EntityWorldMut,
    input: &mut &[u8],
) {
    use bincode::{DefaultOptions, Options};

    let component: C = DefaultOptions::new().deserialize_from(input).unwrap();
    entity.insert(component);
}

fn remove_component<C: Component>(entity: &mut EntityWorldMut) {
    entity.remove::<C>();
}

#[derive(Resource, Default)]
struct Replication {
    send_resource_data: HashMap<ComponentId, SendResourceData>,
    send_event_data: HashMap<ComponentId, SendEventData>,
    send_component_data: HashMap<ComponentId, SendComponentData>,
    resource_deserializers: HashMap<u32, ResourceDeserializer>,
    event_deserializers: HashMap<u32, EventDeserializer>,
    component_deserializers_and_removers: HashMap<u32, (ComponentDeserializer, ComponentRemover)>,
}

type ResourceSerializer = fn(Ptr, &mut Vec<u8>);

type ResourceDeserializer = fn(&mut World, &mut &[u8]);

type EventSerializer = fn(Ptr, &mut Vec<u8>, &mut usize);

type EventDeserializer = fn(&mut World, &mut &[u8], Entity);

type ComponentSerializer = fn(Ptr, &mut Vec<u8>);

type ComponentDeserializer = fn(&mut EntityWorldMut, &mut &[u8]);

type ComponentRemover = fn(&mut EntityWorldMut);

struct SendResourceData {
    type_hash: u32,
    serializer: ResourceSerializer,
}

struct SendEventData {
    type_hash: u32,
    serializer: EventSerializer,
}

struct SendComponentData {
    type_hash: u32,
    serializer: ComponentSerializer,
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
    entity_links: HashMap<u32, Entity>,
    /// All entities which have been added because of a component update. Needs
    /// to be sent to the remote, to create an association between those
    /// entities.
    entity_links_new: Vec<(u32, Entity)>,
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
            entity_links: Default::default(),
            entity_links_new: Default::default(),
        }
    }
}

/// Spawns new connections
fn spawn_new_connections(mut commands: Commands, mut new_connection_rx: ResMut<NewConnectionRx>) {
    while let Ok(connection) = new_connection_rx.0.try_recv() {
        commands.spawn(connection);
    }
}
