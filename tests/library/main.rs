//! easement through its library: every command answers here with data, on
//! scratch projects indexed by the real indexers. `tests/cli.rs` pins only what
//! the binary adds: the arguments, the exit codes and what it prints.

#[path = "../common/mod.rs"]
mod common;
mod run;

mod aliases;
mod answers;
mod augmentations;
mod baseline;
mod cache;
mod commands;
mod components;
mod declarations;
mod diff;
mod evidence;
mod examples;
mod exclude;
mod externals;
mod formats;
mod implementations;
mod imports;
mod indexers;
mod languages;
mod packages;
mod plan;
mod policy;
mod propose;
mod reach;
mod reexports;
mod resolved;
mod review;
mod rules;
mod sets;
mod sizes;
mod suggest;
mod surfaces;
mod symbols;
mod unused;
mod warn;
