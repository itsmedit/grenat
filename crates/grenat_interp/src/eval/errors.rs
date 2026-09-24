//! Built-in error types and the error hierarchy.

/// Built-in error types, usable without a declaration (`raise ApprovalDenied`).
const ERROR_NAMES: &[&str] = &[
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

pub(crate) fn error_is_a(ty: &str, target: &str) -> bool {
    ty == target || matches!(target, "StandardError" | "Exception") || (ty == "LlmRefusal" && target == "LlmError")
}
