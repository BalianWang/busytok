#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
pub mod dto;
pub mod methods;
pub mod ts;

pub use dto::*;
pub use methods::method_manifest;

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
