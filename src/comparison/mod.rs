//! Persistent package comparisons, with bounded archive inspection and APICompat.
mod analyzer;
mod archive;
mod handlers;
mod reference;
mod worker;
pub use handlers::{create, details, list};
pub use worker::worker;
