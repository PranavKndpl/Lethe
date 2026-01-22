use std::io::Cursor;
use std::time::{UNIX_EPOCH}; 
use std::collections::HashSet;
use dav_server::fs::{DavFileSystem, DavFile, DavDirEntry, DavMetaData, FsFuture, FsError, OpenOptions, ReadDirMeta};
use dav_server::davpath::DavPath;
use super::state::LetheState;
use super::file::{LetheDavFile, LetheMetaData};

#[derive(Clone)]
pub struct LetheWebDav {
    pub state: LetheState,
}

impl DavFileSystem for LetheWebDav {
    fn open<'a>(&'a self, path: &'a DavPath, options: OpenOptions) -> FsFuture<'a, Box<dyn DavFile>> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let state = self.state.clone();

        Box::pin(async move {
            let index = state.index.lock().await;
            let mut data = Vec::new();

            if let Some(entry) = index.get_file(&path_str) {
                if entry.is_dir { return Err(FsError::Forbidden); }

                if !options.truncate {
                    for block_id in &entry.blocks {
                        if let Ok(mut chunk) = state.storage.read_block(block_id, &state.key) {
                            data.append(&mut chunk);
                        }
                    }
                }
            } else if !options.write {
                return Err(FsError::NotFound);
            }

            let is_dirty = options.write;

            Ok(Box::new(LetheDavFile {
                buffer: Cursor::new(data),
                path: path_str,
                state: state.clone(),
                is_dirty,
            }) as Box<dyn DavFile>)
        })
    }

    fn read_dir<'a>(&'a self, path: &'a DavPath, _meta: ReadDirMeta) -> FsFuture<'a, dav_server::fs::FsStream<Box<dyn DavDirEntry>>> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let state = self.state.clone();

        Box::pin(async move {
            let index = state.index.lock().await;
            let mut entries = Vec::new();
            let mut seen = HashSet::new();

            for full_path in index.data.files.keys() {
                if let Some(rest) = full_path.strip_prefix(&path_str) {
                    let clean_rest = rest.trim_start_matches('/');
                    if clean_rest.is_empty() { continue; }

                    let name = clean_rest.split('/').next().unwrap_or("");
                    if !name.is_empty() && !seen.contains(name) {
                        seen.insert(name.to_string());
                        
                        let child_full_path = if path_str == "/" { format!("/{}", name) } 
                                              else { format!("{}/{}", path_str.trim_end_matches('/'), name) };

                        let meta = if let Some(e) = index.get_file(&child_full_path) {
                            LetheMetaData {
                                len: e.size,
                                modified: UNIX_EPOCH + std::time::Duration::from_secs(e.modified),
                                is_dir: e.is_dir,
                                etag: format!("\"{:x}-{:x}\"", e.size, e.modified),
                            }
                        } else {
                            LetheMetaData {
                                len: 0, modified: UNIX_EPOCH, is_dir: true, 
                                etag: format!("\"dir-{}\"", fxhash::hash64(name)),
                            }
                        };
                        entries.push(Box::new(LetheDavEntry { name: name.to_string(), meta }) as Box<dyn DavDirEntry>);
                    }
                }
            }
            let stream = futures_util::stream::iter(entries);
            Ok(Box::pin(stream) as dav_server::fs::FsStream<Box<dyn DavDirEntry>>)
        })
    }

    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let state = self.state.clone();

        Box::pin(async move {
            let index = state.index.lock().await;

            if path_str == "/" {
                return Ok(Box::new(LetheMetaData {
                    len: 0, modified: UNIX_EPOCH, is_dir: true, etag: "\"root\"".into()
                }) as Box<dyn DavMetaData>);
            }

            if let Some(e) = index.get_file(&path_str) {
                return Ok(Box::new(LetheMetaData {
                    len: e.size,
                    modified: UNIX_EPOCH + std::time::Duration::from_secs(e.modified),
                    is_dir: e.is_dir,
                    etag: format!("\"{:x}-{:x}\"", e.size, e.modified),
                }) as Box<dyn DavMetaData>);
            }

            let is_dir = index.data.files.keys().any(|k| k.starts_with(&format!("{}/", path_str)));
            if is_dir {
                return Ok(Box::new(LetheMetaData {
                    len: 0, modified: UNIX_EPOCH, is_dir: true, 
                    etag: format!("\"implicit-{}\"", fxhash::hash64(&path_str)),
                }) as Box<dyn DavMetaData>);
            }
            Err(FsError::NotFound)
        })
    }

    fn create_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let state = self.state.clone();
        Box::pin(async move {
            let mut index = state.index.lock().await;
            if index.get_file(&path_str).is_some() { return Err(FsError::Exists); }
            index.add_dir(path_str);
            let _ = index.save(&state.key);
            Ok(())
        })
    }

    fn remove_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let state = self.state.clone();
        Box::pin(async move {
            let mut index = state.index.lock().await;
            if index.data.files.keys().any(|k| k.starts_with(&format!("{}/", path_str))) { return Err(FsError::Forbidden); }
            if index.data.files.remove(&path_str).is_some() {
                let _ = index.save(&state.key);
                Ok(())
            } else { Err(FsError::NotFound) }
        })
    }

    fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, ()> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let state = self.state.clone();
        Box::pin(async move {
            let mut index = state.index.lock().await;
            if index.data.files.remove(&path_str).is_some() {
                let _ = index.save(&state.key);
                Ok(())
            } else { Err(FsError::NotFound) }
        })
    }

    fn rename<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<'a, ()> {
        let old_path = from.as_pathbuf().to_string_lossy().replace("\\", "/");
        let new_path = to.as_pathbuf().to_string_lossy().replace("\\", "/");
        let state = self.state.clone();
        Box::pin(async move {
            let mut index = state.index.lock().await;
            let mut to_move = Vec::new();
            if index.data.files.contains_key(&old_path) { to_move.push(old_path.clone()); }
            for k in index.data.files.keys() {
                if k.starts_with(&format!("{}/", old_path)) { to_move.push(k.clone()); }
            }
            if to_move.is_empty() { return Err(FsError::NotFound); }
            for src in to_move {
                if let Some(mut entry) = index.data.files.remove(&src) {
                    let suffix = src.strip_prefix(&old_path).unwrap_or("");
                    let dest = format!("{}{}", new_path, suffix);
                    entry.path = dest.clone();
                    index.data.files.insert(dest, entry);
                }
            }
            let _ = index.save(&state.key);
            Ok(())
        })
    }
}

pub struct LetheDavEntry { pub name: String, pub meta: LetheMetaData }
impl DavDirEntry for LetheDavEntry {
    fn name(&self) -> Vec<u8> { self.name.as_bytes().to_vec() }
    fn metadata(&self) -> FsFuture<Box<dyn DavMetaData>> {
        let m = self.meta.clone();
        Box::pin(async move { Ok(Box::new(m) as Box<dyn DavMetaData>) })
    }
}