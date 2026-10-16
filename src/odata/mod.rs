//! Legacy NuGet OData V2 protocol, query validation, SQL persistence and Atom encoding.
mod application;
mod filter;
mod frameworks;
mod handlers;
mod model;
mod protocol;
mod query;
mod repository;
pub use handlers::{download, endpoint, metadata, service};
