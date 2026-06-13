// Transport probe and selection — implementation in mux-ed1
use mux_core::types::TransportMode;

pub fn probe_transport(_host: &str, _port: u16) -> anyhow::Result<TransportMode> {
    todo!("streamlocal probe and TCP fallback (mux-ed1)")
}
