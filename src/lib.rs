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

pub use alloy;

pub use revm;

// Declare calculation module
pub mod calculation;

// Declare additional modules to fix unresolved imports
pub mod cache;
pub mod market_state;
pub mod swap;
pub mod tracing;
pub mod rgen;

pub mod tx_sender;
pub mod stream;
pub mod simulator;
pub mod searcher;
pub mod history_db;
pub mod qouter;
pub mod graph;
pub mod gas_station;
pub mod filters;
pub mod events;
pub mod estimator;
pub mod constants;
pub mod bytecode;
pub mod market_state;
pub mod main;
pub mod ignition;
// Re-export Calculator for easier import
// pub use crate::calculation::Calculator;
pub mod calculation::Calculator;
pub const AMOUNT: Lazy<RwLock<U256>> = Lazy::new(|| RwLock::new(U256::from(1_000_000_000_000_000_000u128)));
