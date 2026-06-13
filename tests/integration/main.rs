// Integration test entry point.
//
// Gated behind the `integration-tests` feature:
//   cargo test -p mux-integration-tests --test integration --features integration-tests -- --test-threads=1
//
// Tests that require Docker use the require_docker!() macro to skip gracefully
// on runners without Docker. The primary skip mechanism is the feature gate
// itself — without `--features integration-tests`, this crate is never compiled.
//
// See prompts/docs/integration-tests.md for the full environment plan.

mod harness;

// `init` tests do not require Docker — safe to run anywhere.
mod init;

// All other modules require Docker. Use --test-threads=1 when running them
// because tests share fixed-port Docker container services.
mod agent;   // mux agent deploy / logs / stop
mod host;    // mux host test / trust
mod session; // mux create / list / status / attach / kill
