//! A facet's library, loaded: its ABI version checked first, its manifest
//! read, its entry points called with JSON arguments.
//!
//! A library is loaded once per process and never unloaded: Rust code may
//! have registered thread-local destructors that must outlive it. Loading
//! the same file again (same path, size and modification time) gives the
//! same library; a library replaced on disk is loaded anew.

use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use grenat_ext::abi::{
    ABI_VERSION, Buffer, EntryFn, FREE_SYMBOL, FreeFn, MANIFEST_SYMBOL, ManifestFn, STATUS_ERROR, STATUS_OK,
    STATUS_PANIC, VERSION_SYMBOL, VersionFn,
};
use grenat_ext::manifest::Manifest;
use serde_json::Value as Json;

/// How a call ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The result, as JSON.
    Returned(Json),
    /// An error to raise in Grenat (an `Err`, arguments that did not decode).
    Raised { ty: String, message: String },
    /// A panic, caught in the library.
    Panicked(String),
}

pub struct Library {
    pub path: PathBuf,
    pub manifest: Manifest,
    handle: ManuallyDrop<libloading::Library>,
    free: FreeFn,
    /// Entry points already looked up, by symbol.
    entries: Mutex<HashMap<String, EntryFn>>,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library").field("path", &self.path).finish_non_exhaustive()
    }
}

type Key = (PathBuf, Option<SystemTime>, u64);

fn loaded() -> &'static Mutex<HashMap<Key, Arc<Library>>> {
    static LOADED: OnceLock<Mutex<HashMap<Key, Arc<Library>>>> = OnceLock::new();
    LOADED.get_or_init(Default::default)
}

impl Library {
    /// The library at `path`, loaded (once) and checked.
    pub fn open(path: &Path) -> Result<Arc<Library>, String> {
        let metadata = std::fs::metadata(path).map_err(|e| format!("cannot find {}: {e}", path.display()))?;
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let key = (canonical, metadata.modified().ok(), metadata.len());
        let mut cache = loaded().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(library) = cache.get(&key) {
            return Ok(library.clone());
        }
        let library = Arc::new(Library::load(path)?);
        cache.insert(key, library.clone());
        Ok(library)
    }

    fn load(path: &Path) -> Result<Library, String> {
        let shown = path.display();
        // SAFETY: loading runs the library's initializers: a native facet is
        // trusted by the application before it is ever loaded
        let handle = ManuallyDrop::new(
            unsafe { libloading::Library::new(path) }.map_err(|e| format!("cannot load {shown}: {e}"))?,
        );
        // SAFETY: the symbols' types are those of the ABI, whose version is checked first
        let version = unsafe { symbol::<VersionFn>(&handle, VERSION_SYMBOL) }
            .map_err(|_| format!("{shown} is not a Grenat native library (no `{VERSION_SYMBOL}`)"))?;
        // SAFETY: as above
        let version = unsafe { version() };
        if version != ABI_VERSION {
            return Err(format!(
                "{shown} was built for version {version} of Grenat's native ABI, and this Grenat speaks version \
                 {ABI_VERSION}: rebuild it with a matching grenat_ext (`setter install`)"
            ));
        }
        // SAFETY: as above, the version being the right one
        let (manifest_fn, free) =
            unsafe { (symbol::<ManifestFn>(&handle, MANIFEST_SYMBOL)?, symbol::<FreeFn>(&handle, FREE_SYMBOL)?) };
        let mut out = Buffer::empty();
        // SAFETY: a local buffer, given back to the library once read
        let (status, bytes) = unsafe {
            let status = manifest_fn(&mut out);
            let bytes = out.as_slice().to_vec();
            free(out);
            (status, bytes)
        };
        if status != STATUS_OK {
            return Err(format!("{shown}: its manifest cannot be read: {}", String::from_utf8_lossy(&bytes)));
        }
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|e| format!("{shown}: its manifest is not valid: {e}"))?;
        if manifest.abi != ABI_VERSION {
            return Err(format!("{shown}: its manifest is for version {} of the native ABI", manifest.abi));
        }
        Ok(Library { path: path.to_path_buf(), manifest, handle, free, entries: Mutex::new(HashMap::new()) })
    }

    /// Calls the function `name` with `args`, a JSON array of its arguments.
    pub fn call(&self, name: &str, args: &[u8]) -> Result<Outcome, String> {
        let entry = self.entry(name)?;
        let mut out = Buffer::empty();
        // SAFETY: the entry point's type is the ABI's; the arguments are live
        // bytes, the result is read before it is given back to the library
        let (status, bytes) = unsafe {
            let status = entry(args.as_ptr(), args.len(), &mut out);
            let bytes = out.as_slice().to_vec();
            (self.free)(out);
            (status, bytes)
        };
        let json: Json =
            serde_json::from_slice(&bytes).map_err(|e| format!("`{name}` answered something that is not JSON: {e}"))?;
        let text = |key: &str| json.get(key).and_then(Json::as_str).unwrap_or_default().to_string();
        match status {
            STATUS_OK => Ok(Outcome::Returned(json)),
            STATUS_ERROR => Ok(Outcome::Raised { ty: text("type"), message: text("message") }),
            STATUS_PANIC => Ok(Outcome::Panicked(text("message"))),
            other => Err(format!("`{name}` answered with the unknown status {other}")),
        }
    }

    /// The entry point of the function `name`, from the manifest's symbol.
    fn entry(&self, name: &str) -> Result<EntryFn, String> {
        let function = self
            .manifest
            .function(name)
            .ok_or_else(|| format!("{} exports no function `{name}`", self.path.display()))?;
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = entries.get(&function.symbol) {
            return Ok(*entry);
        }
        // SAFETY: an entry point listed by the manifest has the ABI's entry type
        let entry = unsafe { symbol::<EntryFn>(&self.handle, &function.symbol)? };
        entries.insert(function.symbol.clone(), entry);
        Ok(entry)
    }
}

/// The function `name` of `handle`, copied out (the library is never unloaded).
///
/// # Safety
/// `T` is the symbol's real type.
unsafe fn symbol<T: Copy>(handle: &libloading::Library, name: &str) -> Result<T, String> {
    // SAFETY: as the caller guarantees
    unsafe { handle.get::<T>(name.as_bytes()) }.map(|s| *s).map_err(|e| format!("no symbol `{name}`: {e}"))
}
