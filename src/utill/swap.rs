use serde::{Deserialize, Serialize};
use alloy::primitives::Address;
use pool_sync::PoolType;
use std::convert::From;
use std::hash::Hash;

use crate::util::util::rgen::{FlashQuoter, FlashSwap};
use crate::util::util::constants::AMOUNT;

#[derive(Serialize, Deserialize, Debug)]
struct Point {
    x: i32,
    y: i32,
}

/// Represents an individual swap step in a multi-hop path.
#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct SwapStep {
    pub pool_address: Address,
    pub token_in: Address,
    pub token_out: Address,
    #[serde(with = "pool_type_serde")]
    pub protocol: PoolType,
    pub fee: u32,
}

// Custom serde module for PoolType
mod pool_type_serde {
    use super::PoolType;
    use serde::{Deserializer, Serializer, Deserialize};

    pub fn serialize<S>(pt: &PoolType, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{:?}", pt))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PoolType, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "UniswapV2" => Ok(PoolType::UniswapV2),
            "SushiSwapV2" => Ok(PoolType::SushiSwapV2),
            "PancakeSwapV2" => Ok(PoolType::PancakeSwapV2),
            "UniswapV3" => Ok(PoolType::UniswapV3),
            "SushiSwapV3" => Ok(PoolType::SushiSwapV3),
            "Aerodrome" => Ok(PoolType::Aerodrome),
            "Slipstream" => Ok(PoolType::Slipstream),
            _ => Err(serde::de::Error::custom(format!("Unknown PoolType: {}", s))),
        }
    }
}

/// Full swap path that the bot will evaluate and potentially execute.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SwapPath {
    pub steps: Vec<SwapStep>,
    pub hash: u64,
}

/// This conversion is useful after estimating quotes from a flash quoter and preparing a swap call.
impl From<FlashQuoter::SwapParams> for FlashSwap::SwapParams {
    fn from(params: FlashQuoter::SwapParams) -> Self {
        FlashSwap::SwapParams {
            pools: params.pools,
            poolVersions: params.poolVersions,
            amountIn: params.amountIn,
        }
    }
}

/// Converts a [`SwapPath`] into a [`FlashQuoter::SwapParams`] for quote estimation.
impl From<SwapPath> for FlashQuoter::SwapParams {
    fn from(path: SwapPath) -> Self {
        let mut pools: Vec<Address> = Vec::with_capacity(path.steps.len());
        let mut protocols: Vec<u8> = Vec::with_capacity(path.steps.len());

        for step in path.steps {
            pools.push(step.pool_address);
            protocols.push(if step.protocol.is_v3() { 1 } else { 0 });
        }

        FlashQuoter::SwapParams {
            pools,
            poolVersions: protocols,
            amountIn: AMOUNT,
        }
    }
}
