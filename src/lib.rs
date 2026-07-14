//! binsweep — cross-ecosystem inventory of globally installed developer
//! binaries. Scans the cargo, Go, pipx and npm global install locations,
//! attributes every executable to the package that put it there, flags
//! orphans nobody claims, and reports PATH shadowing between them.
//!
//! Every module below is pure (filesystem-in, data-out) except `cli`,
//! which owns argument parsing, environment lookups and output.

pub mod cargo;
pub mod cli;
pub mod gobin;
pub mod inventory;
pub mod json;
pub mod npm;
pub mod pipx;
pub mod report;
pub mod shadow;
pub mod util;
