use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tantivy::directory::error::{DeleteError, LockError, OpenReadError, OpenWriteError};
// Removed the redundant OwnedBytes import here
use tantivy::directory::{
    self, DirectoryLock, FileHandle, Lock, WatchCallback, WatchHandle, WritePtr,
};

use crate::error::{Result, ZdbError};

// --- Portable ReadAt Abstraction ---

trait ReadAt {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()>;
}

#[cfg(unix)]
impl ReadAt for std::fs::File {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        use std::os::unix::fs::FileExt;
        FileExt::read_exact_at(self, buf, offset)
    }
}

#[cfg(windows)]
impl ReadAt for std::fs::File {
    fn read_exact_at(&self, mut buf: &mut [u8], mut offset: u64) -> io::Result<()> {
        use std::os::windows::fs::FileExt;

        while !buf.is_empty() {
            match self.seek_read(buf, offset) {
                Ok(0) => break, // EOF reached early
                Ok(n) => {
                    let tmp = buf;
                    buf = &mut tmp[n..];
                    offset += n as u64;
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }

        if !buf.is_empty() {
            Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "failed to fill whole buffer",
            ))
        } else {
            Ok(())
        }
    }
}

// --- Directory Implementation ---

// ZIP entry metadata for direct file access
#[derive(Debug, Clone)]
struct ZipEntryInfo {
    offset: u64,
    size: u64,
}

#[derive(Clone, Debug)]
pub struct ZipDirectory {
    file: Arc<fs::File>,
    entries: Arc<HashMap<String, ZipEntryInfo>>, // Completely Lock-Free
}

impl ZipDirectory {
    pub fn open(zip_path: PathBuf) -> Result<Self> {
        let file = fs::File::open(&zip_path)
            .map_err(|e| ZdbError::general_error(format!("Failed to open zip: {}", e)))?;

        let mut archive = zip::ZipArchive::new(&file)
            .map_err(|e| ZdbError::general_error(format!("Failed to read zip: {}", e)))?;

        let mut entries = HashMap::new();
        for i in 0..archive.len() {
            if let Ok(entry) = archive.by_index(i)
                && !entry.is_dir() && entry.compression() == zip::CompressionMethod::Stored
                    && let Some(offset) = entry.data_start() {
                        let name = entry.name().to_string();
                        entries.insert(
                            name,
                            ZipEntryInfo {
                                offset,
                                size: entry.size(),
                            },
                        );
                    }
        }

        Ok(Self {
            file: Arc::new(file),
            entries: Arc::new(entries),
        })
    }

    fn get_entry_info(&self, path: &Path) -> Result<ZipEntryInfo> {
        let name = path.to_string_lossy().replace('\\', "/");
        self.entries
            .get(&name)
            .cloned()
            .ok_or_else(|| ZdbError::general_error(format!("Entry not found in zip: {}", name)))
    }

    fn has_entry(&self, path: &Path) -> bool {
        let name = path.to_string_lossy().replace('\\', "/");
        self.entries.contains_key(&name)
    }
}

#[derive(Debug)]
struct ZipFileHandle {
    file: Arc<fs::File>,
    entry_info: ZipEntryInfo,
}

impl ZipFileHandle {
    fn new(file: Arc<fs::File>, entry_info: ZipEntryInfo) -> Self {
        Self { file, entry_info }
    }
}

impl FileHandle for ZipFileHandle {
    fn read_bytes(&self, range: std::ops::Range<usize>) -> io::Result<directory::OwnedBytes> {
        if range.end > self.entry_info.size as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Range exceeds file size",
            ));
        }

        let len = range.end - range.start;
        let mut buffer = vec![0u8; len];

        // This now uses our cross-platform ReadAt trait
        self.file
            .read_exact_at(&mut buffer, self.entry_info.offset + range.start as u64)?;

        Ok(directory::OwnedBytes::new(buffer))
    }
}

impl tantivy::HasLen for ZipFileHandle {
    fn len(&self) -> usize {
        self.entry_info.size as usize
    }
}

impl directory::Directory for ZipDirectory {
    fn get_file_handle(
        &self,
        path: &Path,
    ) -> std::result::Result<Arc<dyn FileHandle>, OpenReadError> {
        let entry_info = self.get_entry_info(path).map_err(|e| {
            OpenReadError::wrap_io_error(
                io::Error::new(io::ErrorKind::NotFound, e.to_string()),
                path.to_path_buf(),
            )
        })?;

        let handle = ZipFileHandle::new(self.file.clone(), entry_info);
        Ok(Arc::new(handle))
    }

    fn delete(&self, path: &Path) -> std::result::Result<(), DeleteError> {
        Err(DeleteError::IoError {
            io_error: Arc::new(io::Error::new(
                io::ErrorKind::Unsupported,
                "ZipDirectory is read-only",
            )),
            filepath: path.to_path_buf(),
        })
    }

    fn exists(&self, path: &Path) -> std::result::Result<bool, OpenReadError> {
        Ok(self.has_entry(path))
    }

    fn open_write(&self, path: &Path) -> std::result::Result<WritePtr, OpenWriteError> {
        Err(OpenWriteError::IoError {
            io_error: Arc::new(io::Error::new(
                io::ErrorKind::Unsupported,
                "ZipDirectory is read-only",
            )),
            filepath: path.to_path_buf(),
        })
    }

    fn atomic_read(&self, path: &Path) -> std::result::Result<Vec<u8>, OpenReadError> {
        let handle = self.get_file_handle(path)?;
        let len = handle.len();
        let owned_bytes = handle
            .read_bytes(0..len)
            .map_err(|e| OpenReadError::wrap_io_error(e, path.to_path_buf()))?;
        Ok(owned_bytes.as_slice().to_vec())
    }

    fn atomic_write(&self, path: &Path, _data: &[u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("ZipDirectory is read-only: {}", path.display()),
        ))
    }

    fn sync_directory(&self) -> io::Result<()> {
        Ok(())
    }

    fn watch(&self, _watch_callback: WatchCallback) -> tantivy::Result<WatchHandle> {
        Ok(WatchHandle::empty())
    }

    fn acquire_lock(&self, _lock: &Lock) -> std::result::Result<DirectoryLock, LockError> {
        Ok(DirectoryLock::from(Box::new(())))
    }
}
