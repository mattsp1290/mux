// Integration test entry point (scaffold — not yet wired into Cargo).
//
// This file and harness.rs document the planned test structure.
// When the first integration test module is written (mux-av5, mux-zpx, mux-qz4),
// a dedicated integration test crate will be added to the workspace with a
// [features] integration-tests gate and a [[test]] entry pointing here.
//
// Tests will be skipped automatically if Docker is unavailable.
// See prompts/docs/integration-tests.md for the full environment plan.

mod harness;

// Test modules are added here as the integration suite grows.
// Each module corresponds to a mux command group:
//
//   mod init;      (mux-3bv follow-on)
//   mod host;      (mux-av5)
//   mod agent;     (mux-zpx)
//   mod session;   (mux-qz4)
