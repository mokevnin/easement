//! The `packages` key: a manifest is a checked file, and each dependency it
//! declares is a dependency of it, like one a source file writes. A
//! dev-dependency is test code, so `tests: auto` takes it. What differs by kind
//! of manifest is a field of its entry in the registry
//! ([`manifest::Manifest`]), whose code sits in `packages/<kind>.rs`.

pub(crate) mod cargo;
pub mod declared;
pub mod entries;
pub(crate) mod go;
pub mod manifest;
pub(crate) mod npm;
