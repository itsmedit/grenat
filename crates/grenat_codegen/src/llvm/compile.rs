//! LLVM IR to an object file, with `clang -O3`.

use std::path::PathBuf;
use std::process::Command;

/// The `clang` to use: `GRENAT_CLANG`, else Homebrew's LLVM, else `clang`.
pub fn clang() -> PathBuf {
    if let Some(path) = std::env::var_os("GRENAT_CLANG") {
        return path.into();
    }
    ["/opt/homebrew/opt/llvm/bin/clang", "/usr/local/opt/llvm/bin/clang"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .unwrap_or_else(|| "clang".into())
}

/// Optimizes and compiles `ir` for this machine; the object file's bytes.
pub fn compile(ir: &str) -> Result<Vec<u8>, String> {
    let dir = std::env::temp_dir().join(format!("grenat-llvm-{}-{}", std::process::id(), unique()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (source, object) = (dir.join("program.ll"), dir.join("program.o"));
    std::fs::write(&source, ir).map_err(|e| e.to_string())?;
    let clang = clang();
    let out = Command::new(&clang)
        .args(["-O3", "-fPIC", "-Wno-override-module", "-c", "-x", "ir"])
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .map_err(|e| format!("cannot run {}: {e} (install LLVM, or set GRENAT_CLANG)", clang.display()))?;
    if !out.status.success() {
        let keep = std::env::var_os("GRENAT_KEEP_OBJECT").is_some();
        if !keep {
            let _ = std::fs::remove_dir_all(&dir);
        }
        return Err(format!("{} failed: {}", clang.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    let bytes = std::fs::read(&object).map_err(|e| e.to_string());
    if std::env::var_os("GRENAT_KEEP_OBJECT").is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    bytes
}

fn unique() -> usize {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
