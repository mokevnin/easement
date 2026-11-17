//! easement, an architecture quality gate, as a library: [`commands`] answers
//! each command of the binary, which only parses the arguments and prints.

mod baseline;
mod chains;
mod check;
pub mod commands;
mod components;
mod config;
mod decision;
mod diagram;
mod diff;
mod doctor;
mod drift;
mod examples;
mod explain;
mod flow;
mod graph;
mod impact;
mod implementations;
mod indexer;
mod init;
mod lang;
mod manifests;
mod matching;
mod packages;
mod patch;
mod policy;
mod project;
mod propose;
mod relpath;
mod report;
mod resolved;
mod review;
mod scan;
mod scip;
mod scope;
mod stdlib;
mod suggest;
mod suppress;
mod symbols;
mod unused;

pub use check::{Place, Platform};
pub use config::{DEFAULT_CONFIG, schema_text};
pub use graph::GraphFormat;
pub use indexer::{
    CacheUse, Environment, Indexing, Jobs, NotCached, cannot_index, default_cache_dir,
};
pub use report::{Coverage, EvidenceGap, GapKind};
pub use scan::Cache;
