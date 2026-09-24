//! Noms intégrés (erreurs) et accès aux noms de types.

use grenat_ast::Type;

pub(crate) const ERROR_NAMES: &[&str] = &[
    "Exception",
    "StandardError",
    "BudgetExceeded",
    "ApprovalDenied",
    "LlmRefusal",
    "MaxTurnsExceeded",
    "NoMatchingPattern",
    "AgentDown",
    "Cancelled",
];

pub(crate) fn is_error_name(name: &str) -> bool {
    name.ends_with("Error") || ERROR_NAMES.contains(&name)
}

pub(crate) fn type_name(ty: &Type) -> &str {
    match ty {
        Type::Named { path, .. } => &path.last().expect("chemin non vide").name,
        Type::Optional(inner, _) | Type::Tainted(inner, _) => type_name(inner),
    }
}
