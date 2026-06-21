pub mod dto;
pub mod methods;
pub mod ts;

pub use dto::*;
pub use methods::method_manifest;

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
