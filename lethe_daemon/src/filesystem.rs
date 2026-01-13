#[cfg(target_os = "windows")]
use std::ffi::{OsStr, c_void};


#[cfg(target_os = "windows")]
use widestring::U16CStr;

#[cfg(target_os = "windows")]
use winfsp::{
    filesystem::{
        DirInfo, DirMarker, FileInfo, FileSecurity, FileSystemContext,
        OpenFileInfo, VolumeInfo, WideNameInfo, 
    },
    host::{FileSystemHost, VolumeParams},
    Result,
};

#[cfg(target_os = "windows")]
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
};

#[cfg(target_os = "windows")]
pub struct LetheFS {
    readme: Vec<u8>,
}

#[cfg(target_os = "windows")]
impl LetheFS {
    pub fn new() -> Self {
        Self {
            readme: b"Welcome to Project Lethe.\r\nThis is a virtual encrypted vault."
                .to_vec(),
        }
    }
}

#[cfg(target_os = "windows")]
impl FileSystemContext for LetheFS {
    type FileContext = ();

    fn get_volume_info(&self, info: &mut VolumeInfo) -> Result<()> {
        info.total_size = 1024 * 1024 * 1024;
        info.free_size = 512 * 1024 * 1024;
        
        // FIX 1: Removed '?' because this returns &mut VolumeInfo, not Result
        info.set_volume_label(OsStr::new("Lethe Vault")); 
        
        Ok(())
    }

    fn get_security_by_name(
        &self,
        _file_name: &U16CStr,
        _security_descriptor: Option<&mut [c_void]>,
        _resolve_reparse_points: impl FnOnce(&U16CStr) -> Option<FileSecurity>,
    ) -> Result<FileSecurity> {
        Ok(FileSecurity {
            attributes: 0,
            reparse: false,
            sz_security_descriptor: 0,
        })
    }

    fn open(
        &self,
        _file_name: &U16CStr,
        _create_options: u32,
        _granted_access: u32,
        _open_file_info: &mut OpenFileInfo,
    ) -> Result<()> {
        Ok(())
    }

    fn get_file_info(
        &self,
        _context: &Self::FileContext,
        info: &mut FileInfo,
    ) -> Result<()> {
        *info = FileInfo {
            file_attributes: FILE_ATTRIBUTE_NORMAL.0,
            file_size: self.readme.len() as u64,
            allocation_size: 0,
            creation_time: 0,
            last_access_time: 0,
            last_write_time: 0,
            change_time: 0,
            index_number: 0,
            hard_links: 0,
            reparse_tag: 0,
            ea_size: 0,
        };
        Ok(())
    }

    fn read(
        &self,
        _context: &Self::FileContext,
        buffer: &mut [u8],
        offset: u64,
    ) -> Result<u32> {
        let offset = offset as usize;
        if offset >= self.readme.len() {
            return Ok(0);
        }

        let len = std::cmp::min(buffer.len(), self.readme.len() - offset);
        buffer[..len].copy_from_slice(&self.readme[offset..offset + len]);
        Ok(len as u32)
    }

    fn read_directory(
        &self,
        _context: &Self::FileContext,
        _pattern: Option<&U16CStr>,
        marker: DirMarker,
        buffer: &mut [u8],
    ) -> Result<u32> {
        let mut written = 0;

        let mut add = |name: &str, is_dir: bool| -> Option<()> {
            let mut dir_info = DirInfo::<256>::new();
            
            // 1. Set data using the standard methods
            dir_info.set_name(OsStr::new(name)).ok()?;
            dir_info.file_info_mut().file_attributes = if is_dir {
                FILE_ATTRIBUTE_DIRECTORY.0
            } else {
                FILE_ATTRIBUTE_NORMAL.0
            };

            // 2.RAW MEMORY ACCESS
            // The first 2 bytes of the DirInfo struct ALWAYS contain the size (u16).
            // We interpret the struct as a byte slice.
            let ptr = &dir_info as *const _ as *const u8;
            
            // Read the first 2 bytes to get the size (Little Endian u16)
            let size = unsafe {
                let size_bytes = std::slice::from_raw_parts(ptr, 2);
                u16::from_le_bytes([size_bytes[0], size_bytes[1]]) as usize
            };

            // 3. Safety Check
            if written + size > buffer.len() {
                return None;
            }

            // 4. Copy the exact number of bytes
            unsafe {
                let entry_slice = std::slice::from_raw_parts(ptr, size);
                buffer[written..written + size].copy_from_slice(entry_slice);
            }
            
            written += size;
            Some(())
        };

        if marker.is_none() {
            if add(".", true).is_none() { return Ok(written as u32); }
            if add("..", true).is_none() { return Ok(written as u32); }
            add("README.txt", false);
        }

        Ok(written as u32)
    }
    fn close(&self, _context: Self::FileContext) {}
}

#[cfg(target_os = "windows")]
pub fn mount_vault(mountpoint: &str) -> Result<FileSystemHost<'static>> {
    let params = VolumeParams::default();
    let fs = LetheFS::new();

    let mut host = FileSystemHost::new(params, fs)?;
    host.mount(OsStr::new(mountpoint))?;
    Ok(host)
}