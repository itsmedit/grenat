//! The test server's SFTP subsystem: the real files of one directory
//! (`/x` and `x` both lead to `<root>/x`; `..` is refused).

use std::collections::HashMap;
use std::fs::{self, File as FsFile, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use russh_sftp::protocol::{Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version};

pub struct Files {
    root: PathBuf,
    handles: HashMap<String, Open>,
    next: u64,
}

enum Open {
    File(FsFile),
    /// A listing, and whether it was sent already.
    Dir(Vec<File>, bool),
}

impl Files {
    pub fn new(root: &Path) -> Files {
        Files { root: root.to_path_buf(), handles: HashMap::new(), next: 0 }
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, StatusCode> {
        let relative = Path::new(path.trim_start_matches('/'));
        if relative.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(StatusCode::PermissionDenied);
        }
        Ok(self.root.join(relative))
    }

    fn keep(&mut self, open: Open) -> String {
        self.next += 1;
        let handle = format!("h{}", self.next);
        self.handles.insert(handle.clone(), open);
        handle
    }

    fn file(&mut self, handle: &str) -> Result<&mut FsFile, StatusCode> {
        match self.handles.get_mut(handle) {
            Some(Open::File(file)) => Ok(file),
            _ => Err(StatusCode::Failure),
        }
    }
}

fn status(e: std::io::Error) -> StatusCode {
    match e.kind() {
        ErrorKind::NotFound => StatusCode::NoSuchFile,
        ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

fn ok(id: u32) -> Status {
    Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() }
}

impl russh_sftp::server::Handler for Files {
    type Error = StatusCode;

    fn unimplemented(&self) -> StatusCode {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _version: u32, _extensions: HashMap<String, String>) -> Result<Version, StatusCode> {
        Ok(Version::new())
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        flags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, StatusCode> {
        let path = self.resolve(&filename)?;
        let mut options = OpenOptions::new();
        options
            .read(flags.contains(OpenFlags::READ))
            .write(flags.contains(OpenFlags::WRITE) || flags.contains(OpenFlags::APPEND))
            .append(flags.contains(OpenFlags::APPEND))
            .truncate(flags.contains(OpenFlags::TRUNCATE));
        if flags.contains(OpenFlags::EXCLUDE) {
            options.create_new(true);
        } else if flags.contains(OpenFlags::CREATE) {
            options.create(true);
        }
        let file = options.open(path).map_err(status)?;
        Ok(Handle { id, handle: self.keep(Open::File(file)) })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, StatusCode> {
        self.handles.remove(&handle).ok_or(StatusCode::Failure)?;
        Ok(ok(id))
    }

    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<Data, StatusCode> {
        let file = self.file(&handle)?;
        file.seek(SeekFrom::Start(offset)).map_err(status)?;
        let mut data = vec![0u8; len as usize];
        let n = file.read(&mut data).map_err(status)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        data.truncate(n);
        Ok(Data { id, data })
    }

    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, StatusCode> {
        let file = self.file(&handle)?;
        file.seek(SeekFrom::Start(offset)).map_err(status)?;
        file.write_all(&data).map_err(status)?;
        Ok(ok(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        let metadata = fs::metadata(self.resolve(&path)?).map_err(status)?;
        Ok(Attrs { id, attrs: FileAttributes::from(&metadata) })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
        let metadata = fs::symlink_metadata(self.resolve(&path)?).map_err(status)?;
        Ok(Attrs { id, attrs: FileAttributes::from(&metadata) })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, StatusCode> {
        let metadata = self.file(&handle)?.metadata().map_err(status)?;
        Ok(Attrs { id, attrs: FileAttributes::from(&metadata) })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, StatusCode> {
        let dir = self.resolve(&path)?;
        let mut files = vec![File::new(".", FileAttributes::dummy()), File::new("..", FileAttributes::dummy())];
        for entry in fs::read_dir(dir).map_err(status)? {
            let entry = entry.map_err(status)?;
            let metadata = entry.metadata().map_err(status)?;
            files.push(File::new(entry.file_name().to_string_lossy(), FileAttributes::from(&metadata)));
        }
        Ok(Handle { id, handle: self.keep(Open::Dir(files, false)) })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, StatusCode> {
        match self.handles.get_mut(&handle) {
            Some(Open::Dir(files, sent)) if !*sent => {
                *sent = true;
                Ok(Name { id, files: files.clone() })
            }
            Some(Open::Dir(..)) => Err(StatusCode::Eof),
            _ => Err(StatusCode::Failure),
        }
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, StatusCode> {
        fs::remove_file(self.resolve(&filename)?).map_err(status)?;
        Ok(ok(id))
    }

    async fn mkdir(&mut self, id: u32, path: String, _attrs: FileAttributes) -> Result<Status, StatusCode> {
        fs::create_dir(self.resolve(&path)?).map_err(status)?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, StatusCode> {
        fs::remove_dir(self.resolve(&path)?).map_err(status)?;
        Ok(ok(id))
    }

    async fn rename(&mut self, id: u32, from: String, to: String) -> Result<Status, StatusCode> {
        fs::rename(self.resolve(&from)?, self.resolve(&to)?).map_err(status)?;
        Ok(ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, StatusCode> {
        let absolute = format!("/{}", path.trim_start_matches('/').trim_start_matches("./"));
        Ok(Name { id, files: vec![File::dummy(if path == "." { "/".into() } else { absolute })] })
    }
}
