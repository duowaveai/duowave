//! The providers the gateway registers on the bus (docs/plans/bus.md): apps, the gateway's services, folders and
//! files, devices. A new kind of participant is a new module here and one `bus.register` in `app.rs`.

pub mod apps;
pub mod devices;
pub mod files;
pub mod system;
