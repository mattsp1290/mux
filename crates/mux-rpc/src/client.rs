// RPC client over SSH-forwarded socket — implementation in mux-4kc
pub struct RpcClient;

impl RpcClient {
    pub async fn health(&self) -> anyhow::Result<super::schema::HealthResponse> {
        todo!("RPC client over SSH socket (mux-4kc)")
    }
}
