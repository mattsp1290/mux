pub struct TmuxAdapter;

impl TmuxAdapter {
    pub async fn new_session(&self, _name: &str, _workdir: &str) -> anyhow::Result<()> {
        todo!("tmux new-session via direct argv (mux-8m1)")
    }

    pub async fn kill_session(&self, _name: &str) -> anyhow::Result<()> {
        todo!("tmux kill-session via direct argv (mux-8m1)")
    }

    pub async fn list_sessions(&self) -> anyhow::Result<Vec<String>> {
        todo!("tmux ls via direct argv (mux-8m1)")
    }
}
