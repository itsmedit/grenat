//! The native functions a program may call, by name: from the manifests of
//! its installed native facets. A library is loaded on the first call of one
//! of its functions, once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use grenat_ext::manifest::Function;

use crate::install::Installed;
use crate::library::{Library, Outcome};

#[derive(Debug, Default)]
pub struct Registry {
    functions: HashMap<String, Entry>,
}

#[derive(Debug)]
struct Entry {
    facet: String,
    function: Function,
    source: Arc<Source>,
}

#[derive(Debug)]
struct Source {
    path: PathBuf,
    library: OnceLock<Result<Arc<Library>, String>>,
}

impl Registry {
    /// The functions of `installed`; a name exported twice is the first facet's.
    pub fn new(installed: &[Installed]) -> Registry {
        let mut functions = HashMap::new();
        for facet in installed {
            let source = Arc::new(Source { path: facet.library.clone(), library: OnceLock::new() });
            for function in &facet.manifest.functions {
                functions.entry(function.name.clone()).or_insert_with(|| Entry {
                    facet: facet.facet.clone(),
                    function: function.clone(),
                    source: source.clone(),
                });
            }
        }
        Registry { functions }
    }

    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// The function `name`, and the facet that exports it.
    pub fn function(&self, name: &str) -> Option<(&str, &Function)> {
        self.functions.get(name).map(|e| (e.facet.as_str(), &e.function))
    }

    /// Calls `name` with `args` (a JSON array), loading its library if needed.
    pub fn call(&self, name: &str, args: &[u8]) -> Result<Outcome, String> {
        let entry = self.functions.get(name).ok_or_else(|| {
            format!("no native library provides `{name}`: install the facet that declares it (`setter install`)")
        })?;
        let library = entry.source.library.get_or_init(|| Library::open(&entry.source.path));
        library.as_ref().map_err(|e| format!("facet `{}`: {e}", entry.facet))?.call(name, args)
    }
}
