//! The gateway's secrets, sealed in the database.

pub mod secret_box;
pub mod service;

pub use service::{SecretMask, SecretsService};
