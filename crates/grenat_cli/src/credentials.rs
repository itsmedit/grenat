//! `grenat credentials edit|show [--env <env>]`: the application's encrypted
//! secrets, as with Rails.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use grenat_config::credentials::{self, Location};

pub fn credentials(args: &[String]) -> ExitCode {
    let usage = || {
        eprintln!("usage: grenat credentials edit|show [--env <environment>]");
        ExitCode::from(2)
    };
    let (command, env) = match args {
        [command] => (command.as_str(), None),
        [command, flag, env] if flag == "--env" => (command.as_str(), Some(env.as_str())),
        _ => return usage(),
    };
    let root = crate::package::current().map(|p| p.root).unwrap_or_else(|_| PathBuf::from("."));
    let result = match command {
        "edit" => edit(&root, env),
        "show" => show(&root, env),
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn show(root: &Path, env: Option<&str>) -> Result<(), String> {
    let location = match env {
        Some(env) => Location::of(root, Some(env)),
        None => Location::for_env(root, &grenat_config::environment()),
    };
    if !location.exists() {
        return Err(format!("no credentials: {} (grenat credentials edit)", location.file.display()));
    }
    print!("{}", location.read()?);
    Ok(())
}

/// Decrypts into a private temporary file, opens the editor, and encrypts
/// what was saved (if it is valid YAML). Creates the credentials and their
/// key first when there are none.
fn edit(root: &Path, env: Option<&str>) -> Result<(), String> {
    let location = Location::of(root, env);
    if !location.exists() {
        location.create()?;
        eprintln!("  create  {} (keep it secret: never in the repository)", shown(root, &location.key));
        eprintln!("  create  {}", shown(root, &location.file));
        if credentials::ignore_keys(root)? {
            eprintln!("  update  .gitignore");
        }
    }
    let before = location.read()?;
    let dir = private_dir()?;
    let file = dir.join("credentials.yml");
    let result = (|| {
        std::fs::write(&file, &before).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
        run_editor(&file)?;
        let after = std::fs::read_to_string(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        if after == before {
            eprintln!("no change");
            return Ok(());
        }
        location.write(&after).map_err(|e| format!("{e}: nothing saved, edit again"))?;
        eprintln!("✓ {} saved, encrypted", shown(root, &location.file));
        Ok(())
    })();
    // the plain text never stays on disk
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// `$VISUAL`, `$EDITOR`, or `vi` — with its own arguments (`code --wait`).
fn run_editor(file: &Path) -> Result<(), String> {
    let editor = ["VISUAL", "EDITOR"].iter().find_map(|v| std::env::var(v).ok().filter(|e| !e.trim().is_empty())).unwrap_or_else(|| "vi".into());
    let mut words = editor.split_whitespace();
    let program = words.next().unwrap_or("vi");
    let status = std::process::Command::new(program)
        .args(words)
        .arg(file)
        .status()
        .map_err(|e| format!("cannot run the editor `{editor}`: {e} (set EDITOR)"))?;
    if !status.success() {
        return Err(format!("the editor `{editor}` failed: nothing saved"));
    }
    Ok(())
}

/// A new directory only its owner can read.
fn private_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("grenat-credentials-{}-{}", std::process::id(), credentials::new_key()));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    Ok(dir)
}

fn shown(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).display().to_string()
}
