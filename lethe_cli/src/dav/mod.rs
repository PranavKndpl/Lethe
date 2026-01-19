pub mod fs;
pub mod file;
pub mod state;

pub use fs::LetheWebDav;
pub use state::LetheState;

use dav_server::fs::{DavDirEntry, DavMetaData, FsFuture};
use self::file::LetheFileMetaData;

// --- INLINE DIRECTORY ENTRY STRUCT ---
pub struct LetheDavDirEntry {
    pub name: String,
    pub meta: LetheFileMetaData,
}

impl DavDirEntry for LetheDavDirEntry {
    fn name(&self) -> Vec<u8> {
        self.name.as_bytes().to_vec()
    }

    fn metadata(&self) -> FsFuture<Box<dyn DavMetaData>> {
        let m = self.meta.clone();
        Box::pin(async move { Ok(Box::new(m) as Box<dyn DavMetaData>) })
    }
}