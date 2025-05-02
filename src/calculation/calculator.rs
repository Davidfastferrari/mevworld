use alloy_sol_types::sol;
use alloy::network::Network;
use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use pool_sync::PoolType;
use std::collections::HashSet;
use std::sync::Arc;

mod utils;

use crate::utils::modcal::cache::Cache;
use crate::utils::modcal::market_state::MarketState;
use crate::utils::modcal::swap::{SwapPath, SwapStep};
use crate::utils::utils::constants::AMOUNT;
use crate::modcal::uniswap;
use crate::modcal::balancer;
use crate::modcal::aerodrome;

pub struct Calculator<N, P>
where
    N: Network,
    P: Provider<N>,
{
    pub market_state: Arc<MarketState<N, P>>,
    pub cache: Arc<Cache>,
}

impl<N, P> Calculator<N, P>
where
    N: Network,
    P: Provider<N>,
{
    pub fn new(market_state: Arc<MarketState<N, P>>) -> Self {
        Self {
            market_state,
            cache: Arc::new(Cache::new(500)),
        }
    }

    pub fn invalidate_cache(&self, pools: &HashSet<Address>) {
        for pool in pools {
            self.cache.invalidate(*pool);
        }
    }

    #[inline(always)]
    pub fn compute_pool_output(
        &self,
        pool_addr: Address,
        token_in: Address,
        protocol: PoolType,
        fee: u32,
        input: U256,
    ) -> U256 {
        self.compute_amount_out(input, pool_addr, token_in, protocol, fee)
    }

    pub fn debug_calculation(&self, path: &SwapPath) -> Vec<U256> {
        let mut amount = *AMOUNT.read().unwrap();
        let mut path_trace = vec![amount];

        for swap_step in &path.steps {
            let output_amount = self.compute_amount_out(
                amount,
                swap_step.pool_address,
                swap_step.token_in,
                swap_step.protocol,
                swap_step.fee,
            );
            path_trace.push(output_amount);
            amount = output_amount;
        }

        path_trace
    }

    pub fn compute_amount_out(
        &self,
        input_amount: U256,
        pool_address: Address,
        token_in: Address,
        pool_type: PoolType,
        fee: u32,
    ) -> U256 {
        match pool_type {
            PoolType::UniswapV2 | PoolType::SushiSwapV2 | PoolType::SwapBasedV2 => {
                uniswap::uniswap_v2_out(self, input_amount, &pool_address, &token_in, U256::from(9970))
            }
            PoolType::PancakeSwapV2 | PoolType::BaseSwapV2 | PoolType::DackieSwapV2 => {
                uniswap::uniswap_v2_out(self, input_amount, &pool_address, &token_in, U256::from(9975))
            }
            PoolType::AlienBaseV2 => {
                uniswap::uniswap_v2_out(self, input_amount, &pool_address, &token_in, U256::from(9984))
            }
            PoolType::UniswapV3
            | PoolType::SushiSwapV3
            | PoolType::BaseSwapV3
            | PoolType::Slipstream
            | PoolType::PancakeSwapV3
            | PoolType::AlienBaseV3
            | PoolType::SwapBasedV3
            | PoolType::DackieSwapV3 => {
                uniswap::uniswap_v3_out(self, input_amount, &pool_address, &token_in, fee)
                    .expect("Uniswap V3 computation failed")
            }
            PoolType::Aerodrome => aerodrome::aerodrome_out(self, input_amount, token_in, pool_address),
            PoolType::BalancerV2 => balancer::balancer_v2_out(self, input_amount, token_in, token_in, pool_address),
            PoolType::MaverickV1 | PoolType::MaverickV2 => {
                tracing::warn!("Maverick pool logic not implemented");
                U256::ZERO
            }
            PoolType::CurveTwoCrypto | PoolType::CurveTriCrypto => {
                tracing::warn!("Curve pool logic not implemented");
                U256::ZERO
            }
        }
    }
}
