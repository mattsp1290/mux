use anyhow::Result;
use std::path::Path;

pub struct Store {
    // SQLite connection placeholder — rusqlite integration in mux-7sa
    _path: std::path::PathBuf,
}

impl Store {
    pub fn open(_path: &Path) -> Result<Self> {
        todo!("SQLite store opening (mux-7sa)")
    }
}
