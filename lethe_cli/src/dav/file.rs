use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::time::SystemTime;
use bytes::{Buf, Bytes};
use dav_server::fs::{DavFile, DavMetaData, FsError, FsFuture, FsResult};
use super::state::LetheState;

#[derive(Debug, Clone)]
pub struct LetheMetaData {
    pub len: u64,
    pub modified: SystemTime,
    pub is_dir: bool,
    pub etag: String,
}

impl DavMetaData for LetheMetaData {
    fn len(&self) -> u64 { self.len }
    fn modified(&self) -> FsResult<SystemTime> { Ok(self.modified) }
    fn is_dir(&self) -> bool { self.is_dir }
    fn etag(&self) -> Option<String> { Some(self.etag.clone()) }
}

#[derive(Debug)]
pub struct LetheDavFile {
    pub buffer: Cursor<Vec<u8>>, 
    pub path: String,
    pub state: LetheState,
    pub is_dirty: bool,
}

impl DavFile for LetheDavFile {
    fn read_bytes(&mut self, count: usize) -> FsFuture<'_, Bytes> {
        let mut buf = vec![0u8; count];
        match self.buffer.read(&mut buf) {
            Ok(n) => {
                buf.truncate(n);
                Box::pin(async move { Ok(Bytes::from(buf)) })
            }
            Err(_) => Box::pin(async { Err(FsError::GeneralFailure) }),
        }
    }

    fn write_buf(&mut self, mut buf: Box<dyn Buf + Send>) -> FsFuture<'_, ()> {
        let mut chunk = vec![0u8; buf.remaining()];
        buf.copy_to_slice(&mut chunk);
        match self.buffer.write_all(&chunk) {
            Ok(_) => {
                self.is_dirty = true;
                Box::pin(async { Ok(()) })
            }
            Err(_) => Box::pin(async { Err(FsError::GeneralFailure) }),
        }
    }

    fn write_bytes(&mut self, buf: Bytes) -> FsFuture<'_, ()> {
        self.write_buf(Box::new(buf))
    }

    fn seek(&mut self, pos: SeekFrom) -> FsFuture<'_, u64> {
        let res = self.buffer.seek(pos).map_err(|_| FsError::GeneralFailure);
        Box::pin(async move { res })
    }

    fn flush(&mut self) -> FsFuture<'_, ()> {
        let path = self.path.clone();
        let data = self.buffer.get_ref().clone();
        let state = self.state.clone();
        let is_dirty = self.is_dirty;

        Box::pin(async move {
            if !is_dirty { return Ok(()); }
            let size = data.len() as u64;
            let block_id = match state.storage.write_block(&data, &state.key) {
                Ok(id) => id,
                Err(_) => return Err(FsError::GeneralFailure),
            };
            let mut index = state.index.lock().await;
            index.add_file(path, vec![block_id], size);
            match index.save(&state.key) {
                Ok(_) => Ok(()),
                Err(_) => Err(FsError::GeneralFailure),
            }
        })
    }

    fn metadata(&mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        let len = self.buffer.get_ref().len() as u64;
        let modified = SystemTime::now();
        let etag = format!("\"mem-{:x}\"", len);
        Box::pin(async move {
            Ok(Box::new(LetheMetaData {
                len, modified, is_dir: false, etag
            }) as Box<dyn DavMetaData>)
        })
    }
}