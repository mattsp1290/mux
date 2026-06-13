use mux_core::types::{SessionStatus, TransportMode};

#[test]
fn session_status_variants_exist() {
    let _ = SessionStatus::Active;
    let _ = SessionStatus::Dead;
    let _ = SessionStatus::Unreachable;
    let _ = SessionStatus::Orphaned;
}

#[test]
fn transport_mode_variants_exist() {
    let _ = TransportMode::Streamlocal;
    let _ = TransportMode::Tcp;
}
