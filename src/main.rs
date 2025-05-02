use std::{collections::HashMap, sync::RwLock, time::Duration};
use tracing::info;
use alloy::providers::Provider;
use alloy_network::TransactionBuilder;
use anyhow::Result;
use once_cell::sync::Lazy;
use pool_sync::{Chain, PoolSync, PoolType};
use alloy::primitives::U256;


use crate::util::ignition::start_workers;
use super::calculator;
use log::LevelFilter;

pub const AMOUNT: Lazy<RwLock<U256>> = Lazy::new(|| RwLock::new(U256::from(1_000_000_000_000_000_000u128)));

// Token decimals map to convert $100k into base units
pub static TOKEN_DECIMALS: Lazy<HashMap<&'static str, u8>> = Lazy::new(|| {
    let mut map = HashMap::new();
    map.insert("USDC", 6);
    map.insert("USDT", 6);
    map.insert("WETH", 18);
    map.insert("DAI", 18);
    map
});

/// Converts a token symbol to an on-chain value in base units
pub fn amount_for_token(token_symbol: &str) -> U256 {
    let decimals = TOKEN_DECIMALS.get(token_symbol).copied().unwrap_or(18);
    let multiplier = U256::from(10).pow(U256::from(decimals as u32));
    U256::from(100_000) * multiplier // Correct conversion
}

/// Updates the global `AMOUNT` based on the token
pub fn update_amount(token_symbol: &str) {
    let calculated = amount_for_token(token_symbol);
    let amount_lock = &AMOUNT;
    let mut amount = amount_lock.write().unwrap();
    *amount = calculated;
}

/// Entry point: starts the workers and main loop
#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables and logger
    dotenv::dotenv().ok();
    env_logger::Builder::new()
        .filter_module("BaseBuster", LevelFilter::Info)
        .init();

    info!("Loading and syncing pools...");

    // Initialize pool sync across all supported AMM protocols
    let pool_sync = PoolSync::builder()
        .add_pools([
            PoolType::UniswapV2,
            PoolType::PancakeSwapV2,
            PoolType::SushiSwapV2,
            PoolType::UniswapV3,
            PoolType::SushiSwapV3,
            PoolType::BaseSwapV2,
            PoolType::BaseSwapV3,
            PoolType::Aerodrome,
            PoolType::Slipstream,
            PoolType::AlienBaseV2,
            PoolType::AlienBaseV3,
            PoolType::BaseSwapV2,
            PoolType::BaseSwapV3,
            PoolType::MaverickV1,
            PoolType::MaverickV2,
        ].into_iter())
        .chain(Chain::Base)
        .build()?;

    let (pools, last_synced_block) = pool_sync.sync_pools().await?;

    // Start async workers
    start_workers(pools, last_synced_block).await;

    // Loop to keep main thread alive if workers are spawned independently
    loop {
        tokio::time::sleep(Duration::from_secs(1000)).await;
    }

    // This is never reached unless loop is broken
    #[allow(unreachable_code)]
    Ok(())
}