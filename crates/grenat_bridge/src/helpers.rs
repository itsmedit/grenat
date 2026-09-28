//! The helper libraries a server requires — `require "grenat/bridge"` in
//! Ruby, `import grenat_bridge` in Python — shipped with Grenat (their
//! sources are in the repository's `bridges/`), written into the installed
//! facet and put on the server's load path: `RUBYLIB`, `PYTHONPATH`. A
//! facet needs no gem and no pip package.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// `bridges/ruby/lib/grenat/bridge.rb`.
pub const RUBY: &str = include_str!("../../../bridges/ruby/lib/grenat/bridge.rb");
/// `bridges/python/grenat_bridge.py`.
pub const PYTHON: &str = include_str!("../../../bridges/python/grenat_bridge.py");

/// (path in the helpers' directory, text).
const FILES: [(&str, &str); 2] = [("ruby/grenat/bridge.rb", RUBY), ("python/grenat_bridge.py", PYTHON)];

/// Writes the helper libraries in `lib`, unless they are there already
/// (as this Grenat ships them): through a file renamed into place, so that
/// a process starting meanwhile never reads half a file.
pub fn write(lib: &Path) -> Result<(), String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    for (file, text) in FILES {
        let path = lib.join(file);
        if std::fs::read_to_string(&path).is_ok_and(|current| current == text) {
            continue;
        }
        let dir = path.parent().expect("a file in a directory");
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let partial = dir.join(format!(".partial-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::write(&partial, text).map_err(|e| format!("cannot write {}: {e}", partial.display()))?;
        std::fs::rename(&partial, &path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(())
}

/// The variables that put the helpers of `lib` on the load path.
pub fn env(lib: &Path) -> Vec<(String, String)> {
    let path = |sub: &str| lib.join(sub).to_string_lossy().into_owned();
    vec![
        ("RUBYLIB".into(), path("ruby")),
        ("PYTHONPATH".into(), path("python")),
        // no `__pycache__` in the facet, and standard error as it is written
        ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
        ("PYTHONUNBUFFERED".into(), "1".into()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helpers_are_written_once_and_put_on_the_load_path() {
        let lib = std::env::temp_dir().join(format!("grenat-bridge-helpers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&lib);
        write(&lib).unwrap();
        assert_eq!(std::fs::read_to_string(lib.join("ruby/grenat/bridge.rb")).unwrap(), RUBY);
        assert_eq!(std::fs::read_to_string(lib.join("python/grenat_bridge.py")).unwrap(), PYTHON);
        // an outdated copy is replaced; nothing is left behind
        std::fs::write(lib.join("python/grenat_bridge.py"), "old").unwrap();
        write(&lib).unwrap();
        assert_eq!(std::fs::read_to_string(lib.join("python/grenat_bridge.py")).unwrap(), PYTHON);
        let names: Vec<String> = std::fs::read_dir(lib.join("python"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["grenat_bridge.py"]);
        let env = env(&lib);
        assert_eq!(env[0], ("RUBYLIB".to_string(), lib.join("ruby").to_string_lossy().into_owned()));
        assert_eq!(env[1], ("PYTHONPATH".to_string(), lib.join("python").to_string_lossy().into_owned()));
        let _ = std::fs::remove_dir_all(&lib);
    }
}
