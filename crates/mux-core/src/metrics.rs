/// Timing and diagnostic data collected during a `mux create` flow.
///
/// The fields mirror the `create_flow` tracing event emitted by `run_create`.
/// Used for structured consumption via the event bus (`BusEvent::CreateFlow`)
/// when the CLI has access to a bus instance.
#[derive(Debug, Default, Clone)]
pub struct CreateFlowMetrics {
    /// Total wall-clock duration of the create operation, in milliseconds.
    pub create_duration_ms: u64,

    /// Wall-clock duration of the `git clone` step, if it was attempted.
    pub git_clone_duration_ms: Option<u64>,

    /// The error category string if the flow failed (e.g. `"git_clone_failed"`).
    /// `None` on success.
    pub error_category: Option<String>,

    /// The host alias targeted by this create flow, if known.
    pub host: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::CreateFlowMetrics;

    #[test]
    fn default_is_all_none() {
        let m = CreateFlowMetrics::default();
        assert_eq!(m.create_duration_ms, 0);
        assert!(m.git_clone_duration_ms.is_none());
        assert!(m.error_category.is_none());
        assert!(m.host.is_none());
    }

    #[test]
    fn can_set_all_fields() {
        let m = CreateFlowMetrics {
            create_duration_ms: 1234,
            git_clone_duration_ms: Some(800),
            error_category: Some("git_clone_failed".into()),
            host: Some("prod-01".into()),
        };
        assert_eq!(m.create_duration_ms, 1234);
        assert_eq!(m.git_clone_duration_ms, Some(800));
        assert_eq!(m.error_category.as_deref(), Some("git_clone_failed"));
        assert_eq!(m.host.as_deref(), Some("prod-01"));
    }
}
