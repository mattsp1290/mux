use mux_core::types::{
    Endpoint, HostAlias, Port, RepoRef, SessionSelector, SessionStatus, TransportMode,
};

// ── SessionStatus ──────────────────────────────────────────────────────────────

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
fn session_status_from_str_all_variants() {
    assert_eq!(
        "active".parse::<SessionStatus>().unwrap(),
        SessionStatus::Active
    );
    assert_eq!(
        "dead".parse::<SessionStatus>().unwrap(),
        SessionStatus::Dead
    );
    assert_eq!(
        "unreachable".parse::<SessionStatus>().unwrap(),
        SessionStatus::Unreachable
    );
    assert_eq!(
        "orphaned".parse::<SessionStatus>().unwrap(),
        SessionStatus::Orphaned
    );
}

#[test]
fn session_status_from_str_rejects_unknown_and_mixed_case() {
    assert!("Active".parse::<SessionStatus>().is_err());
    assert!("DEAD".parse::<SessionStatus>().is_err());
    assert!("pending".parse::<SessionStatus>().is_err());
    assert!("".parse::<SessionStatus>().is_err());
}

#[test]
fn session_status_error_message_contains_input() {
    let err = "bogus".parse::<SessionStatus>().unwrap_err();
    assert!(
        err.to_string().contains("bogus"),
        "error should contain bad input"
    );
}

// ── TransportMode ──────────────────────────────────────────────────────────────

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

// ── HostAlias ─────────────────────────────────────────────────────────────────

#[test]
fn host_alias_valid_forms() {
    assert!("myhost".parse::<HostAlias>().is_ok());
    assert!("host-1".parse::<HostAlias>().is_ok());
    assert!("host_name".parse::<HostAlias>().is_ok());
    assert!("a".parse::<HostAlias>().is_ok());
    assert!("a".repeat(64).parse::<HostAlias>().is_ok());
}

#[test]
fn host_alias_invalid_forms() {
    assert!("".parse::<HostAlias>().is_err(), "empty should fail");
    assert!(
        "-host".parse::<HostAlias>().is_err(),
        "leading hyphen should fail"
    );
    assert!("host.com".parse::<HostAlias>().is_err(), "dot should fail");
    assert!("my host".parse::<HostAlias>().is_err(), "space should fail");
    assert!(
        "a".repeat(65).parse::<HostAlias>().is_err(),
        "over 64 chars should fail"
    );
}

#[test]
fn host_alias_serde_roundtrip() {
    let alias: HostAlias = "my-host".parse().unwrap();
    let json = serde_json::to_string(&alias).unwrap();
    assert_eq!(json, r#""my-host""#);
    let back: HostAlias = serde_json::from_str(&json).unwrap();
    assert_eq!(alias, back);
}

#[test]
fn host_alias_deserialize_rejects_invalid() {
    assert!(serde_json::from_str::<HostAlias>(r#""has.dots""#).is_err());
    assert!(serde_json::from_str::<HostAlias>(r#""-starts-hyphen""#).is_err());
}

#[test]
fn host_alias_error_message_contains_input() {
    let err = "bad.alias".parse::<HostAlias>().unwrap_err();
    assert!(err.to_string().contains("bad.alias"));
}

// ── Port ──────────────────────────────────────────────────────────────────────

#[test]
fn port_valid_boundary_values() {
    assert_eq!("1".parse::<Port>().unwrap().value(), 1);
    assert_eq!("22".parse::<Port>().unwrap().value(), 22);
    assert_eq!("65535".parse::<Port>().unwrap().value(), 65535);
}

#[test]
fn port_invalid_values() {
    assert!("0".parse::<Port>().is_err(), "port 0 rejected");
    assert!("65536".parse::<Port>().is_err(), "port 65536 rejected");
    assert!("".parse::<Port>().is_err(), "empty rejected");
    assert!("ssh".parse::<Port>().is_err(), "non-numeric rejected");
    assert!("-1".parse::<Port>().is_err(), "negative rejected");
}

#[test]
fn port_default_is_22() {
    assert_eq!(Port::default().value(), 22);
}

#[test]
fn port_serde_number_and_string_forms() {
    let p: Port = serde_json::from_str("22").unwrap();
    assert_eq!(p.value(), 22);
    let p: Port = serde_json::from_str(r#""8022""#).unwrap();
    assert_eq!(p.value(), 8022);
}

#[test]
fn port_serde_rejects_invalid() {
    assert!(serde_json::from_str::<Port>("0").is_err());
    assert!(serde_json::from_str::<Port>("65536").is_err());
    assert!(serde_json::from_str::<Port>(r#""65536""#).is_err());
}

#[test]
fn port_try_from_u16() {
    assert_eq!(Port::try_from(22u16).unwrap().value(), 22);
    assert!(Port::try_from(0u16).is_err());
}

#[test]
fn port_error_message_contains_input() {
    let err = "99999".parse::<Port>().unwrap_err();
    assert!(err.to_string().contains("99999"));
}

// ── Endpoint ──────────────────────────────────────────────────────────────────

#[test]
fn endpoint_valid_forms() {
    let ep: Endpoint = "alice@192.168.1.1".parse().unwrap();
    assert_eq!(ep.user(), "alice");
    assert_eq!(ep.addr(), "192.168.1.1");

    let ep: Endpoint = "bob@host.example.com".parse().unwrap();
    assert_eq!(ep.user(), "bob");
    assert_eq!(ep.addr(), "host.example.com");
}

#[test]
fn endpoint_invalid_forms() {
    assert!("noatsign".parse::<Endpoint>().is_err());
    assert!("".parse::<Endpoint>().is_err());
    assert!("@host".parse::<Endpoint>().is_err(), "empty user rejected");
    assert!("user@".parse::<Endpoint>().is_err(), "empty addr rejected");
    assert!(
        "user@host@extra".parse::<Endpoint>().is_err(),
        "@ in addr rejected"
    );
}

#[test]
fn endpoint_display_roundtrip() {
    let s = "alice@192.168.1.1";
    let ep: Endpoint = s.parse().unwrap();
    assert_eq!(ep.to_string(), s);
}

#[test]
fn endpoint_serde_flat_string_format() {
    let ep: Endpoint = "alice@192.168.1.1".parse().unwrap();
    let json = serde_json::to_string(&ep).unwrap();
    assert_eq!(json, r#""alice@192.168.1.1""#, "serializes as flat string");
    let back: Endpoint = serde_json::from_str(&json).unwrap();
    assert_eq!(ep, back);
}

#[test]
fn endpoint_deserialize_rejects_invalid() {
    assert!(serde_json::from_str::<Endpoint>(r#""noatsign""#).is_err());
    assert!(serde_json::from_str::<Endpoint>(r#""@addr""#).is_err());
}

#[test]
fn endpoint_error_message_contains_input() {
    let err = "badformat".parse::<Endpoint>().unwrap_err();
    assert!(err.to_string().contains("badformat"));
}

// ── SessionSelector ───────────────────────────────────────────────────────────

#[test]
fn session_selector_uuid_vs_shortname() {
    let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
    let sel: SessionSelector = uuid_str.parse().unwrap();
    assert!(
        matches!(sel, SessionSelector::Uuid(_)),
        "UUID-shaped string parses as Uuid"
    );

    let sel: SessionSelector = "my-session".parse().unwrap();
    assert_eq!(
        sel,
        SessionSelector::Shortname("my-session".to_owned()),
        "non-UUID parses as Shortname"
    );
}

#[test]
fn session_selector_uuid_shaped_shortname_is_uuid() {
    let sel: SessionSelector = "00000000-0000-0000-0000-000000000000".parse().unwrap();
    assert!(matches!(sel, SessionSelector::Uuid(_)));
}

#[test]
fn session_selector_infallible_any_string() {
    let _: SessionSelector = "".parse().unwrap();
    let _: SessionSelector = "any-random-string-123".parse().unwrap();
}

// ── RepoRef ───────────────────────────────────────────────────────────────────

#[test]
fn repo_ref_owner_repo_form() {
    let r: RepoRef = "mattsp1290/mux".parse().unwrap();
    assert_eq!(r.owner(), "mattsp1290");
    assert_eq!(r.repo(), "mux");
    assert_eq!(r.host(), None);
    assert_eq!(r.repo_slug(), "mattsp1290/mux");
    assert_eq!(r.repo_leaf(), "mux");
    assert_eq!(r.clone_url(), None, "no host → no clone URL");
}

#[test]
fn repo_ref_git_url_form() {
    let r: RepoRef = "git@github.com:mattsp1290/mux.git".parse().unwrap();
    assert_eq!(r.owner(), "mattsp1290");
    assert_eq!(r.repo(), "mux");
    assert_eq!(r.host(), Some("github.com"));
    assert_eq!(r.repo_slug(), "mattsp1290/mux");
    assert_eq!(r.clone_url().unwrap(), "git@github.com:mattsp1290/mux.git");
}

#[test]
fn repo_ref_rejects_dot_git_shorthand() {
    let err = "mattsp1290/mux.git".parse::<RepoRef>().unwrap_err();
    assert!(
        err.to_string().contains("mux.git") || err.to_string().contains("owner/repo.git"),
        "error should mention the rejected form; got: {err}"
    );
}

#[test]
fn repo_ref_invalid_forms() {
    assert!("/mux".parse::<RepoRef>().is_err(), "empty owner rejected");
    assert!(
        "mattsp1290/".parse::<RepoRef>().is_err(),
        "empty repo rejected"
    );
    assert!(
        "mattsp1290".parse::<RepoRef>().is_err(),
        "no slash rejected"
    );
    assert!(
        "org/sub/repo".parse::<RepoRef>().is_err(),
        "three-part rejected"
    );
}

#[test]
fn repo_ref_lowercases_input() {
    let r: RepoRef = "MyOrg/MyRepo".parse().unwrap();
    assert_eq!(r.owner(), "myorg");
    assert_eq!(r.repo(), "myrepo");
}

#[test]
fn repo_ref_storage_slug() {
    let r: RepoRef = "my-org/my_repo".parse().unwrap();
    assert_eq!(r.storage_slug(), "my-org-my-repo");

    let r: RepoRef = "org/dotted.repo".parse().unwrap();
    assert_eq!(r.storage_slug(), "org-dotted-repo");
}

#[test]
fn repo_ref_clone_url_for() {
    let r: RepoRef = "mattsp1290/mux".parse().unwrap();
    assert_eq!(
        r.clone_url_for("github.com"),
        "git@github.com:mattsp1290/mux.git"
    );
    // Stored host takes precedence
    let r: RepoRef = "git@gitlab.com:mattsp1290/mux.git".parse().unwrap();
    assert_eq!(
        r.clone_url_for("github.com"),
        "git@gitlab.com:mattsp1290/mux.git"
    );
}

#[test]
fn repo_ref_display_roundtrip_owner_repo() {
    let s = "mattsp1290/mux";
    let r: RepoRef = s.parse().unwrap();
    assert_eq!(r.to_string(), s);
}

#[test]
fn repo_ref_display_roundtrip_git_url() {
    let s = "git@github.com:mattsp1290/mux.git";
    let r: RepoRef = s.parse().unwrap();
    assert_eq!(r.to_string(), s);
}

#[test]
fn repo_ref_git_url_invalid_forms() {
    assert!(
        "git@:owner/repo.git".parse::<RepoRef>().is_err(),
        "empty host rejected"
    );
    assert!(
        "git@github.com/owner/repo.git".parse::<RepoRef>().is_err(),
        "no colon rejected"
    );
}

#[test]
fn repo_ref_error_message_contains_input() {
    let err = "not/valid/repo".parse::<RepoRef>().unwrap_err();
    assert!(err.to_string().contains("not/valid/repo"));
}
