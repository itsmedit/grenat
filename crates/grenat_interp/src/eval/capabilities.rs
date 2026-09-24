//! Capacités déclarées par `uses` et leur application aux accès disque.

use crate::prelude::*;

/// Normalisation lexicale d'un chemin : `./docs/../docs/a` → `docs/a`.
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

/// `declared` (restriction de `uses`) autorise-t-il l'accès à `path` ?
fn path_allowed(declared: &str, path: &str) -> bool {
    let (d_abs, d) = path_components(declared);
    let (p_abs, p) = path_components(path);
    d_abs == p_abs && !p.first().is_some_and(|c| c == "..") && p.starts_with(&d)
}

impl<'p> Interp<'p> {
    /// Capacités déclarées par `uses` (restrictions évaluées) ; `None` si rien n'est déclaré.
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

    /// Vérifie qu'un accès disque est couvert par chaque fonction de la pile qui déclare ses effets.
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
                    format!("`{effect}` sur `{path}` n'est pas autorisé par `{owner}` (uses {})", declared.join(", ")),
                );
            }
        }
        Ok(())
    }
}
