//! Event bus semantic tests.
//!
//! Verifies non-blocking publish/subscribe/drop behavior and required signal names.

use mux_core::event_bus::{BusEvent, BUS_CAPACITY, CreateFlowEvent, EventBus, RpcRequestEvent};
use std::sync::Arc;

// ── Non-blocking semantic tests ───────────────────────────────────────────────

#[tokio::test]
async fn subscriber_after_publish_misses_event() {
    // Publish before subscribing → the event is not buffered for late subscribers.
    let bus = EventBus::new();
    bus.publish(BusEvent::AgentStopped);
    let mut rx = bus.subscribe();
    // Nothing to receive — the channel is empty.
    assert!(rx.try_recv().is_err(), "late subscriber must not see events published before subscribe");
}

#[tokio::test]
async fn publish_does_not_block_with_slow_subscriber() {
    // Fill the buffer twice over without reading — publish must return immediately every time.
    let bus = Arc::new(EventBus::new());
    let mut _rx = bus.subscribe(); // hold a receiver so send() doesn't return Err
    for i in 0..(BUS_CAPACITY * 2) as u64 {
        bus.publish(BusEvent::RpcRequest(RpcRequestEvent {
            method: "Health".into(),
            duration_ms: i,
            success: true,
        }));
    }
    // If we reach here without blocking or panicking, the test passes.
}

#[tokio::test]
async fn publish_to_lagged_subscriber_does_not_panic() {
    use tokio::sync::broadcast::error::RecvError;
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    // Overflow the buffer.
    for i in 0..=(BUS_CAPACITY as u64) {
        bus.publish(BusEvent::RpcRequest(RpcRequestEvent {
            method: format!("op-{i}"),
            duration_ms: i,
            success: true,
        }));
    }

    // Receiver is lagged — recv returns Lagged, not a panic.
    let result = rx.recv().await;
    assert!(
        matches!(result, Err(RecvError::Lagged(_))),
        "overflowed subscriber must get Lagged, not panic: {result:?}"
    );
}

// ── Signal field name / structure tests ───────────────────────────────────────

#[test]
fn bus_capacity_is_64() {
    assert_eq!(BUS_CAPACITY, 64, "bus capacity must be 64 (spec §create-flow-observability)");
}

#[test]
fn rpc_request_event_has_required_fields() {
    // Verify the fields that observability consumers depend on exist with correct types.
    let e = RpcRequestEvent {
        method: "Health".into(),
        duration_ms: 42,
        success: true,
    };
    assert_eq!(e.method, "Health");
    let _: u64 = e.duration_ms;   // must be u64 (not usize or u32)
    let _: bool = e.success;
}

#[test]
fn create_flow_event_has_required_fields() {
    // Verify all spec-required fields: create_duration_ms, git_clone_duration_ms,
    // error_category (for label cardinality), host.
    let e = CreateFlowEvent {
        create_duration_ms: 1200,
        git_clone_duration_ms: Some(400),
        error_category: Some("git_clone_failed".into()),
        host: Some("prod-01".into()),
    };
    let _: u64 = e.create_duration_ms;
    let _: Option<u64> = e.git_clone_duration_ms;
    let _: Option<String> = e.error_category;
    let _: Option<String> = e.host;
}

#[test]
fn error_category_values_are_well_known() {
    // Document and exercise the four error categories used in create.rs.
    let categories = [
        "git_clone_failed",
        "agent_start_failed",
        "rpc_create_session_failed",
        "activate_failed",
    ];
    for cat in categories {
        let e = CreateFlowEvent {
            create_duration_ms: 0,
            git_clone_duration_ms: None,
            error_category: Some(cat.into()),
            host: None,
        };
        assert_eq!(e.error_category.as_deref(), Some(cat));
    }
}
