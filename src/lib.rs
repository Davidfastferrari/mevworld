//! Library root for mevworld crate.

// Import macros for logging
#[macro_use]
extern crate log;

// Import tracing macros
#[macro_use]
extern crate tracing;

// Import lazy_static macro
#[macro_use]
extern crate lazy_static;

// Import alloy sol macro
pub use alloy_sol_types::sol;

// Declare calculation module
pub mod calculation;

// Re-export Calculator for easier import
pub use crate::calculation::Calculator;
