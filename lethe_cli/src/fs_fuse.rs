#![cfg(unix)]

use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyWrite, ReplyCreate, ReplyEmpty, Request, TimeOrNow,
};
use libc::{ENOENT, EACCES};
use std::ffi::OsStr;
use std::time::{Duration, UNIX_EPOCH, SystemTime};
use std::collections::{HashMap, HashSet};
use lethe_core::index::IndexManager;
use lethe_core::storage::BlockManager;
use lethe_core::crypto::MasterKey;

const TTL: Duration = Duration::from_secs(1);

pub struct LetheFS {
    pub index: IndexManager,
    pub storage: BlockManager,
    pub key: MasterKey,
    pub inode_map: HashMap<u64, String>,

    // WRITE BUFFER: Inode -> File Content (in RAM)
    // We buffer writes here until the file is closed (Release)
    pub write_buffer: HashMap<u64, Vec<u8>>,
}

impl LetheFS {
    fn get_file_attr(&self, path: &str, ino: u64) -> FileAttr {
        // Root Directory
        if path == "/" {
            return FileAttr {
                ino: 1,
                size: 0,
                blocks: 0,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::Directory,
                perm: 0o755,
                nlink: 2,
                uid: 1000,
                gid: 1000,
                rdev: 0,
                flags: 0,
                blksize: 512,
            };
        }

        // Implicit Directories (if path is in inode_map but not in index)
        // Check if it is a file in the index OR currently being written
        let is_file =
            self.index.data.files.contains_key(path) || self.write_buffer.contains_key(&ino);

        if !is_file {
            return FileAttr {
                ino,
                size: 0,
                blocks: 0,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::Directory,
                perm: 0o755,
                nlink: 2,
                uid: 1000,
                gid: 1000,
                rdev: 0,
                flags: 0,
                blksize: 512,
            };
        }

        // Regular File (From Index)
        if let Some(entry) = self.index.get_file(path) {
            return FileAttr {
                ino,
                size: entry.size,
                blocks: 1,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::RegularFile,
                perm: 0o644,
                nlink: 1,
                uid: 1000,
                gid: 1000,
                rdev: 0,
                flags: 0,
                blksize: 512,
            };
        }

        // Regular File (Currently being written - size is buffer size)
        if let Some(buffer) = self.write_buffer.get(&ino) {
            return FileAttr {
                ino,
                size: buffer.len() as u64,
                blocks: 1,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::RegularFile,
                perm: 0o644,
                nlink: 1,
                uid: 1000,
                gid: 1000,
                rdev: 0,
                flags: 0,
                blksize: 512,
            };
        }

        // Not Found
        FileAttr {
            ino,
            size: 0,
            blocks: 0,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::RegularFile,
            perm: 0o000,
            nlink: 0,
            uid: 0,
            gid: 0,
            rdev: 0,
            flags: 0,
            blksize: 0,
        }
    }
}

impl Filesystem for LetheFS {
    // 1. LOOKUP
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = name.to_string_lossy();

        if let Some(parent_path) = self.inode_map.get(&parent) {
            let child_path = if parent_path == "/" {
                format!("/{}", name_str)
            } else {
                format!("{}/{}", parent_path, name_str)
            };

            let ino = fxhash::hash64(&child_path);

            if self.inode_map.contains_key(&ino) || self.write_buffer.contains_key(&ino) {
                self.inode_map.insert(ino, child_path.clone());
                reply.entry(&TTL, &self.get_file_attr(&child_path, ino), 0);
                return;
            }
        }
        reply.error(ENOENT);
    }

    // 2. GET ATTR
    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if let Some(path) = self.inode_map.get(&ino) {
            reply.attr(&TTL, &self.get_file_attr(path, ino));
        } else if ino == 1 {
            reply.attr(&TTL, &self.get_file_attr("/", 1));
        } else {
            reply.error(ENOENT);
        }
    }

    // 3. SET ATTR
    fn setattr(
        &mut self,
        _req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        if let Some(path) = self.inode_map.get(&ino).cloned() {
            if let Some(new_size) = size {
                if let Some(buffer) = self.write_buffer.get_mut(&ino) {
                    buffer.resize(new_size as usize, 0);
                }
            }
            reply.attr(&TTL, &self.get_file_attr(&path, ino));
        } else {
            reply.error(ENOENT);
        }
    }

    // 4. READ DIR
    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let dir_path = match self.inode_map.get(&ino) {
            Some(p) => p.clone(),
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let mut entries = vec![
            (ino, FileType::Directory, ".".to_string()),
            (ino, FileType::Directory, "..".to_string()),
        ];

        let mut seen = HashSet::new();

        for (child_ino, child_path) in &self.inode_map {
            let is_child = if dir_path == "/" {
                child_path.starts_with('/') && child_path.matches('/').count() == 1
            } else {
                child_path.starts_with(&dir_path)
                    && child_path.len() > dir_path.len()
                    && child_path.chars().nth(dir_path.len()) == Some('/')
                    && child_path[dir_path.len() + 1..].matches('/').count() == 0
            };

            if is_child {
                let name = if dir_path == "/" {
                    child_path.trim_start_matches('/').to_string()
                } else {
                    child_path
                        .strip_prefix(&format!("{}/", dir_path))
                        .unwrap_or("")
                        .to_string()
                };

                if !name.is_empty() && !seen.contains(&name) {
                    seen.insert(name.clone());
                    let kind = if self.index.data.files.contains_key(child_path) {
                        FileType::RegularFile
                    } else {
                        FileType::Directory
                    };
                    entries.push((*child_ino, kind, name));
                }
            }
        }

        for (i, (inode, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(inode, (i + 1) as i64, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    // 5. CREATE
    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let name_str = name.to_string_lossy();
        if let Some(parent_path) = self.inode_map.get(&parent).cloned() {
            let child_path = if parent_path == "/" {
                format!("/{}", name_str)
            } else {
                format!("{}/{}", parent_path, name_str)
            };

            let ino = fxhash::hash64(&child_path);

            self.inode_map.insert(ino, child_path.clone());
            self.write_buffer.insert(ino, Vec::new());

            reply.created(&TTL, &self.get_file_attr(&child_path, ino), 0, 0, 0);
        } else {
            reply.error(ENOENT);
        }
    }

    // 6. WRITE
    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        if let Some(buffer) = self.write_buffer.get_mut(&ino) {
            let end = offset as usize + data.len();
            if end > buffer.len() {
                buffer.resize(end, 0);
            }
            buffer[offset as usize..end].copy_from_slice(data);
            reply.written(data.len() as u32);
        } else {
            reply.error(ENOENT);
        }
    }

    // 7. READ
    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        if let Some(buffer) = self.write_buffer.get(&ino) {
            let data_len = buffer.len() as u64;
            if offset as u64 >= data_len {
                reply.data(&[]);
                return;
            }
            let end = std::cmp::min((offset as u64 + size as u64) as usize, buffer.len());
            reply.data(&buffer[offset as usize..end]);
            return;
        }

        if let Some(path) = self.inode_map.get(&ino) {
            if let Some(entry) = self.index.get_file(path) {
                let mut full_data = Vec::new();
                for block_id in &entry.blocks {
                    if let Ok(mut chunk) = self.storage.read_block(block_id, &self.key) {
                        full_data.append(&mut chunk);
                    }
                }
                let data_len = full_data.len() as u64;
                if offset as u64 >= data_len {
                    reply.data(&[]);
                    return;
                }
                let end =
                    std::cmp::min((offset as u64 + size as u64) as usize, full_data.len());
                reply.data(&full_data[offset as usize..end]);
            } else {
                reply.error(ENOENT);
            }
        } else {
            reply.error(ENOENT);
        }
    }

    // 8. RELEASE
    fn release(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        if let Some(data) = self.write_buffer.remove(&ino) {
            if let Some(path) = self.inode_map.get(&ino).cloned() {
                if let Ok(block_id) = self.storage.write_block(&data, &self.key) {
                    self.index
                        .add_file(path.clone(), vec![block_id], data.len() as u64);
                    let _ = self.index.save(&self.key);
                }
            }
        }
        reply.ok();
    }

    // 9. UNLINK
    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name_str = name.to_string_lossy();

        if let Some(parent_path) = self.inode_map.get(&parent).cloned() {
            let file_path = if parent_path == "/" {
                format!("/{}", name_str)
            } else {
                format!("{}/{}", parent_path, name_str)
            };

            if self.index.data.files.remove(&file_path).is_some() {
                let ino = fxhash::hash64(&file_path);
                self.inode_map.remove(&ino);
                self.write_buffer.remove(&ino);
                let _ = self.index.save(&self.key);
                reply.ok();
                return;
            }
        }
        reply.error(ENOENT);
    }

    // 10. RMDIR
    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name_str = name.to_string_lossy();

        if let Some(parent_path) = self.inode_map.get(&parent).cloned() {
            let dir_path = if parent_path == "/" {
                format!("/{}", name_str)
            } else {
                format!("{}/{}", parent_path, name_str)
            };

            let is_empty = !self.index.data.files.keys().any(|k| {
                k.starts_with(&dir_path)
                    && k.len() > dir_path.len()
                    && k.chars().nth(dir_path.len()) == Some('/')
            });

            if is_empty {
                let ino = fxhash::hash64(&dir_path);
                self.inode_map.remove(&ino);
                reply.ok();
            } else {
                reply.error(libc::ENOTEMPTY);
            }
        } else {
            reply.error(ENOENT);
        }
    }

    // 11. RENAME
    fn rename(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        let name_str = name.to_string_lossy();
        let newname_str = newname.to_string_lossy();

        let old_parent = self.inode_map.get(&parent).cloned();
        let new_parent = self.inode_map.get(&newparent).cloned();

        if let (Some(old_p), Some(new_p)) = (old_parent, new_parent) {
            let old_path = if old_p == "/" {
                format!("/{}", name_str)
            } else {
                format!("{}/{}", old_p, name_str)
            };

            let new_path = if new_p == "/" {
                format!("/{}", newname_str)
            } else {
                format!("{}/{}", new_p, newname_str)
            };

            if let Some(entry) = self.index.data.files.remove(&old_path) {
                self.index.data.files.insert(new_path.clone(), entry);

                let old_ino = fxhash::hash64(&old_path);
                let new_ino = fxhash::hash64(&new_path);

                self.inode_map.remove(&old_ino);
                self.inode_map.insert(new_ino, new_path);

                let _ = self.index.save(&self.key);
                reply.ok();
            } else {
                reply.error(ENOENT);
            }
        } else {
            reply.error(ENOENT);
        }
    }
}
