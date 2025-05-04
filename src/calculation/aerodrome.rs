
use crate::calculation::Calculator; // Fix: Import Calculator struct

use alloy::network::Network;
use alloy::primitives::Address;
use alloy::providers::Provider;
use alloy::{primitives::U256, sol, sol_types::SolCall};
use once_cell::sync::Lazy;
use std::str::FromStr;
use tracing::warn;
// Assuming MarketState provides the necessary db access and pool info methods used below.


pub static INITIAL_AMT: Lazy<U256> = Lazy::new(|| U256::from_str("1000000000000000000").unwrap());
pub static WETH: Lazy<Address> =
    Lazy::new(|| Address::from_str("0x4200000000000000000000000000000000000006").unwrap());
pub static USDC: Lazy<Address> =
    Lazy::new(|| Address::from_str("0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913").unwrap());

// --- Aerodrome V2State contract
sol! {
    #[sol(rpc)]
    contract V2State {
        function getReserves() external view returns (
            uint112 reserve0,
            uint112 reserve1,
            uint32 blockTimestampLast
        );
    }
}

// --- Implementation for Calculator ---
// This block defines methods *on* the Calculator struct.
impl<N, P> Calculator<N, P>
where
    N: Network,
    P: Provider<N>, // Fix: Ensure Provider trait bound is correct
{
    /// Calculates Aerodrome swap output amount.
    pub fn aerodrome_out(&self, amount_in: U256, token_in: Address, pool_address: Address) -> U256 {
        // Access the database via market_state field on Calculator
        let db = self.market_state.db.read().expect("DB read poisoned");

        // Assuming these methods exist on your DB type within MarketState
        let (reserve0, reserve1) = db.get_reserves(&pool_address);
        let (dec0, dec1) = db.get_decimals(&pool_address);
        let fee = db.get_fee(&pool_address); // Assuming fee is u32 or similar, needs compatible math below
        let stable = db.get_stable(&pool_address);
        let token0 = db.get_token0(pool_address);

        let mut res0 = U256::from(reserve0);
        let mut res1 = U256::from(reserve1);

        // Apply fee - Ensure fee is represented correctly (e.g., basis points)
        // If fee is 1 = 0.01%, then divide by 10_000. Adjust if fee represents something else.
        let fee_amount = (amount_in * U256::from(fee) / U256::from(10_000));
        let amount_after_fee = amount_in.saturating_sub(fee_amount);

        if amount_after_fee.is_zero() {
            return U256::ZERO;
        }

        let token0_decimals = U256::from(10).pow(U256::from(dec0));
        let token1_decimals = U256::from(10).pow(U256::from(dec1));

        // Ensure decimals result in non-zero values before division
        if token0_decimals.is_zero() || token1_decimals.is_zero() {
            warn!(?pool_address, dec0, dec1, "Token decimals are zero, cannot calculate output.");
            return U256::ZERO;
        }

        if stable {
            // Stable swap math (Velodrome V1 style)
            // Scale reserves and amount_in to 18 decimals for calculation
            let scale_factor = U256::from(10).pow(U256::from(18));
            if scale_factor.is_zero() { // Should not happen for 10^18
                warn!("Scale factor is zero, cannot calculate stable swap.");
                return U256::ZERO;
            }

            let scaled_res0 = (res0.saturating_mul(scale_factor)) / token0_decimals;
            let scaled_res1 = (res1.saturating_mul(scale_factor)) / token1_decimals;
            let scaled_amount_in = if token_in == token0 {
                (amount_after_fee.saturating_mul(scale_factor)) / token0_decimals
            } else {
                (amount_after_fee.saturating_mul(scale_factor)) / token1_decimals
            };

            let (scaled_res_a, scaled_res_b) = if token_in == token0 {
                (scaled_res0, scaled_res1)
            } else {
                (scaled_res1, scaled_res0)
            };

            let xy = Self::_k(scaled_res0, scaled_res1); // Use scaled reserves
            let y_in = scaled_res_a.saturating_add(scaled_amount_in);
            let new_y = Self::_get_y(y_in, xy, scaled_res_b);
            let scaled_y = scaled_res_b.saturating_sub(new_y);

            // Scale output back to original token decimals
            if token_in == token0 {
                (scaled_y.saturating_mul(token1_decimals)) / scale_factor
            } else {
                (scaled_y.saturating_mul(token0_decimals)) / scale_factor
            }
        } else {
            // Volatile swap math (Uniswap V2 style)
            let (res_a, res_b) = if token_in == token0 {
                (res0, res1)
            } else {
                (res1, res0)
            };
            // Classic formula: dy = (dx * R_out) / (R_in + dx)
            (amount_after_fee * res_b) / (res_a + amount_after_fee)
        }
    }

    // Helper for stable k calculation (assumes inputs are scaled to 18 decimals)
    fn _k(x: U256, y: U256) -> U256 {
        let scale_factor = U256::from(10).pow(U256::from(18));
        if scale_factor.is_zero() { return U256::ZERO; }
        
        // k = xy(x^2 + y^2)
        let x_sq = (x.saturating_mul(x)) / scale_factor;
        let y_sq = (y.saturating_mul(y)) / scale_factor;
        let xy_term = (x.saturating_mul(y)) / scale_factor;
        (xy_term.saturating_mul(x_sq.saturating_add(y_sq))) / scale_factor
    }

    // Helper for stable get_y (Newton's method, assumes inputs scaled to 18 decimals)
    fn _get_y(x0: U256, xy_k: U256, mut y: U256) -> U256 {
        let scale_factor = U256::from(10).pow(U256::from(18));
        if scale_factor.is_zero() { return U256::ZERO; }
        let precision_one = U256::from(1);

        for i in 0..255 {
            let k_current = Self::_f(x0, y); // Current k based on x0 and y
            let d_val = Self::_d(x0, y);      // Derivative dK/dy

            if d_val.is_zero() {
                // Should not happen with positive reserves
                warn!(iteration = i, x0 = %x0, y = %y, "Aerodrome _get_y derivative is zero");
                return y; // Return current y as best estimate
            }

            let diff = if k_current > xy_k { k_current.saturating_sub(xy_k) } else { xy_k.saturating_sub(k_current) };
            let dy = (diff.saturating_mul(scale_factor)) / d_val; // Calculate change in y

            // If dy is zero, check boundaries or return current y
            if dy < precision_one {
                // Check if further iteration might cross the target k
                let next_y = if k_current < xy_k { y.saturating_add(precision_one) } else { y.saturating_sub(precision_one) };
                if next_y.is_zero() && k_current >= xy_k { // Prevent underflow if already at target or above
                    return y;
                }
                let k_next = Self::_f(x0, next_y);
                if k_current < xy_k {
                    if k_next >= xy_k { return next_y; } // Crossed target
                } else {
                    if k_next <= xy_k { return y; } // Crossed target or exactly hit
                }
                // If not crossed, dy=1 is the smallest step
                if dy == U256::ZERO {
                    if k_current == xy_k { return y; } // Already converged
                    // If not converged, make minimum step in the right direction
                    if k_current < xy_k { y = y.saturating_add(precision_one); } else { y = y.saturating_sub(precision_one); }
                } else {
                    // Apply calculated dy
                    if k_current < xy_k { y = y.saturating_add(dy); } else { y = y.saturating_sub(dy); }
                }
            } else {
                // Apply calculated dy
                if k_current < xy_k { y = y.saturating_add(dy); } else { y = y.saturating_sub(dy); }
            }
            
            if y.is_zero() && k_current < xy_k {
                // Should not happen if reserve y > 0 initially unless amount_in is huge
                warn!(iteration = i, x0 = %x0, "Aerodrome _get_y resulted in zero y prematurely");
                return U256::ZERO; // Indicate pool drain or error
            }
        }
        
        warn!("Aerodrome _get_y did not converge after 255 iterations");
        // Return the best estimate or potentially signal an error.
        y
    }

    // Helper for stable f(x, y) = xy(x^2+y^2) (assumes inputs scaled to 18 decimals)
    #[inline]
    fn _f(x: U256, y: U256) -> U256 {
        // Reuse _k logic directly
        Self::_k(x, y)
    }

    // Helper for stable derivative dK/dy = x(x^2 + 3y^2) (assumes inputs scaled to 18 decimals)
    fn _d(x: U256, y: U256) -> U256 {
        let scale_factor = U256::from(10).pow(U256::from(18));
        if scale_factor.is_zero() { return U256::ZERO; }
        
        let x_sq = (x.saturating_mul(x)) / scale_factor;
        let y_sq = (y.saturating_mul(y)) / scale_factor;
        let three_y_sq = U256::from(3).saturating_mul(y_sq);
        (x.saturating_mul(x_sq.saturating_add(three_y_sq))) / scale_factor
    }
}

// === Standalone Utility Functions ===
// These functions now correctly use the methods defined on the Calculator instance.

/// Simulate a MEV sandwich attack on Aerodrome + Uniswap
pub fn simulate_bundle_profit<N: Network, P: Provider<N>>(
    _calculator: &Calculator<N, P>,
    _aerodrome_pool_address: Address,
    _uniswap_pool_address: Address,
) -> U256 {
    // Assuming simulate_mev_bundle exists on Calculator
    // And that Trade struct/enum is defined appropriately
    /*
    let bundle = vec![...]; // Define the trades in the bundle
    calculator.simulate_mev_bundle(
        bundle,
        *INITIAL_AMT,
        *WETH,
        *USDC,
        U256::ZERO, // Assuming fee is handled within simulate_trade or is zero here
    )
    */
     // Placeholder as simulate_mev_bundle definition isn't fully shown in calculator.rs
    warn!("simulate_bundle_profit logic depends on Calculator::simulate_mev_bundle");
    U256::ZERO
}

/// Example usage to print best route
pub fn ample_best_route<N: Network, P: Provider<N>>( // Renamed to avoid conflict with potential keywords
    _calculator: &Calculator<N, P>,
    _initial_amt: U256,
    _weth: Address,
    _usdc: Address,
) {
    // Assuming find_best_route exists and returns Option<(Vec<Trade>, U256)> or similar
     // Also assumes Trade struct is defined and Debug printable
    /*
    let best_route_result = calculator.find_best_route(initial_amt, weth, usdc, 3);
    if let Some((path, amount_out)) = best_route_result {
        println!("Best route: {:?}, Amount out: {}", path, amount_out);
    } else {
        println!("No route found!");
    }
     */
     // Placeholder as find_best_route definition isn't fully shown in calculator.rs
     warn!("example_best_route logic depends on Calculator::find_best_route");
}
// === Extra Utility Functions ===

// Simulate a MEV sandwich attack on Aerodrome + Uniswap
// pub fn simulate_bundle_profit<N: alloy::providers::Network, P: alloy::providers::Provider<N>>(
//     calculator: &Calculator<N, P>,
//     aerodrome_pool_address: Address,
//     uniswap_pool_address: Address,
// ) -> U256 {
//     let profit = calculator.simulate_mev_bundle(
//         *INITIAL_AMT,
//         *WETH,
//         *USDC,
//         aerodrome_pool_address,
//         uniswap_pool_address,
//     );
//     profit
// }


// pub fn ample_best_route<N: alloy::providers::Network, P: alloy::providers::Provider<N>>(
//     calculator: &Calculator<N, P>,
//     initial_amt: U256,
//     weth: Address,
//     usdc: Address,
// ) {
//     let best_route = calculator.find_best_route(initial_amt, weth, usdc, 3);
//     if let Some((path, amount_out)) = best_route {
//         println!("Best route: {:?}, Amount out: {}", path, amount_out);
//     } else {
//         println!("No route found!");
//     }
// }
