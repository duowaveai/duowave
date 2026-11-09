//! Identity and rights: permissions, roles, principals and the one policy every check goes through.

pub mod permissions;
pub mod policy;
pub mod principal;
pub mod roles;

pub use policy::can;
pub use principal::{Principal, PrincipalKind, Restrictions};
pub use roles::Role;
