//! Capabilities declared by `uses` and their enforcement on file system and
//! network access.

use crate::prelude::*;

/// Lexical path normalization: `./docs/../docs/a` → `docs/a`.
fn path_components(path: &str) -> (bool, Vec<String>) {
    let absolute = path.starts_with('/');
    let mut parts: Vec<String> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| p != "..") => {
                parts.pop();
            }
            other => parts.push(other.to_string()),
        }
    }
    (absolute, parts)
}

/// Does `declared` (a `uses` restriction) allow access to `path`?
fn path_allowed(declared: &str, path: &str) -> bool {
    let (d_abs, d) = path_components(declared);
    let (p_abs, p) = path_components(path);
    d_abs == p_abs && !p.first().is_some_and(|c| c == "..") && p.starts_with(&d)
}

impl<'p> Interp<'p> {
    /// Capabilities declared by `uses` (evaluated restrictions); `None` if nothing is declared.
    pub(crate) fn declared_capabilities(&mut self, def: &'p FnDef) -> Result<Option<crate::Capabilities>, Ctrl<'p>> {
        if def.effects.is_empty() {
            return Ok(None);
        }
        let mut caps = Vec::new();
        for effect in &def.effects {
            let path = effect.path.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(".");
            let arg = match effect.args.first() {
                Some(e) => Some(self.eval(e)?.to_display()),
                None => None,
            };
            caps.push((path, arg));
        }
        Ok(Some(caps))
    }

    /// Checks that a file system access is covered by every function on the stack that declares its effects.
    pub(crate) fn check_fs(&self, effect: &str, path: &str) -> Result<(), Ctrl<'p>> {
        for (owner, caps) in &self.capabilities {
            let allowed = caps.iter().any(|(declared, arg)| {
                let covers = declared == effect || effect.starts_with(&format!("{declared}."));
                covers && arg.as_deref().is_none_or(|prefix| path_allowed(prefix, path))
            });
            if !allowed {
                let declared: Vec<String> =
                    caps.iter().map(|(p, a)| a.as_ref().map_or(p.clone(), |a| format!("{p}(\"{a}\")"))).collect();
                return raise(
                    "CapabilityError",
                    format!("`{effect}` on `{path}` is not allowed by `{owner}` (uses {})", declared.join(", ")),
                );
            }
        }
        Ok(())
    }

    /// Checks that a request to `host` is covered by every function on the
    /// stack that declares its effects: `net`, or `net("<host>")`.
    pub(crate) fn check_net(&self, host: &str, url: &str) -> Result<(), Ctrl<'p>> {
        for (owner, caps) in &self.capabilities {
            let allowed = caps.iter().any(|(declared, arg)| declared == "net" && arg.as_deref().is_none_or(|h| h == host));
            if !allowed {
                let declared: Vec<String> =
                    caps.iter().map(|(p, a)| a.as_ref().map_or(p.clone(), |a| format!("{p}(\"{a}\")"))).collect();
                return raise(
                    "CapabilityError",
                    format!("`net` to `{url}` is not allowed by `{owner}` (uses {})", declared.join(", ")),
                );
            }
        }
        Ok(())
    }

    /// Checks that `effect` (`db.read`…) is declared by every function on
    /// the stack that declares its effects.
    pub(crate) fn check_effect(&self, effect: &str) -> Result<(), Ctrl<'p>> {
        for (owner, caps) in &self.capabilities {
            if !caps.iter().any(|(declared, _)| declared == effect || effect.starts_with(&format!("{declared}."))) {
                let declared: Vec<&str> = caps.iter().map(|(p, _)| p.as_str()).collect();
                return raise(
                    "CapabilityError",
                    format!("`{effect}` is not allowed by `{owner}` (uses {})", declared.join(", ")),
                );
            }
        }
        Ok(())
    }

    /// Checks that running `program` is covered by every function on the
    /// stack that declares its effects: `shell`, or `shell("<program>")`.
    pub(crate) fn check_program(&self, program: &str) -> Result<(), Ctrl<'p>> {
        for (owner, caps) in &self.capabilities {
            let allowed =
                caps.iter().any(|(declared, arg)| declared == "shell" && arg.as_deref().is_none_or(|p| p == program));
            if !allowed {
                let declared: Vec<String> =
                    caps.iter().map(|(p, a)| a.as_ref().map_or(p.clone(), |a| format!("{p}(\"{a}\")"))).collect();
                return raise(
                    "CapabilityError",
                    format!("running `{program}` is not allowed by `{owner}` (uses {})", declared.join(", ")),
                );
            }
        }
        Ok(())
    }

    /// Checks that using the MCP server `name` is covered by every function
    /// on the stack that declares its effects: `mcp`, or `mcp("<name>")`.
    pub(crate) fn check_mcp(&self, name: &str) -> Result<(), Ctrl<'p>> {
        for (owner, caps) in &self.capabilities {
            if !caps.iter().any(|(declared, arg)| declared == "mcp" && arg.as_deref().is_none_or(|s| s == name)) {
                let declared: Vec<String> =
                    caps.iter().map(|(p, a)| a.as_ref().map_or(p.clone(), |a| format!("{p}(\"{a}\")"))).collect();
                return raise(
                    "CapabilityError",
                    format!("the MCP server `{name}` is not allowed by `{owner}` (uses {})", declared.join(", ")),
                );
            }
        }
        Ok(())
    }
}
