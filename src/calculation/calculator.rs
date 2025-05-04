use alloy::network::Network;
use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use pool_sync::PoolType;
use std::collections::HashSet;
use std::sync::Arc;

use crate::calculation::aerodrome;
use crate::calculation::balancer;
use crate::calculation::uniswap;
use crate::utile::{AMOUNT, Cache, MarketState, SwapPath};

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
                uniswap::uniswap_v2_out(
                    self,
                    input_amount,
                    &pool_address,
                    &token_in,
                    U256::from(9970),
                )
            }
            PoolType::PancakeSwapV2 | PoolType::BaseSwapV2 | PoolType::DackieSwapV2 => {
                uniswap::uniswap_v2_out(
                    self,
                    input_amount,
                    &pool_address,
                    &token_in,
                    U256::from(9975),
                )
            }
            PoolType::AlienBaseV2 => uniswap::uniswap_v2_out(
                self,
                input_amount,
                &pool_address,
                &token_in,
                U256::from(9984),
            ),
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
            PoolType::Aerodrome => {
                aerodrome::aerodrome_out(self, input_amount, token_in, pool_address)
            }
            PoolType::BalancerV2 => {
                balancer::balancer_v2_out(self, input_amount, token_in, token_in, pool_address)
            }
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

    pub fn simulate_mev_bundle(
        &self,
        bundle: Vec<Trade>,
        input_amount: U256,
        token_in: Address,
        token_out: Address,
        fee: U256,
    ) -> U256 {
        let mut output_amount = input_amount;
        for trade in bundle {
            output_amount = self.simulate_trade(
                output_amount,
                token_in,
                token_out,
                trade.pool_address,
                trade.pool_type,
                fee,
            );
        }
        output_amount
    }

    pub fn find_best_route(
        &self,
        initial_amt: U256,
        token_in: Address,
        token_out: Address,
        max_hops: u8,
    ) -> Vec<Trade> {
        let mut best_route = Vec::new();
        let mut best_profit = U256::ZERO;
        let mut current_amount = initial_amt;

        for hop in 1..=max_hops {
            let mut current_route = Vec::new();
            let mut current_profit = U256::ZERO;

            for pool in self.get_pools(token_in, token_out) {
                let output_amount = self.simulate_trade(
                    current_amount,
                    token_in,
                    token_out,
                    pool.address,
                    pool.pool_type,
                    U256::from(9984),
                );

                if output_amount > current_amount {
                    current_profit = output_amount - current_amount;
                    current_route.push(Trade {
                        pool_address: pool.address,
                        pool_type: pool.pool_type,
                    });
                }
            }

            if current_profit > best_profit {
                best_profit = current_profit;
                best_route = current_route;
            }

            current_amount = output_amount;
        }

        best_route
    }

    fn get_pools(&self, token_in: Address, token_out: Address) -> Vec<Pool> {
        let pool_sync = self.market_state.pool_sync.read().unwrap();
        let pools = pool_sync.get_pools(token_in, token_out);
        pools
            .into_iter()
            .map(|pool| Pool {
                address: pool.address,
                pool_type: pool.pool_type,
            })
            .collect()
    }

    fn simulate_trade(
        &self,
        input_amount: U256,
        token_in: Address,
        token_out: Address,
        pool_address: Address,
        pool_type: PoolType,
        fee: U256,
    ) -> U256 {
        self.compute_amount_out(input_amount, pool_address, token_in, pool_type, fee)
    }
}

struct Trade {
    pool_address: Address,
    pool_type: PoolType,
}

struct Pool {
    address: Address,
    pool_type: PoolType,
}