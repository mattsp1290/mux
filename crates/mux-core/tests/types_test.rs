use mux_core::types::{SessionStatus, TransportMode};

#[test]
fn session_status_serde_all_variants() {
    let cases = [
        (SessionStatus::Active, "\"active\""),
        (SessionStatus::Dead, "\"dead\""),
        (SessionStatus::Unreachable, "\"unreachable\""),
        (SessionStatus::Orphaned, "\"orphaned\""),
    ];
    for (status, expected_json) in cases {
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, expected_json, "unexpected wire format for {status:?}");
        let back: SessionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, status, "round-trip failed for {status:?}");
    }
}

#[test]
fn transport_mode_serde_all_variants() {
    let cases = [
        (TransportMode::Streamlocal, "\"streamlocal\""),
        (TransportMode::Tcp, "\"tcp\""),
    ];
    for (mode, expected_json) in cases {
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, expected_json, "unexpected wire format for {mode:?}");
        let back: TransportMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, mode, "round-trip failed for {mode:?}");
    }
}
