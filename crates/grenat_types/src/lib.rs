//! Vérification statique de Grenat : noms, types, effets et teinte `~T`.
//!
//! Le vérificateur est **graduel** : ce qu'il ne sait pas typer devient
//! `Ty::Unknown`, compatible avec tout, et ne produit jamais d'erreur. Ce
//! qu'il prouve, en revanche, il le prouve avant l'exécution :
//!
//! - **teinte** : une valeur produite par un LLM ne peut pas atteindre une
//!   fonction à effet dangereux (`shell`, `net`, `fs.write`, `human`) sans
//!   `.check`, `.approve(by: :human)` ou `.trust!` (E0412) ; l'analyse suit
//!   la teinte à travers les appels (chaque fonction est vérifiée pour la
//!   teinte réelle de ses arguments), les champs, l'interpolation, les
//!   blocs et l'état `@…` des agents ;
//! - **effets** : une fonction qui déclare `uses` doit couvrir tout ce que
//!   son corps fait, et `main` comme les `tool` doivent déclarer (E0300) ;
//! - **noms et types** : variables, champs, méthodes, arité, arguments
//!   nommés, types incompatibles (E0100, E0200) ;
//! - **déclarations** : `prompt`, agents et outils bien formés (E0413, E0500).
//!
//! L'interpréteur garde ses vérifications à l'exécution : défense en profondeur.

mod agents;
mod builtins;
mod calls;
mod checker;
mod collect;
mod construct;
mod context;
mod declarations;
mod effects;
mod expr;
mod functions;
mod methods;
mod names;
mod ops;
mod pattern;
mod resolve;
mod suggest;
mod taint;
mod ty;

pub use grenat_ast::Diagnostic;
use grenat_ast::Program;

pub(crate) use checker::*;
pub(crate) use context::*;
pub(crate) use effects::*;
pub(crate) use names::*;
pub(crate) use suggest::*;
pub(crate) use taint::*;

pub const E_NAME: &str = "E0100";
pub const E_TYPE: &str = "E0200";
pub const E_EFFECT: &str = "E0300";
pub const E_TAINT: &str = "E0412";
pub const E_TAINT_DECL: &str = "E0413";
pub const E_DECL: &str = "E0500";

/// Vérifie un programme ; renvoie les diagnostics triés par position.
pub fn check(program: &Program) -> Vec<Diagnostic> {
    let mut checker = Checker::new(program);
    checker.collect();
    checker.infer_ivars();
    // l'état `@…` teinté et les effets des fonctions récursives se propagent d'une passe à l'autre
    for _ in 0..4 {
        let before = (checker.ivar_taint.len(), checker.effect_count());
        checker.memo.clear();
        checker.pass();
        if (checker.ivar_taint.len(), checker.effect_count()) == before {
            break;
        }
    }
    let mut diags = checker.diags;
    diags.sort_by_key(|d| (d.span.start, d.span.end));
    diags
}
