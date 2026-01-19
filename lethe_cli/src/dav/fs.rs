// lethe_cli/src/dav/fs.rs
use super::file::LetheDavFile;
use crate::dav::state::LetheState;
use dav_server::fs::{DavFileSystem, DavFile, DavDirEntry, FsFuture, FsError, OpenOptions, ReadDirMeta};
use dav_server::davpath::DavPath;
use std::sync::Arc;
use futures_util::stream;
use std::time::UNIX_EPOCH;

#[derive(Clone)]
pub struct LetheWebDav {
    pub state: Arc<LetheState>,
}

impl DavFileSystem for LetheWebDav {
    fn open<'a>(&'a self, path: &'a DavPath, options: OpenOptions) -> FsFuture<Box<dyn DavFile>> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let state = self.state.clone();

        Box::pin(async move {
            let guard = state.get_resources().await.ok_or(FsError::Forbidden)?;

            if options.write && options.truncate {
                return Ok(Box::new(LetheDavFile {
                    index: guard.index.clone(),
                    storage: guard.storage.clone(),
                    key: guard.key.clone(),
                    path: path_str.clone(),
                    read_blocks: vec![],
                    file_size: 0,
                    pos: 0,
                    write_buffer: Vec::with_capacity(65536),
                    new_block_ids: Vec::new(),
                    is_dirty: true,
                }) as Box<dyn DavFile>);
            }

            let index = guard.index.lock().await;
            if let Some(entry) = index.get_file(&path_str) {
                Ok(Box::new(LetheDavFile {
                    index: guard.index.clone(),
                    storage: guard.storage.clone(),
                    key: guard.key.clone(),
                    path: path_str.clone(),
                    read_blocks: entry.blocks.clone(),
                    file_size: entry.size,
                    pos: 0,
                    write_buffer: Vec::new(),
                    new_block_ids: Vec::new(),
                    is_dirty: false,
                }) as Box<dyn DavFile>)
            } else if options.write {
                Ok(Box::new(LetheDavFile {
                    index: guard.index.clone(),
                    storage: guard.storage.clone(),
                    key: guard.key.clone(),
                    path: path_str.clone(),
                    read_blocks: vec![],
                    file_size: 0,
                    pos: 0,
                    write_buffer: Vec::with_capacity(65536),
                    new_block_ids: Vec::new(),
                    is_dirty: true,
                }) as Box<dyn DavFile>)
            } else {
                Err(FsError::NotFound)
            }
        })
    }

    fn read_dir<'a>(&'a self, path: &'a DavPath, _meta: ReadDirMeta) -> FsFuture<dav_server::fs::FsStream<Box<dyn DavDirEntry>>> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let state = self.state.clone();

        Box::pin(async move {
            let guard = state.get_resources().await.ok_or(FsError::Forbidden)?;
            let index = guard.index.lock().await;
            let mut entries = Vec::new();
            let mut seen = std::collections::HashSet::new();

            for full_path in index.data.files.keys() {
                if let Some(rest) = full_path.strip_prefix(&path_str) {
                    let clean_rest = rest.trim_start_matches('/');
                    if clean_rest.is_empty() { continue; }

                    let name = clean_rest.split('/').next().unwrap_or("");
                    if !name.is_empty() && !seen.contains(name) {
                        seen.insert(name.to_string());

                        let is_file = index.get_file(full_path).is_some();
                        let meta = if is_file {
                            let e = index.get_file(full_path).unwrap();
                            super::file::LetheFileMetaData {
                                len: e.size,
                                modified: UNIX_EPOCH + std::time::Duration::from_secs(e.modified),
                                is_dir: e.is_dir,
                                etag: format!("\"{:x}-{:x}\"", e.size, e.modified),
                            }
                        } else {
                            super::file::LetheFileMetaData {
                                len: 0,
                                modified: UNIX_EPOCH,
                                is_dir: true,
                                etag: format!("\"dir-{}\"", fxhash::hash64(name)),
                            }
                        };
                        entries.push(Box::new(super::LetheDavDirEntry {
                            name: name.to_string(),
                            meta,
                        }) as Box<dyn DavDirEntry>);
                    }
                }
            }

            Ok(Box::pin(stream::iter(entries)) as dav_server::fs::FsStream<Box<dyn DavDirEntry>>)
        })
    }

    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<Box<dyn dav_server::fs::DavMetaData>> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let state = self.state.clone();

        Box::pin(async move {
            let guard = state.get_resources().await.ok_or(FsError::Forbidden)?;
            let index = guard.index.lock().await;

            if path_str == "/" {
                return Ok(Box::new(super::file::LetheFileMetaData {
                    len: 0,
                    modified: UNIX_EPOCH,
                    is_dir: true,
                    etag: "\"root\"".to_string(),
                }) as Box<dyn dav_server::fs::DavMetaData>);
            }

            if let Some(entry) = index.get_file(&path_str) {
                return Ok(Box::new(super::file::LetheFileMetaData {
                    len: entry.size,
                    modified: UNIX_EPOCH + std::time::Duration::from_secs(entry.modified),
                    is_dir: entry.is_dir,
                    etag: format!("\"{:x}-{:x}\"", entry.size, entry.modified),
                }) as Box<dyn dav_server::fs::DavMetaData>);
            }

            let is_dir = index.data.files.keys().any(|k| k.starts_with(&format!("{}/", path_str)));
            if is_dir {
                return Ok(Box::new(super::file::LetheFileMetaData {
                    len: 0,
                    modified: UNIX_EPOCH,
                    is_dir: true,
                    etag: format!("\"implicit-{}\"", fxhash::hash64(&path_str)),
                }) as Box<dyn dav_server::fs::DavMetaData>);
            }

            Err(FsError::NotFound)
        })
    }
}
