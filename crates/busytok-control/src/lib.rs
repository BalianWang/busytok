#![allow(warnings, clippy::all)]
#![allow(unexpected_cfgs)]
#![allow(dead_code)]
#![cfg_attr(test, allow(clippy::all))]
#![allow(clippy::uninlined_format_args)]
//! Busytok control protocol — local IPC transport for GUI/CLI communication.
//!
//! This crate provides the control layer between the Busytok service and
//! its clients (GUI, CLI). It uses Unix domain sockets with length-prefixed
//! JSON frames for IPC. It does NOT expose HTTP, SSE, WebSocket, loopback
//! compatibility endpoints, proxy listeners, provider protocol parsing,
//! route-shaped commands, or old DTO reuse.

pub mod client;
pub mod dispatch;
pub mod protocol;
pub mod server;
pub mod transport;

pub use client::ControlClient;
pub use dispatch::{ControlDispatcher, RuntimeControl, TestRuntimeControl};
pub use server::ControlServer;

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");
