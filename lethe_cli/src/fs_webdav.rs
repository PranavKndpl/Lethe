use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use tokio::sync::Mutex;
use dav_server::fs::{DavDirEntry, DavFile, DavFileSystem, DavMetaData, FsFuture, FsError, FsResult, ReadDirMeta, OpenOptions};
use dav_server::davpath::DavPath;
use lethe_core::index::IndexManager;
use lethe_core::storage::BlockManager;
use lethe_core::crypto::MasterKey;
use bytes::Buf;

// --- 0. CONCRETE METADATA STRUCT ---
#[derive(Debug, Clone)]
pub struct LetheMetaData {
    len: u64,
    modified: SystemTime,
    is_dir: bool,
    etag: String,
}

impl DavMetaData for LetheMetaData {
    fn len(&self) -> u64 { self.len }
    fn modified(&self) -> FsResult<SystemTime> { Ok(self.modified) }
    fn is_dir(&self) -> bool { self.is_dir }
    fn etag(&self) -> Option<String> { Some(self.etag.clone()) }
}

// --- 1. THE FILE HANDLE ---
#[derive(Debug)]
pub struct LetheDavFile {
    buffer: Cursor<Vec<u8>>, 
    path: String,
    fs: LetheWebDav, 
}

impl DavFile for LetheDavFile {
    fn read_bytes(&mut self, count: usize) -> FsFuture<bytes::Bytes> {
        let mut buf = vec![0u8; count];
        match self.buffer.read(&mut buf) {
            Ok(n) => {
                buf.truncate(n);
                Box::pin(async move { Ok(bytes::Bytes::from(buf)) })
            }
            Err(_) => Box::pin(async { Err(FsError::GeneralFailure) }),
        }
    }

    fn write_buf(&mut self, mut buf: Box<dyn Buf + Send>) -> FsFuture<()> {
        let mut chunk = vec![0u8; buf.remaining()];
        buf.copy_to_slice(&mut chunk);
        match self.buffer.write_all(&chunk) {
            Ok(_) => Box::pin(async { Ok(()) }),
            Err(_) => Box::pin(async { Err(FsError::GeneralFailure) }),
        }
    }

    fn write_bytes(&mut self, buf: bytes::Bytes) -> FsFuture<()> {
        self.write_buf(Box::new(buf))
    }

    fn metadata(&mut self) -> FsFuture<Box<dyn DavMetaData>> {
        let len = self.buffer.get_ref().len() as u64;
        // In-memory files are considered "new" until flushed, but we give them a specific tag
        // to avoid confusion if they are read before flush.
        let modified = SystemTime::now(); 
        let etag = format!("\"mem-{:x}\"", len); 

        Box::pin(async move {
            Ok(Box::new(LetheMetaData {
                len,
                modified,
                is_dir: false,
                etag,
            }) as Box<dyn DavMetaData>)
        })
    }

    fn seek(&mut self, pos: SeekFrom) -> FsFuture<u64> {
        let res = self.buffer.seek(pos).map_err(|_| FsError::GeneralFailure);
        Box::pin(async move { res })
    }

    fn flush(&mut self) -> FsFuture<()> {
        let path = self.path.clone();
        let data = self.buffer.get_ref().clone();
        let index_arc = self.fs.index.clone();
        let storage_arc = self.fs.storage.clone();
        let key_arc = self.fs.key.clone();

        Box::pin(async move {
            let mut index = index_arc.lock().await;
            if let Ok(block_id) = storage_arc.write_block(&data, &key_arc) {
                // When we save, we rely on IndexManager to set the 'modified' time
                index.add_file(path, vec![block_id], data.len() as u64);
                let _ = index.save(&key_arc);
                Ok(())
            } else {
                Err(FsError::GeneralFailure)
            }
        })
    }
}

// --- 2. THE FILESYSTEM ---
#[derive(Debug, Clone)]
pub struct LetheWebDav {
    pub index: Arc<Mutex<IndexManager>>,
    pub storage: Arc<BlockManager>,
    pub key: Arc<MasterKey>,
}

impl DavFileSystem for LetheWebDav {
    fn open<'a>(&'a self, path: &'a DavPath, _options: OpenOptions) -> FsFuture<Box<dyn DavFile>> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let fs_clone = self.clone();
        let index_arc = self.index.clone();
        let storage_arc = self.storage.clone();
        let key_arc = self.key.clone();

        Box::pin(async move {
            let index = index_arc.lock().await;
            let mut data = Vec::new();
            
            // If it's a directory, return error immediately
            if let Some(entry) = index.get_file(&path_str) {
                if entry.is_dir { return Err(FsError::Forbidden); }
                
                for block_id in &entry.blocks {
                    if let Ok(mut chunk) = storage_arc.read_block(block_id, &key_arc) {
                        data.append(&mut chunk);
                    }
                }
            }

            Ok(Box::new(LetheDavFile {
                buffer: Cursor::new(data),
                path: path_str,
                fs: fs_clone,
            }) as Box<dyn DavFile>)
        })
    }

    fn read_dir<'a>(&'a self, path: &'a DavPath, _meta: ReadDirMeta) -> FsFuture<dav_server::fs::FsStream<Box<dyn DavDirEntry>>> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let index_arc = self.index.clone();

        Box::pin(async move {
            let index = index_arc.lock().await;
            let mut entries = Vec::new();
            let mut seen = std::collections::HashSet::new();

            for full_path in index.data.files.keys() {
                if let Some(rest) = full_path.strip_prefix(&path_str) {
                    let clean_rest = rest.trim_start_matches('/');
                    if clean_rest.is_empty() { continue; }
                    
                    let name = clean_rest.split('/').next().unwrap_or("");
                    if !name.is_empty() && !seen.contains(name) {
                        seen.insert(name.to_string());
                        
                        let is_exact_match = full_path == &format!("{}/{}", path_str.trim_end_matches('/'), name) 
                                          || full_path == &format!("/{}", name);
                        
                        let meta = if is_exact_match {
                            if let Some(e) = index.get_file(full_path) {
                                LetheMetaData {
                                    len: e.size,
                                    modified: UNIX_EPOCH + std::time::Duration::from_secs(e.modified),
                                    is_dir: e.is_dir,
                                    // STABLE ETAG: Hash of size + modification time
                                    etag: format!("\"{:x}-{:x}\"", e.size, e.modified),
                                }
                            } else {
                                // Fallback (should not happen)
                                LetheMetaData { len: 0, modified: UNIX_EPOCH, is_dir: false, etag: "\"0\"".to_string() }
                            }
                        } else {
                            // IMPLICIT DIRECTORY
                            // CRITICAL FIX: Use stable time (EPOCH) and stable ETag (Hash of name)
                            LetheMetaData { 
                                len: 0, 
                                modified: UNIX_EPOCH, // Always 1970. Stable.
                                is_dir: true,
                                etag: format!("\"dir-{}\"", fxhash::hash64(name)), // Stable ETag
                            }
                        };

                        entries.push(Box::new(LetheDavEntry {
                            name: name.to_string(),
                            meta,
                        }) as Box<dyn DavDirEntry>);
                    }
                }
            }
            
            let stream = futures_util::stream::iter(entries);
            Ok(Box::pin(stream) as dav_server::fs::FsStream<Box<dyn DavDirEntry>>)
        })
    }

    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<Box<dyn DavMetaData>> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let index_arc = self.index.clone();

        Box::pin(async move {
            let index = index_arc.lock().await;

            if path_str == "/" {
                return Ok(Box::new(LetheMetaData {
                    len: 0, 
                    modified: UNIX_EPOCH, // Root is always 1970
                    is_dir: true,
                    etag: "\"root\"".to_string(),
                }) as Box<dyn DavMetaData>);
            }

            if let Some(entry) = index.get_file(&path_str) {
                return Ok(Box::new(LetheMetaData {
                    len: entry.size,
                    modified: UNIX_EPOCH + std::time::Duration::from_secs(entry.modified),
                    is_dir: entry.is_dir,
                    // STABLE ETAG
                    etag: format!("\"{:x}-{:x}\"", entry.size, entry.modified),
                }) as Box<dyn DavMetaData>);
            }

            // Check implicit directories
            let is_dir = index.data.files.keys().any(|k: &String| k.starts_with(&format!("{}/", path_str)));
            if is_dir {
                return Ok(Box::new(LetheMetaData {
                    len: 0,
                    modified: UNIX_EPOCH, // Implicit dirs are always 1970
                    is_dir: true,
                    // STABLE ETAG based on path hash
                    etag: format!("\"implicit-{}\"", fxhash::hash64(&path_str)),
                }) as Box<dyn DavMetaData>);
            }

            Err(FsError::NotFound)
        })
    }

    // --- WRITE OPS --- (Keep these as they were, they are safe)
    fn create_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<()> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let index_arc = self.index.clone();
        let key_arc = self.key.clone();

        Box::pin(async move {
            let mut index = index_arc.lock().await;
            if index.get_file(&path_str).is_some() { return Err(FsError::Exists); }
            index.add_dir(path_str);
            let _ = index.save(&key_arc);
            Ok(())
        })
    }

    fn remove_dir<'a>(&'a self, path: &'a DavPath) -> FsFuture<()> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let index_arc = self.index.clone();
        let key_arc = self.key.clone();

        Box::pin(async move {
            let mut index = index_arc.lock().await;
            let has_children = index.data.files.keys().any(|k| k.starts_with(&format!("{}/", path_str)));
            if has_children { return Err(FsError::Forbidden); }
            if index.data.files.remove(&path_str).is_some() {
                let _ = index.save(&key_arc);
                Ok(())
            } else {
                Err(FsError::NotFound)
            }
        })
    }

    fn remove_file<'a>(&'a self, path: &'a DavPath) -> FsFuture<()> {
        let path_str = path.as_pathbuf().to_string_lossy().replace("\\", "/");
        let index_arc = self.index.clone();
        let key_arc = self.key.clone();
        Box::pin(async move {
            let mut index = index_arc.lock().await;
            if index.data.files.remove(&path_str).is_some() {
                let _ = index.save(&key_arc);
                Ok(())
            } else {
                Err(FsError::NotFound)
            }
        })
    }

    fn rename<'a>(&'a self, from: &'a DavPath, to: &'a DavPath) -> FsFuture<()> {
        let old_path = from.as_pathbuf().to_string_lossy().replace("\\", "/");
        let new_path = to.as_pathbuf().to_string_lossy().replace("\\", "/");
        let index_arc = self.index.clone();
        let key_arc = self.key.clone();
        Box::pin(async move {
            let mut index = index_arc.lock().await;
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
            let _ = index.save(&key_arc);
            Ok(())
        })
    }
}

pub struct LetheDavEntry { name: String, meta: LetheMetaData }
impl DavDirEntry for LetheDavEntry {
    fn name(&self) -> Vec<u8> { self.name.as_bytes().to_vec() }
    fn metadata(&self) -> FsFuture<Box<dyn DavMetaData>> {
        let m = self.meta.clone();
        Box::pin(async move { Ok(Box::new(m) as Box<dyn DavMetaData>) })
    }
}