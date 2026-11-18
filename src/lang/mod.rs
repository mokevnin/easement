//! The code easement reads, language by language. This file only declares the
//! modules, which `easement.yml` puts in its tiers, and names the model every
//! caller reaches as `lang::…`.

pub mod adapters;
pub mod csharp;
pub mod go;
pub mod java;
pub mod kotlin;
pub mod language;
pub mod model;
pub mod parts;
pub mod php;
pub mod python;
pub mod registry;
pub mod ruby;
pub mod rust;
pub mod swift;
pub mod syntax;
pub mod typescript;

pub use model::*;
