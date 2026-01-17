use std::sync::Arc;
use std::time::SystemTime;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use tokio::sync::Mutex;
use dav_server::fs::{DavDirEntry, DavFile, DavFileSystem, DavMetaData, FsFuture, FsError, FsResult, ReadDirMeta, OpenOptions};
use dav_server::davpath::DavPath;
use lethe_core::index::IndexManager;
use lethe_core::storage::BlockManager;
use lethe_core::crypto::MasterKey;
use bytes::Buf; // Import Buf trait for buffer operations

// --- 0. CONCRETE METADATA STRUCT ---
#[derive(Debug, Clone)]
pub struct LetheMetaData {
    len: u64,
    modified: SystemTime,
    is_dir: bool,
}

impl DavMetaData for LetheMetaData {
    fn len(&self) -> u64 { self.len }
    fn modified(&self) -> FsResult<SystemTime> { Ok(self.modified) }
    fn is_dir(&self) -> bool { self.is_dir }
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
        Box::pin(async move {
            Ok(Box::new(LetheMetaData {
                len,
                modified: SystemTime::now(),
                is_dir: false,
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
        
        // Clone individual Arcs to avoid moving 'fs'
        let index_arc = self.fs.index.clone();
        let storage_arc = self.fs.storage.clone();
        let key_arc = self.fs.key.clone();

        Box::pin(async move {
            let mut index = index_arc.lock().await;
            if let Ok(block_id) = storage_arc.write_block(&data, &key_arc) {
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
        
        // Clone for the async block
        let fs_clone = self.clone();
        let index_arc = self.index.clone();
        let storage_arc = self.storage.clone();
        let key_arc = self.key.clone();

        Box::pin(async move {
            let index = index_arc.lock().await;
            let mut data = Vec::new();
            
            if let Some(entry) = index.get_file(&path_str) {
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
        
        // Clone for the async block
        let index_arc = self.index.clone();

        Box::pin(async move {
            let index = index_arc.lock().await;
            let mut entries = Vec::new();
            let mut seen = std::collections::HashSet::new();

            for full_path in index.data.files.keys() {
                // EXPLICIT TYPE FIX: strip_prefix returns Option<&str>
                if let Some(rest) = full_path.strip_prefix(&path_str) {
                    let clean_rest = rest.trim_start_matches('/');
                    if clean_rest.is_empty() { continue; }
                    
                    let name = clean_rest.split('/').next().unwrap_or("");
                    if !name.is_empty() && !seen.contains(name) {
                        seen.insert(name.to_string());
                        
                        let is_file = full_path == &format!("{}/{}", path_str.trim_end_matches('/'), name) 
                                   || full_path == &format!("/{}", name);
                        
                        let meta = if is_file {
                            if let Some(e) = index.get_file(full_path) {
                                LetheMetaData {
                                    len: e.size,
                                    modified: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(e.modified),
                                    is_dir: false,
                                }
                            } else {
                                LetheMetaData { len: 0, modified: SystemTime::now(), is_dir: false }
                            }
                        } else {
                            LetheMetaData { len: 0, modified: SystemTime::now(), is_dir: true }
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
        
        // Clone for the async block
        let index_arc = self.index.clone();

        Box::pin(async move {
            let index = index_arc.lock().await;

            if path_str == "/" {
                return Ok(Box::new(LetheMetaData {
                    len: 0, 
                    modified: SystemTime::now(), 
                    is_dir: true
                }) as Box<dyn DavMetaData>);
            }

            if let Some(entry) = index.get_file(&path_str) {
                return Ok(Box::new(LetheMetaData {
                    len: entry.size,
                    modified: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(entry.modified),
                    is_dir: false
                }) as Box<dyn DavMetaData>);
            }

            // EXPLICIT TYPE FIX: Hint the closure type |k: &String|
            let is_dir = index.data.files.keys().any(|k: &String| k.starts_with(&format!("{}/", path_str)));
            if is_dir {
                return Ok(Box::new(LetheMetaData {
                    len: 0,
                    modified: SystemTime::now(),
                    is_dir: true
                }) as Box<dyn DavMetaData>);
            }

            Err(FsError::NotFound)
        })
    }
}

// --- 3. THE DIRECTORY ENTRY ---
pub struct LetheDavEntry { 
    name: String, 
    meta: LetheMetaData 
}
impl DavDirEntry for LetheDavEntry {
    fn name(&self) -> Vec<u8> { self.name.as_bytes().to_vec() }
    fn metadata(&self) -> FsFuture<Box<dyn DavMetaData>> {
        let m = self.meta.clone();
        Box::pin(async move { Ok(Box::new(m) as Box<dyn DavMetaData>) })
    }
}