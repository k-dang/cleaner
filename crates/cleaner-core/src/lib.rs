//! Portable cleanup workflow, Target metadata, Selection rules, and results.
//! Native apps supply their catalog and opaque Scan snapshots. This crate does
//! not access the filesystem or depend on a windowing or operating-system library.

pub mod controller;
pub mod results;
pub mod selection;
pub mod targets;
