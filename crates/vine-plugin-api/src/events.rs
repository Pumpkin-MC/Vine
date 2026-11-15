//! Typed event subscription API for Vine plugins.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};

pub use crate::wit::vine::plugin::event::{
    Event, EventType, PlayerChatEvent, PlayerCommandEvent, PlayerDisconnectEvent, PlayerJoinEvent,
    PlayerLoginEvent, PlayerPreLoginEvent, PluginMessageEvent, ProxyPingEvent, ServerConnectEvent,
    ServerConnectedEvent,
};
pub use crate::wit::vine::plugin::types::EventPriority;

use crate::Context;

pub(crate) static NEXT_EVENT_HANDLER_ID: AtomicU32 = AtomicU32::new(0);
pub(crate) static EVENT_HANDLERS: Mutex<BTreeMap<u32, Arc<dyn ErasedEventHandler>>> =
    Mutex::new(BTreeMap::new());

/// Connects an event marker type to its WIT [`EventType`] discriminant and payload record.
pub trait FromIntoEvent: Sized {
    /// Host event discriminant.
    const EVENT_TYPE: EventType;

    /// Concrete event record carried by this event.
    type Data;

    /// Extracts the typed payload from the [`Event`] variant, or `None` on mismatch.
    fn try_from_event(event: Event) -> Option<Self::Data>;

    /// Wraps the typed payload back into the [`Event`] variant.
    fn into_event(data: Self::Data) -> Event;
}

macro_rules! define_event_marker {
    ($marker:ident, $doc:expr, $variant:ident, $data:ty) => {
        #[doc = $doc]
        pub struct $marker;

        impl FromIntoEvent for $marker {
            const EVENT_TYPE: EventType = EventType::$variant;
            type Data = $data;

            fn try_from_event(event: Event) -> Option<Self::Data> {
                match event {
                    Event::$variant(data) => Some(data),
                    _ => None,
                }
            }

            fn into_event(data: Self::Data) -> Event {
                Event::$variant(data)
            }
        }
    };
}

define_event_marker!(
    ProxyPing,
    "Server list status ping event (`ProxyPingEvent`).",
    ProxyPing,
    ProxyPingEvent
);
define_event_marker!(
    PlayerPreLogin,
    "Initial login start handshake event before authentication (`PlayerPreLoginEvent`).",
    PlayerPreLogin,
    PlayerPreLoginEvent
);
define_event_marker!(
    PlayerLogin,
    "Post-authentication player login event (`PlayerLoginEvent`).",
    PlayerLogin,
    PlayerLoginEvent
);
define_event_marker!(
    PlayerJoin,
    "Event fired when a player joins their initial backend server (`PlayerJoinEvent`).",
    PlayerJoin,
    PlayerJoinEvent
);
define_event_marker!(
    ServerConnect,
    "Event fired before connecting or switching a player to a backend server (`ServerConnectEvent`).",
    ServerConnect,
    ServerConnectEvent
);
define_event_marker!(
    ServerConnected,
    "Event fired once a player finishes connecting to a backend server (`ServerConnectedEvent`).",
    ServerConnected,
    ServerConnectedEvent
);
define_event_marker!(
    PlayerDisconnect,
    "Event fired when a player disconnects from the proxy (`PlayerDisconnectEvent`).",
    PlayerDisconnect,
    PlayerDisconnectEvent
);
define_event_marker!(
    PlayerChat,
    "Event fired when a player sends a chat message (`PlayerChatEvent`).",
    PlayerChat,
    PlayerChatEvent
);
define_event_marker!(
    PlayerCommand,
    "Event fired when a player executes a command (`PlayerCommandEvent`).",
    PlayerCommand,
    PlayerCommandEvent
);
define_event_marker!(
    PluginMessage,
    "Event fired when a custom payload / plugin message traverses the proxy (`PluginMessageEvent`).",
    PluginMessage,
    PluginMessageEvent
);

/// Handler for a specific typed proxy event.
pub trait EventHandler<E: FromIntoEvent>: Send + Sync {
    /// Processes `event` and returns the (optionally modified) event payload.
    fn handle(&self, event: E::Data) -> E::Data;
}

impl<E, F> EventHandler<E> for F
where
    E: FromIntoEvent,
    F: Fn(E::Data) -> E::Data + Send + Sync,
{
    fn handle(&self, event: E::Data) -> E::Data {
        (self)(event)
    }
}

pub(crate) trait ErasedEventHandler: Send + Sync {
    fn handle_erased(&self, event: Event) -> Event;
}

struct HandlerWrapper<E: FromIntoEvent, H> {
    handler: H,
    _phantom: PhantomData<E>,
}

impl<E, H> ErasedEventHandler for HandlerWrapper<E, H>
where
    E: FromIntoEvent + Send + Sync,
    H: EventHandler<E> + Send + Sync,
{
    fn handle_erased(&self, event: Event) -> Event {
        let discriminant_backup = event.clone();
        if let Some(data) = E::try_from_event(event) {
            E::into_event(self.handler.handle(data))
        } else {
            discriminant_backup
        }
    }
}

/// Extension methods on [`Context`] for registering typed event handlers.
pub trait EventContextExt {
    /// Registers a closure taking `&mut E::Data` with the given [`EventPriority`].
    fn on_event<E, F>(&self, priority: EventPriority, handler: F) -> u32
    where
        E: FromIntoEvent + Send + Sync + 'static,
        F: Fn(&mut E::Data) + Send + Sync + 'static;

    /// Registers a struct implementing [`EventHandler<E>`] with the given [`EventPriority`].
    fn register_event_handler<E, H>(&self, priority: EventPriority, handler: H) -> u32
    where
        E: FromIntoEvent + Send + Sync + 'static,
        H: EventHandler<E> + 'static;
}

impl EventContextExt for Context {
    fn on_event<E, F>(&self, priority: EventPriority, handler: F) -> u32
    where
        E: FromIntoEvent + Send + Sync + 'static,
        F: Fn(&mut E::Data) + Send + Sync + 'static,
    {
        self.register_event_handler::<E, _>(priority, move |mut data: E::Data| {
            handler(&mut data);
            data
        })
    }

    fn register_event_handler<E, H>(&self, priority: EventPriority, handler: H) -> u32
    where
        E: FromIntoEvent + Send + Sync + 'static,
        H: EventHandler<E> + 'static,
    {
        let id = NEXT_EVENT_HANDLER_ID.fetch_add(1, Ordering::Relaxed);
        let wrapped = HandlerWrapper {
            handler,
            _phantom: PhantomData::<E>,
        };
        EVENT_HANDLERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, Arc::new(wrapped));

        self.register_event(id, E::EVENT_TYPE, priority);
        id
    }
}
