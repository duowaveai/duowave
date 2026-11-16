//! Identity and rights: permissions, roles, principals and the one policy every check goes through; who this gateway
//! is — its home and itself (home.rs, the one part of the core that keeps state: the settings and the secrets).

pub mod home;
pub mod permissions;
pub mod policy;
pub mod principal;
pub mod roles;

pub use policy::can;
pub use principal::{Principal, PrincipalKind, Restrictions};
pub use roles::Role;
