//! The platform — the computer's part, added to «Основание»'s `platform` under its name: whether others reach this
//! computer on Windows (windows_net.rs) and the report of what went wrong, for the owner to send (report.rs).

pub use duowave_base::platform::*;

pub mod report;
pub mod windows_net;
