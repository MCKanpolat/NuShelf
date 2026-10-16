//! Source-aware live keyword search. Origin counts remain separate from visible
//! results because routing, grants and duplicate IDs can reduce each page.
mod handlers;
mod model;
mod service;
pub use handlers::search;
