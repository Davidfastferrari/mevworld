use std::str::FromStr;
use alloy::primitives::U256;
use once_cell::sync::Lazy;
use std::sync::RwLock;

/// Global amount used across modules
pub static AMOUNT: Lazy<RwLock<U256>> =
    Lazy::new(|| RwLock::new(U256::from(1_000_000_000_000_000_000u128)));
pub static U256_ONE: Lazy<U256> = Lazy::new(|| U256::from(1u64));
pub const MIN_SQRT_RATIO: u128 = 4295128739;
pub static MAX_SQRT_RATIO: Lazy<U256> = Lazy::new(|| U256::from_str("1461446703485210103287273052203988822378723970342").expect("Invalid MAX_SQRT_RATIO string"));
pub const MIN_TICK: i32 = -887272;
pub const MAX_TICK: i32 = 887272;
