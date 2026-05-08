// ABOUTME: Shared runtime surface for Barnstormer frontends.
// ABOUTME: Exposes startup configuration and server lifecycle helpers.

pub mod config;
pub mod server;

pub use config::{RuntimeConfig, RuntimeOptions};
pub use server::{ServerHandle, launch};
