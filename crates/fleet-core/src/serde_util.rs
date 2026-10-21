//! The `#[serde(default = …, skip_serializing_if = …)]` predicates every
//! wire and row type shares, written once.

/// `#[serde(default = "crate::serde_util::default_true")]`.
pub(crate) fn default_true() -> bool {
    true
}

/// `#[serde(skip_serializing_if = "crate::serde_util::is_false")]`.
pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

/// `#[serde(skip_serializing_if = "crate::serde_util::is_zero_u32")]`.
pub(crate) fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}
