// lethe_cli/src/dav/file.rs
use std::io::{Cursor, Seek, SeekFrom};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use bytes::{Buf, Bytes};
use dav_server::fs::{DavFile, DavMetaData, FsError, FsFuture};
use lethe_core::storage::BlockManager;
use lethe_core::crypto::MasterKey;
use lethe_core::index::IndexManager;
use tokio::sync::Mutex;

// Size of each storage block (64KB)
const BLOCK_SIZE: usize = 65536;

#[derive(Debug, Clone)]
pub struct LetheFileMetaData {
    pub len: u64,
    pub modified: SystemTime,
    pub is_dir: bool,
    pub etag: String,
}

impl DavMetaData for LetheFileMetaData {
    fn len(&self) -> u64 { self.len }
    fn modified(&self) -> Result<SystemTime, FsError> { Ok(self.modified) }
    fn is_dir(&self) -> bool { self.is_dir }
    fn etag(&self) -> Option<String> { Some(self.etag.clone()) }
}

#[derive(Debug)]
pub struct LetheDavFile {
    // Shared state
    pub index: Arc<Mutex<IndexManager>>,
    pub storage: Arc<BlockManager>,
    pub key: Arc<MasterKey>,

    // File identity
    pub path: String,

    // READ state
    pub read_blocks: Vec<String>,
    pub file_size: u64,
    pub pos: u64,

    // WRITE state
    pub write_buffer: Vec<u8>,
    pub new_block_ids: Vec<String>,
    pub is_dirty: bool,
}

impl LetheDavFile {
    /// Flushes the current write_buffer to storage
    fn flush_chunk(&mut self) -> Result<(), FsError> {
        if self.write_buffer.is_empty() {
            return Ok(());
        }

        // Write the buffer as a block
        let block_id = self.storage.write_block(&self.write_buffer, &self.key)
            .map_err(|_| FsError::GeneralFailure)?;
        self.new_block_ids.push(block_id);
        self.write_buffer.clear();
        Ok(())
    }

    /// Helper to get the total file size from blocks
    fn total_size(&self) -> u64 {
        self.file_size + self.write_buffer.len() as u64
    }
}

impl DavFile for LetheDavFile {
    fn read_bytes(&mut self, count: usize) -> FsFuture<Bytes> {
        let start = self.pos as usize;
        let end = std::cmp::min(start + count, self.file_size as usize);

        // For simplicity, we load blocks sequentially into memory
        let storage = self.storage.clone();
        let key = self.key.clone();
        let blocks = self.read_blocks.clone();
        let mut buf = Vec::with_capacity(count);
        let pos = self.pos;

        Box::pin(async move {
            let mut offset = 0usize;
            let mut remaining = count;

            for block_id in blocks {
                if remaining == 0 { break; }
                let block = storage.read_block(&block_id, &key)
                    .map_err(|_| FsError::GeneralFailure)?;
                if pos as usize + offset >= block.len() {
                    offset += block.len();
                    continue;
                }
                let slice_start = pos as usize + offset;
                let slice_end = std::cmp::min(slice_start + remaining, block.len());
                buf.extend_from_slice(&block[slice_start..slice_end]);
                remaining -= slice_end - slice_start;
                offset += block.len();
            }

            self.pos += buf.len() as u64;
            Ok(Bytes::from(buf))
        })
    }

    fn write_buf(&mut self, mut buf: Box<dyn Buf + Send>) -> FsFuture<()> {
        let mut chunk = vec![0u8; buf.remaining()];
        buf.copy_to_slice(&mut chunk);

        Box::pin(async move {
            self.write_buffer.extend_from_slice(&chunk);
            self.is_dirty = true;

            // Flush in chunks of BLOCK_SIZE
            if self.write_buffer.len() >= BLOCK_SIZE {
                self.flush_chunk()?;
            }
            Ok(())
        })
    }

    fn write_bytes(&mut self, buf: Bytes) -> FsFuture<()> {
        self.write_buf(Box::new(buf))
    }

    fn flush(&mut self) -> FsFuture<()> {
        let path = self.path.clone();
        let index = self.index.clone();
        let key = self.key.clone();

        // Flush remaining buffer first
        if !self.write_buffer.is_empty() {
            if let Err(_) = self.flush_chunk() {
                return Box::pin(async { Err(FsError::GeneralFailure) });
            }
        }

        let blocks = self.new_block_ids.clone();
        let size = self.total_size();

        Box::pin(async move {
            let mut idx = index.lock().await;
            idx.add_file(path, blocks, size);
            idx.save(&key).map_err(|_| FsError::GeneralFailure)?;
            Ok(())
        })
    }

    fn seek(&mut self, pos: SeekFrom) -> FsFuture<u64> {
        let new_pos = match pos {
            SeekFrom::Start(off) => off,
            SeekFrom::End(off) => (self.total_size() as i64 + off) as u64,
            SeekFrom::Current(off) => (self.pos as i64 + off) as u64,
        };
        self.pos = new_pos;
        Box::pin(async move { Ok(new_pos) })
    }

    fn metadata(&mut self) -> FsFuture<Box<dyn DavMetaData>> {
        let meta = LetheFileMetaData {
            len: self.total_size(),
            modified: SystemTime::now(),
            is_dir: false,
            etag: format!("\"mem-{:x}\"", self.total_size()),
        };
        Box::pin(async move { Ok(Box::new(meta) as Box<dyn DavMetaData>) })
    }
}
