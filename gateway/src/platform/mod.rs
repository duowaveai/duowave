//! The platform under every module: HTTP.

pub mod http;
#[cfg(feature = "pc")]
pub mod report;
#[cfg(feature = "pc")]
pub mod windows_net;
