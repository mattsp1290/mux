//! Bounded non-blocking in-process event bus.
//!
//! Publishers never block: if all subscribers are slow and the buffer is full,
//! the oldest event is overwritten. Lagged subscribers receive
//! [`tokio::sync::broadcast::error::RecvError::Lagged`] on their next recv.
//!
//! The bus is a thin wrapper around [`tokio::sync::broadcast`] with a fixed
//! capacity ([`BUS_CAPACITY`]).

use tokio::sync::broadcast;

/// Buffer capacity. When full, the oldest event is dropped to make room.
pub const BUS_CAPACITY: usize = 64;

// ── BusEvent ──────────────────────────────────────────────────────────────────

/// Fired after each RPC method completes.
#[derive(Debug, Clone)]
pub struct RpcRequestEvent {
    /// RPC method name, e.g. `"Health"`, `"CreateSession"`.
    pub method: String,
    /// Wall-clock duration of the dispatch, in milliseconds.
    pub duration_ms: u64,
    /// `true` if the response was a success (no `"error"` field).
    pub success: bool,
}

/// Fired when the `mux create` transaction completes (success or failure).
#[derive(Debug, Clone)]
pub struct CreateFlowEvent {
    /// Total wall-clock duration of the create flow, in milliseconds.
    pub create_duration_ms: u64,
    /// Time spent in git clone, if the step was reached.
    pub git_clone_duration_ms: Option<u64>,
    /// Error category string on failure (e.g. `"git_clone_failed"`); `None` on success.
    pub error_category: Option<String>,
    /// Host alias targeted by this create flow.
    pub host: Option<String>,
}

/// Events published to the in-process event bus.
#[derive(Debug, Clone)]
pub enum BusEvent {
    /// An RPC request completed.
    RpcRequest(RpcRequestEvent),
    /// A `mux create` flow completed (success or failure).
    CreateFlow(CreateFlowEvent),
    /// The mux-agent started and is ready to accept connections.
    AgentStarted {
        /// The address the agent is listening on.
        listen_addr: String,
    },
    /// The mux-agent's serve loop exited.
    AgentStopped,
}

// ── EventBus ──────────────────────────────────────────────────────────────────

/// Bounded, non-blocking in-process publish/subscribe bus.
///
/// Create one with [`EventBus::new`], clone the [`Arc`][std::sync::Arc] to share
/// it across components, subscribe with [`EventBus::subscribe`], and publish with
/// [`EventBus::publish`].
pub struct EventBus {
    tx: broadcast::Sender<BusEvent>,
}

impl EventBus {
    /// Create a new bus with capacity [`BUS_CAPACITY`].
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    /// Subscribe to future events.
    ///
    /// The receiver holds a view into the ring buffer. If the subscriber falls
    /// behind by more than [`BUS_CAPACITY`] events, the next recv returns
    /// [`tokio::sync::broadcast::error::RecvError::Lagged`]; missed events are
    /// silently dropped on the publish side.
    pub fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        self.tx.subscribe()
    }

    /// Publish an event without blocking.
    ///
    /// Drops the event silently if there are no active subscribers or if the
    /// buffer is full and all subscribers are lagged.
    pub fn publish(&self, event: BusEvent) {
        // send() returns Err only when there are no receivers — safe to discard.
        let _ = self.tx.send(event);
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::RecvError;

    #[tokio::test]
    async fn publish_subscribe_roundtrip() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(BusEvent::AgentStarted { listen_addr: "127.0.0.1:9000".into() });
        let event = rx.recv().await.unwrap();
        assert!(
            matches!(event, BusEvent::AgentStarted { listen_addr } if listen_addr == "127.0.0.1:9000")
        );
    }

    #[test]
    fn publish_with_no_subscribers_does_not_panic() {
        let bus = EventBus::new();
        // No subscribers — publish must be a no-op, not a panic.
        bus.publish(BusEvent::AgentStopped);
        bus.publish(BusEvent::AgentStopped);
    }

    #[tokio::test]
    async fn buffer_full_drops_oldest_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // Fill the buffer past capacity without reading.
        for i in 0..=(BUS_CAPACITY as u64) {
            bus.publish(BusEvent::RpcRequest(RpcRequestEvent {
                method: format!("op-{i}"),
                duration_ms: i,
                success: true,
            }));
        }

        // The receiver should report Lagged (oldest events were dropped).
        let result = rx.recv().await;
        assert!(
            matches!(result, Err(RecvError::Lagged(_))),
            "expected Lagged after buffer overflow, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn multiple_subscribers_each_receive_event() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.publish(BusEvent::AgentStopped);

        assert!(matches!(rx1.recv().await.unwrap(), BusEvent::AgentStopped));
        assert!(matches!(rx2.recv().await.unwrap(), BusEvent::AgentStopped));
    }

    #[tokio::test]
    async fn rpc_request_event_fields_preserved() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(BusEvent::RpcRequest(RpcRequestEvent {
            method: "CreateSession".into(),
            duration_ms: 42,
            success: false,
        }));
        let event = rx.recv().await.unwrap();
        if let BusEvent::RpcRequest(e) = event {
            assert_eq!(e.method, "CreateSession");
            assert_eq!(e.duration_ms, 42);
            assert!(!e.success);
        } else {
            panic!("expected RpcRequest event");
        }
    }

    #[tokio::test]
    async fn create_flow_event_fields_preserved() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish(BusEvent::CreateFlow(CreateFlowEvent {
            create_duration_ms: 1500,
            git_clone_duration_ms: Some(900),
            error_category: Some("git_clone_failed".into()),
            host: Some("prod-01".into()),
        }));
        let event = rx.recv().await.unwrap();
        if let BusEvent::CreateFlow(e) = event {
            assert_eq!(e.create_duration_ms, 1500);
            assert_eq!(e.git_clone_duration_ms, Some(900));
            assert_eq!(e.error_category.as_deref(), Some("git_clone_failed"));
            assert_eq!(e.host.as_deref(), Some("prod-01"));
        } else {
            panic!("expected CreateFlow event");
        }
    }
}
