//! Consent-based ownership changes share the publication lock for their package.
mod handlers;
pub use handlers::{accept, cancel, list, request};
