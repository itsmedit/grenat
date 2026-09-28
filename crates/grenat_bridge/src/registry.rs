//! The bridge functions a program may call, by name: from the manifests of
//! its installed bridge facets. A facet's server starts at the first call
//! of one of its functions.

use std::collections::HashMap;
use std::sync::Arc;

use grenat_native::{Function, Outcome};

use crate::bridge::{Bridge, needs_network};
use crate::install::Installed;
use crate::process::Log;

#[derive(Debug, Default)]
pub struct Registry {
    functions: HashMap<String, Entry>,
}

#[derive(Debug)]
struct Entry {
    function: Function,
    bridge: Arc<Bridge>,
}

impl Registry {
    /// The functions of `installed`; a name exported twice is the first
    /// facet's. What the servers write on standard error goes to `log`.
    pub fn new(installed: &[Installed], log: Option<Log>) -> Registry {
        let mut functions = HashMap::new();
        for facet in installed {
            let network = needs_network(&facet.manifest);
            let bridge = Arc::new(Bridge::new(&facet.facet, &facet.dir, &facet.spec, network, log.clone()));
            for function in &facet.manifest.functions {
                functions
                    .entry(function.name.clone())
                    .or_insert_with(|| Entry { function: function.clone(), bridge: bridge.clone() });
            }
        }
        Registry { functions }
    }

    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// The function `name`, and the facet that exports it.
    pub fn function(&self, name: &str) -> Option<(&str, &Function)> {
        self.functions.get(name).map(|e| (e.bridge.facet.as_str(), &e.function))
    }

    /// Calls `name` with `args` (a JSON array), starting its server if needed.
    pub fn call(&self, name: &str, args: &[u8]) -> Result<Outcome, String> {
        let entry = self.functions.get(name).ok_or_else(|| {
            format!("no bridge provides `{name}`: install the facet that declares it (`setter install`)")
        })?;
        entry.bridge.call(name, args, entry.function.pure)
    }
}
