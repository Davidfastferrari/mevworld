use crate::calculation::Calculator;
use crate::utile::DbTickDataProvider;
use uniswap_v3_sdk::prelude::TickMath; 
use alloy::network::Network;
use alloy::primitives::{Address, I256, U256};
use alloy::providers::Provider;
use anyhow::{Result, anyhow};
use log::info;
use uniswap_v3_math::swap_math;
use uniswap_v3_math::tick_math::{self, MAX_SQRT_RATIO, MAX_TICK, MIN_SQRT_RATIO, MIN_TICK};
use uniswap_v3_sdk::prelude::TickDataProvider;
pub const U256_1: U256 = U256::from_limbs([1, 0, 0, 0]);

// Mock DB access interface - not used in calculation functions
// pub struct MockDB {
//     pub liquidity: u128,
//     pub sqrt_price_x_96: U256,
//     pub tick: i32,
// }

pub struct CurrentState {
    pub amount_specified_remaining: I256,
    pub amount_calculated: I256,
    pub sqrt_price_x_96: U256,
    pub tick: i32,
    pub liquidity: u128,
}

#[derive(Default)]
pub struct StepComputations {
    pub sqrt_price_start_x_96: U256,
    pub tick_next: i32,
    pub initialized: bool,
    pub sqrt_price_next_x96: U256,
    // These fields are not used in the computation:
    // pub amount_in: U256,
    // pub amount_out: U256,
    // pub fee_amount: U256,
}

// Computes the position in the mapping where the initialized bit for a tick lives
// Not used in the current implementation
// pub fn position(tick: i32) -> (i16, u8) {
//     ((tick >> 8) as i16, (tick % 256) as u8)
// }

impl<N, P> Calculator<N, P>
where
    N: Network,
    P: Provider<N>,
{
    // Calculate the amount out for a uniswapv2 swap
    #[inline]
    pub fn uniswap_v2_out(
        &self,
        amount_in: U256,
        pool_address: &Address,
        token_in: &Address,
        fee: U256,
    ) -> U256 {
        // get read access to db
        let db_read = self.market_state.db.read().unwrap();
        let zero_to_one = match db_read.zero_to_one(pool_address, *token_in) {
            Ok(zto) => zto,
            Err(e) => {
                info!("Failed to get zero_to_one: {}", e);
                return U256::ZERO;
            }
        };
        let (reserve0, reserve1) = db_read.get_reserves(pool_address);

        let scalar = U256::from(10000);

        let (reserve_in, reserve_out) = if zero_to_one {
            (U256::from(reserve0), U256::from(reserve1))
        } else {
            (U256::from(reserve1), U256::from(reserve0))
        };

        let amount_in_with_fee = amount_in * fee;
        let numerator = amount_in_with_fee * reserve_out;
        let denominator = reserve_in * scalar + amount_in_with_fee;
        
        if denominator.is_zero() {
            info!("Uniswap V2 division by zero in denominator");
            return U256::ZERO;
        }
        numerator / denominator
    }

    // calculate the amount out for a uniswapv3 swap using swap_math and full_math for precision
    #[inline]
    pub fn uniswap_v3_out(
        &self,
        amount_in: U256,
        pool_address: &Address,
        token_in: &Address,
        fee: u32,
    ) -> Result<U256> {
        if amount_in.is_zero() {
            return Ok(U256::ZERO);
        }

        // acquire db read access and get all our state information
        let db_read = self.market_state.db.read().unwrap();
        let zero_to_one = db_read.zero_to_one(pool_address, *token_in).unwrap();
        let slot0 = db_read.slot0(*pool_address)?;
        let liquidity = db_read.liquidity(*pool_address)?;
        let tick_spacing = db_read.tick_spacing(pool_address)?;

        // Set sqrt_price_limit_x_96 to the max or min sqrt price in the pool depending on zero_for_one
        let sqrt_price_limit_x_96 = if zero_to_one {
            tick_math::MIN_SQRT_RATIO + U256_1
        } else {
            tick_math::MAX_SQRT_RATIO - U256_1
        };

        // Initialize a mutable state struct to hold the dynamic simulated state of the pool
        let mut current_state = CurrentState {
            sqrt_price_x_96: slot0.sqrtPriceX96, //Active price on the pool
            amount_calculated: I256::ZERO,       //Amount of token_out that has been calculated
            amount_specified_remaining: I256::from_raw(amount_in), //Amount of token_in that has not been swapped
            tick: slot0.tick,
            liquidity, //Current available liquidity in the tick range
        };

        // Prepare tick data provider
        let mut tick_data_provider = crate::utile::DbTickDataProvider::new(db_read.clone(), *pool_address, tick_spacing);

        while current_state.amount_specified_remaining > I256::ZERO
            && current_state.sqrt_price_x_96 != sqrt_price_limit_x_96
        {
            // Initialize a new step struct to hold the dynamic state of the pool at each step
            let mut step = StepComputations {
                // Set the sqrt_price_start_x_96 to the current sqrt_price_x_96
                sqrt_price_start_x_96: current_state.sqrt_price_x_96,
                ..Default::default()
            };

            // Get the next initialized tick using tick data provider
            let (tick_next, initialized) = tick_data_provider
                .next_initialized_tick_within_one_word(
                    current_state.tick,
                    zero_to_one,
                )?;

            step.tick_next = tick_next.clamp(tick_math::MIN_TICK, tick_math::MAX_TICK);
            step.initialized = initialized;

            // Get the next sqrt price from the input amount
            step.sqrt_price_next_x96 = tick_math::TickMath::get_sqrt_ratio_at_tick(step.tick_next)?;

            // Determine the target spot price for the swap step
            let swap_target_sqrt_ratio = if zero_to_one {
                step.sqrt_price_next_x96.max(sqrt_price_limit_x_96)
            } else {
                step.sqrt_price_next_x96.min(sqrt_price_limit_x_96)
            };

            // Compute swap step and update the current state using uniswap_v3_math swap math
            let (sqrt_price_result, amount_in_step, amount_out_step, fee_amount_step) =
                swap_math::compute_swap_step(
                    current_state.sqrt_price_x_96,
                    swap_target_sqrt_ratio,
                    current_state.liquidity,
                    current_state.amount_specified_remaining,
                    fee,
                )?;

            // Update state based on the results of compute_swap_step
            current_state.amount_specified_remaining = current_state.amount_specified_remaining
                .saturating_sub(I256::from_raw(amount_in_step.saturating_add(fee_amount_step)));
            current_state.amount_calculated = current_state.amount_calculated
                .saturating_sub(I256::from_raw(amount_out_step));
            current_state.sqrt_price_x_96 = sqrt_price_result;

            // If the price reached the step's target price, it means we crossed an initialized tick or hit the limit
            if current_state.sqrt_price_x_96 == step.sqrt_price_next_x96 {
                // If the tick crossed was initialized, adjust the liquidity
                if step.initialized {
                    // Get liquidity net from the tick data provider
                    let liquidity_net = tick_data_provider.get_liquidity_net(step.tick_next)?;
                    
                    let liquidity_change = if zero_to_one {
                        -liquidity_net
                    } else {
                        liquidity_net
                    };

                    // Apply the liquidity change safely
                    current_state.liquidity = if liquidity_change < 0 {
                        current_state
                            .liquidity
                            .checked_sub((-liquidity_change) as u128)
                            .ok_or_else(|| anyhow!("Insufficient liquidity during tick cross"))?
                    } else {
                        current_state
                            .liquidity
                            .checked_add(liquidity_change as u128)
                            .ok_or_else(|| anyhow!("Liquidity overflow during tick cross"))?
                    };
                }
                // Update the current tick based on the direction
                current_state.tick = if zero_to_one {
                    step.tick_next - 1
                } else {
                    step.tick_next
                };
            } else if current_state.sqrt_price_x_96 != step.sqrt_price_start_x_96 {
                // Update the tick to the tick corresponding to the final price
                current_state.tick =
                    tick_math::TickMath::get_tick_at_sqrt_ratio(current_state.sqrt_price_x_96)?;
                // Break the loop as amount_specified_remaining should be zero or negative
                break;
            }

            // Optional: Add info logging for debugging steps
            info!(
                "Swap step: tick={}, next_tick={}, sqrt_price={}, amount_in={}, amount_out={}, fee_amount={}",
                current_state.tick, step.tick_next, current_state.sqrt_price_x_96, 
                amount_in_step, amount_out_step, fee_amount_step
            );
        }

        Ok((-current_state.amount_calculated).into_raw())
    }
}
