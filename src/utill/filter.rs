use alloy::primitives::{Address, U160, U256, address};
use alloy_sol_types::{SolCall, SolValue};
use std::{
    collections::HashMap,
    fs::{File, create_dir_all},
    io::{BufReader, BufWriter},
    path::Path,
    str::FromStr,
};
// Added explicit import to bring abi_encode and abi_decode into scope
use super::constant::AMOUNT;
use super::node_db::InsertionType::NodeInsertionType;
use super::node_db::NodeDB;
use super::rgen::ERC20Token::approveCall;
use super::rgen::{V2Aerodrome, V2Swap, V3Swap, V3SwapDeadline, V3SwapDeadlineTick};
use super::state_db::blockstate_db::InsertionType::StateInsertionType;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use log::{debug, info};
use once_cell::sync::Lazy;
use pool_sync::{Chain, Pool, PoolInfo, PoolType};
use rayon::prelude::*;
use reqwest::header::{HeaderMap, HeaderValue};
use reth::chainspec::arbitrary::Result;
use reth::core::primitives::Bytecode;
use reth::revm;
use reth::revm::revm::context::BlockEnv;
use reth::revm::revm::context::Evm;
use reth::revm::revm::primitives::Bytes;
use reth::revm::revm::primitives::*;
use reth::revm::revm::state::AccountInfo;
use reth_ethereum::evm::primitives::execute::Executor;
use reth_ethereum::evm::revm::Database;
use reth_ethereum::evm::revm::revm::primitives::{Address, U256};
use reth_ethereum::provider::db::mdbx::Database;
use reth_node_ethereum::EthereumNode;
use serde::{Deserialize, Serialize};

/// Represents the logical router + calldata type for different swap protocols
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SwapType {
    V2Basic,
    V3Basic,
    V3Deadline,
    V2Aerodrome,
    V3DeadlineTick,
}
//pub static AMOUNT: Lazy<RwLock<U256>> = Lazy::new(|| RwLock::new(U256::from(1_000_000_000_000_000_000u128)));

// Blacklisted tokens we don’t want to consider (e.g. scams, malicious)
lazy_static! {
    static ref BLACKLIST: Vec<Address> = vec![address!("be5614875952b1683cb0a2c20e6509be46d353a4")];
    static ref WETH_ADDRESS: Address = address!("4200000000000000000000000000000000000006");
}

// Common constants
const DEFAULT_PRIORITY_DIVISOR: usize = 50;
const SIMULATED_ACCOUNT: Address = address!("0000000000000000000000000000000000000001");
const MIN_OUTPUT_RATIO: u64 = 95;
const SIMULATED_GAS_LIMIT: u64 = 500_000;

pub static FAKE_TOKEN_AMOUNT: Lazy<U256> =
    Lazy::new(|| U256::from_str("10000000000000000000000000000000000000000").unwrap());

/// Filter and validate pools based on volume and simulated liquidity
#[derive(Serialize, Deserialize)]
struct TopVolumeAddresses(Vec<Address>);

#[derive(Debug, Deserialize)]
struct BirdeyeResponse {
    data: ResponseData,
}

#[derive(Debug, Deserialize)]
struct ResponseData {
    #[serde(default)]
    tokens: Option<Vec<Token>>,
}

#[derive(Debug, Deserialize)]
struct Token {
    #[serde(default)]
    address: Option<String>,
}

pub async fn filter_pools(pools: Vec<Pool>, num_results: usize, chain: Chain) -> Vec<Pool> {
    info!("Initial pool count before filter: {}", pools.len());

    let top_volume_tokens = get_top_volume_tokens(chain, num_results)
        .await
        .expect("Failed to fetch top-volume tokens from Birdeye");

    let filtered_by_token: Vec<Pool> = pools
        .into_par_iter()
        .filter(|pool| {
            let token0 = pool.token0_address();
            let token1 = pool.token1_address();
            top_volume_tokens.contains(&token0)
                && top_volume_tokens.contains(&token1)
                && !BLACKLIST.contains(&token0)
                && !BLACKLIST.contains(&token1)
        })
        .collect();

    info!(
        "Pool count after token match filter: {}",
        filtered_by_token.len()
    );

    let slot_map = construct_slot_map(&filtered_by_token);
    let pools_result = filter_by_swap(filtered_by_token, slot_map).await;

    debug!(
        "Pool count after simulated swap filter: {}",
        pools_result.as_ref().map(|p| p.len()).unwrap_or(0)
    );

    pools_result.expect("filter_by_swap failed")
}

/// Get top volume tokens from Birdeye or cache
async fn get_top_volume_tokens(chain: Chain, num_results: usize) -> Result<Vec<Address>> {
    let cache_file = format!("cache/top_volume_tokens_{}.json", chain);

    if Path::new(&cache_file).exists() {
        return read_addresses_from_file(&cache_file)
            .context("Failed to read cached volume tokens");
    }

    let tokens = fetch_top_volume_tokens(num_results, chain).await?;
    create_dir_all("cache")?;
    write_addresses_to_file(&tokens, &cache_file)?;
    Ok(tokens)
}

fn write_addresses_to_file(addresses: &[Address], filename: &str) -> std::io::Result<()> {
    let file = File::create(filename)?;
    let writer = BufWriter::new(file);
    let address_set = TopVolumeAddresses(addresses.to_vec());
    serde_json::to_writer(writer, &address_set)?;
    Ok(())
}

fn read_addresses_from_file(filename: &str) -> Result<Vec<Address>> {
    let file = File::open(filename)?;
    let reader = BufReader::new(file);
    let address_set: TopVolumeAddresses = serde_json::from_reader(reader)?;
    Ok(address_set.0)
}

async fn fetch_top_volume_tokens(num_results: usize, chain: Chain) -> Result<Vec<Address>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("Failed to build HTTP client");

    let mut headers = HeaderMap::new();
    let api_key = std::env::var("BIRDEYE_KEY").expect("BIRDEYE_KEY not set");

    headers.insert("X-API-KEY", HeaderValue::from_str(&api_key).unwrap());

    headers.insert(
        "x-chain",
        HeaderValue::from_str(match chain {
            Chain::Ethereum => "ethereum",
            Chain::Base => "base",
            // remove `_ => "unknown",` — it's unreachable!
        })
        .unwrap(),
    );

    let mut query_params = vec![];
    for offset in (0..num_results).step_by(DEFAULT_PRIORITY_DIVISOR) {
        let limit = std::cmp::min(DEFAULT_PRIORITY_DIVISOR, num_results - offset);
        query_params.push((offset, limit));
    }

    let mut addresses = vec![];

    for (offset, limit) in query_params {
        let response = client
            .get("https://public-api.birdeye.so/defi/tokenlist")
            .headers(headers.clone())
            .query(&[
                ("sort_by", "v24hUSD"),
                ("sort_type", "desc"),
                ("offset", &offset.to_string()),
                ("limit", &limit.to_string()),
            ])
            .send()
            .await
            .with_context(|| {
                format!(
                    "Failed to send Birdeye request at offset {}, limit {}",
                    offset, limit
                )
            })?;

        if response.status().is_success() {
            let parsed: BirdeyeResponse = response.json().await.with_context(|| {
                format!(
                    "Failed to decode Birdeye response at offset {}, limit {}",
                    offset, limit
                )
            })?;
            addresses.extend(
                parsed
                    .data
                    .tokens
                    .into_iter()
                    .map(|t| t.token_address.clone()),
            );
        }
    }

    Ok(addresses
        .into_iter()
        .filter_map(|addr| Address::from_str(addr.as_str()).ok())
        .collect())
}

fn construct_slot_map(pools: &[Pool]) -> HashMap<Address, FixedBytes<32>> {
    let mut slot_map = HashMap::new();

    for pool in pools {
        for &token in &[pool.token0_address(), pool.token1_address()] {
            if !slot_map.contains_key(&token) {
                // Use token address low 32 bytes as a mock slot, or customize
                let slot: FixedBytes<32> = FixedBytes::from_slice(&token.0[..32]);
                slot_map.insert(token, slot);
            }
        }
    }

    slot_map
}

async fn filter_by_swap(
    pools: Vec<Pool>,
    slot_map: HashMap<Address, FixedBytes<32>>,
) -> Result<Vec<Pool>> {
    let mut filtered = Vec::with_capacity(pools.len());

    let nodedb = NodeDB::open("./node_db.rs")?;

    for pool in pools {
        let (router, swap_type) = match resolve_router_and_type(pool.pool_type()) {
            Some(x) => x,
            None => continue,
        };

        let zero_to_one = determine_swap_direction(&pool);

        let slot0 = slot_map
            .get(&pool.token0_address())
            .copied()
            .ok_or_else(|| anyhow::anyhow!("Missing slot0"))?;
        let slot1 = slot_map
            .get(&pool.token1_address())
            .copied()
            .ok_or_else(|| anyhow::anyhow!("Missing slot1"))?;

        // Insert fake balances
        for (token, slot) in [
            (pool.token0_address(), slot0),
            (pool.token1_address(), slot1),
        ] {
            // nodedb.insert_account_storage(token, slot.into(), *FAKE_TOKEN_AMOUNT, InsertionType::OnChain)
            //     .map_err(|e| anyhow::anyhow!("Failed to insert account storage: {}", e))?;
        }

        let mut evm = EVM::builder()
            .with_db(&nodedb)
            .modify_tx_env(|tx| {
                tx.caller = SIMULATED_ACCOUNT;
                tx.value = U256::ZERO;
                tx.gas_limit = SIMULATED_GAS_LIMIT;
            })
            .build();

        for token in [pool.token0_address(), pool.token1_address()] {
            evm.tx_mut().data = approveCall {
                spender: router,
                amount: *FAKE_TOKEN_AMOUNT,
            }
            .abi_encode()
            .into();

            evm.tx_mut().transact_to = TransactTo::Call(token);
            evm.transact_commit()
                .ok_or_else(|| anyhow::anyhow!("Approval failed"))?;
        }

        let amt_val = *AMOUNT.read().expect("Failed to read amount");
        let min_expected = amt_val * U256::from(MIN_OUTPUT_RATIO) / U256::from(100);

        let forward = simulate_swap(
            &mut evm,
            &pool,
            swap_type,
            router,
            SIMULATED_ACCOUNT,
            amt_val,
            zero_to_one,
        )
        .ok_or_else(|| anyhow::anyhow!("Forward swap simulation failed"))?;

        let backward = simulate_swap(
            &mut evm,
            &pool,
            swap_type,
            router,
            SIMULATED_ACCOUNT,
            forward,
            !zero_to_one,
        )
        .ok_or_else(|| anyhow::anyhow!("Backward swap simulation failed"))?;

        if backward >= min_expected {
            filtered.push(pool.clone());
        }
    }

    Ok(filtered)
}

fn simulate_swap(
    evm: &mut InspectEvm,
    pool: &Pool,
    swap_type: SwapType,
    router: Address,
    account: Address,
    amount: U256,
    zero_to_one: bool,
) -> Option<U256> {
    let (calldata, is_vec) =
        setup_router_calldata(pool.clone(), account, amount, swap_type, zero_to_one);
    evm.tx_mut().transact_to = TransactTo::Call(router);
    evm.tx_mut().data = calldata.into();

    let res = evm.transact().ok()?.result;

    match res {
        ExecutionResult::Success { .. } => {
            let out = res.output()?;
            Some(decode_swap_return(out, is_vec))
        }
        _ => None,
    }
}

fn decode_swap_return(output: &Bytes, is_vec: bool) -> U256 {
    if is_vec {
        let vec_out = match <Vec<U256>>::abi_decode(output) {
            Ok(v) => v,
            Err(e) => {
                debug!("Vec decode failed: {:?}", e);
                return U256::ZERO;
            }
        };

        *vec_out.last().unwrap()
    } else {
        <U256>::abi_decode(output).unwrap()
    }
}

fn resolve_router_and_type(pt: PoolType) -> Option<(Address, SwapType)> {
    use PoolType::*;
    match pt {
        uniswap_v2 => Some((
            address!("0x4752a1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0"),
            SwapType::V2Basic,
        )),
        sushi_swap_v2 => Some((
            address!("0x6BDEa1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0"),
            SwapType::V2Basic,
        )),
        pancake_swap_v2 => Some((
            address!("0x8cFea1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0"),
            SwapType::V2Basic,
        )),
        uniswap_v3 => Some((
            address!("0x2626a1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0"),
            SwapType::V3Basic,
        )),
        sushi_swap_v3 => Some((
            address!("0xFB7ea1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0"),
            SwapType::V3Deadline,
        )),
        aerodrome => Some((
            address!("0xcF77a1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0"),
            SwapType::V2Aerodrome,
        )),
        slipstream => Some((
            address!("0xBE6Da1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0a1a0"),
            SwapType::V3DeadlineTick,
        )),
        _ => None,
    }
}

fn determine_swap_direction(pool: &Pool) -> bool {
    if pool.token0_address() == *WETH_ADDRESS {
        true
    } else if pool.token1_address() == *WETH_ADDRESS {
        false
    } else {
        true // default
    }
}

fn setup_router_calldata(
    pool: Pool,
    account: Address,
    amt: U256,
    swap_type: SwapType,
    zero_to_one: bool,
) -> (Vec<u8>, bool) {
    use alloy_sol_types::SolCall; // Ensure trait is in scope for abi_encode

    // Determine the correct token order
    let (token0, token1) = if zero_to_one {
        (pool.token0_address(), pool.token1_address())
    } else {
        (pool.token1_address(), pool.token0_address())
    };

    match swap_type {
        SwapType::V2Basic => {
            let calldata = V2Swap::swapExactTokensForTokensCall {
                amountIn: amt,
                amountOutMin: U256::ZERO,
                path: vec![token0, token1],
                to: account,
                deadline: U256::MAX,
            }
            .abi_encode();
            (calldata, true)
        }
        SwapType::V3Basic => {
            let swap_fee = pool.get_v3().expect("Missing pool details for V3Basic").fee;
            let params = V3Swap::ExactInputSingleParams {
                tokenIn: token0,
                tokenOut: token1,
                fee: swap_fee.try_into().expect("Invalid fee conversion"),
                recipient: account,
                amountIn: amt,
                amountOutMinimum: U256::ZERO,
                sqrtPriceLimitX96: U160::ZERO,
            };
            (V3Swap::exactInputSingleCall { params }.abi_encode(), false)
        }
        SwapType::V3Deadline => {
            let swap_fee = pool
                .get_v3()
                .expect("Missing pool details for V3Deadline")
                .fee;
            let params = V3SwapDeadline::ExactInputSingleParams {
                tokenIn: token0,
                tokenOut: token1,
                fee: swap_fee.try_into().expect("Invalid fee conversion"),
                recipient: account,
                deadline: U256::MAX,
                amountIn: amt,
                amountOutMinimum: U256::ZERO,
                sqrtPriceLimitX96: U160::ZERO,
            };
            (
                V3SwapDeadline::exactInputSingleCall { params }.abi_encode(),
                false,
            )
        }
        SwapType::V2Aerodrome => {
            let is_stable = pool
                .get_v2()
                .expect("Missing pool details for V2Aerodrome")
                .stable
                .expect("Missing 'stable' flag for Aerodrome pool");

            let route = vec![V2Aerodrome::Route {
                from: token0,
                to: token1,
                stable: is_stable,
                factory: Address::ZERO,
            }];

            let calldata = V2Aerodrome::swapExactTokensForTokensCall {
                amountIn: amt,
                amountOutMin: U256::ZERO,
                routes: route,
                to: account,
                deadline: U256::MAX,
            }
            .abi_encode();
            (calldata, true)
        }
        SwapType::V3DeadlineTick => {
            let tick_spacing = pool
                .get_v3()
                .expect("Missing pool details for V3DeadlineTick")
                .tick_spacing;

            let params = V3SwapDeadlineTick::ExactInputSingleParams {
                tokenIn: token0,
                tokenOut: token1,
                tickSpacing: tick_spacing.try_into().expect("Invalid tick_spacing"),
                recipient: account,
                deadline: U256::MAX,
                amountIn: amt,
                amountOutMinimum: U256::ZERO,
                sqrtPriceLimitX96: U160::ZERO,
            };
            (
                V3SwapDeadlineTick::exactInputSingleCall { params }.abi_encode(),
                false,
            )
        }
    }
}
