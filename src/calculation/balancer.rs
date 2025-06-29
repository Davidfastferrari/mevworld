use crate::calculation::Calculator; // Fix: Import Calculator

use alloy::network::Network;
use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
// Assuming MarketState provides the necessary db access and pool info methods used below.
use crate::utile::MarketState;

impl<N, P> Calculator<N, P>
where
    N: Network,
    P: Provider<N>, // Fix: Correct trait bound for Provider.
{
    /// Calculate output for a Balancer V2 weighted pool swap using exponentiation invariant.
    /// Assumes pool details (balances, weights, fee) are available via market_state.db
    pub fn balancer_v2_out(
        &self,
        amount_in: U256,
        token_in: Address,
        token_out: Address, // Added token_out parameter
        pool_address: Address,
    ) -> U256 {
        // Access the database via market_state field on Calculator
        let db = self.market_state.db.read().expect("DB read poisoned");

        // Fetch Balancer pool details from the DB
        // NOTE: Replace these with your actual DB methods for Balancer pools
        let balances = db.get_balancer_balances(&pool_address); // e.g., returns Vec<U256>
        let weights = db.get_balancer_weights(&pool_address);   // e.g., returns Vec<U256> (scaled)
        let swap_fee = db.get_balancer_fee(&pool_address);     // e.g., returns U256 (scaled, e.g., 1e15 for 0.1%)
        let tokens = db.get_balancer_tokens(&pool_address);     // e.g., returns Vec<Address>

        // Find indices for token_in and token_out
        let token_in_index = tokens.iter().position(|&t| t == token_in).expect("Token in not found in Balancer pool");
        let token_out_index = tokens.iter().position(|&t| t == token_out).expect("Token out not found in Balancer pool");

        // --- Balancer Math (based on SOR or Vault formulas) ---
        // https://docs.balancer.fi/concepts/math/weighted-math.html#swap-calculation
        // amountOut = balanceOut * (1 - (balanceIn / (balanceIn + amountIn)) ^ (weightIn / weightOut))

        let balance_in = balances[token_in_index];
        let balance_out = balances[token_out_index];
        // Weights are typically normalized (sum to 1) or need scaling factor (e.g., 1e18)
        let weight_in = weights[token_in_index];
        let weight_out = weights[token_out_index];

        // Apply swap fee to amount_in
        // Balancer fees are applied on the way IN.
        // amountInAfterFee = amountIn * (1 - swapFeePercentage)
        let one = U256::from(10).pow(U256::from(18)); // Assuming fee is scaled to 1e18
        let amount_in_after_fee = amount_in * (one - swap_fee) / one;

        // Calculate base = balanceIn / (balanceIn + amountInAfterFee)
        let denominator = balance_in + amount_in_after_fee;
        if denominator.is_zero() { return U256::ZERO; } // Avoid division by zero
         // Use precise division (e.g., FixedPoint math or scaled U256) if necessary
         // Simple U256 division might lose precision needed for exponentiation.
         // Using scaled math helpers like in original code:
        let base = Self::div_down_balancer(balance_in, denominator); // div_down assumes scaling

        // Calculate exponent = weightIn / weightOut
        if weight_out.is_zero() { return U256::ZERO; } // Avoid division by zero
        let exponent = Self::div_down_balancer(weight_in, weight_out); // div_down assumes scaling

        // Calculate power = base ^ exponent
        // This is the trickiest part with U256. Requires approximation or library.
        // Using the provided pow_up helper (needs careful review for precision/correctness)
        let power = Self::pow_up_balancer(base, exponent);

        // Calculate amountOut = balanceOut * (1 - power)
        let factor = Self::complement_balancer(power); // complement assumes scaling
        let amount_out = Self::mul_down_balancer(balance_out, factor); // mul_down assumes scaling

        amount_out
    }

    // ---------- Math Helpers ----------

    fn scale(value: U256, decimals: i8) -> U256 {
        value * U256::from(10).pow(U256::from(decimals as u32))
    }

    fn add(a: U256, b: U256) -> U256 {
        a + b
    }

    fn sub(a: U256, b: U256) -> U256 {
        a.saturating_sub(b)
    }

    fn div_up(a: U256, b: U256) -> U256 {
        if a.is_zero() {
            return U256::ZERO;
        }
        let one = U256::from(1_000_000_000_000_000_000u64);
        ((a * one - 1u64) / b) + 1u64
    }

    fn div_down(a: U256, b: U256) -> U256 {
        if a.is_zero() {
            return U256::ZERO;
        }
        (a * U256::from(1_000_000_000_000_000_000u64)) / b
    }

    fn mul_up(a: U256, b: U256) -> U256 {
        if a.is_zero() || b.is_zero() {
            return U256::ZERO;
        }
        let one = U256::from(1_000_000_000_000_000_000u64);
        ((a * b - 1u64) / one) + 1u64
    }

    fn mul_down(a: U256, b: U256) -> U256 {
        (a * b) / U256::from(1_000_000_000_000_000_000u64)
    }

    fn pow_up_balancer(x: U256, y: U256) -> U256 {
        // Implement pow function directly here using floating point approximation or integer math
        // For simplicity, convert to f64, compute powf, then convert back to U256
        let one = U256::from(1_000_000_000_000_000_000u64);
        let x_f64 = x.as_u128() as f64 / 1e18;
        let y_f64 = y.as_u128() as f64 / 1e18;
        let result_f64 = x_f64.powf(y_f64);
        let result_u128 = (result_f64 * 1e18) as u128;
        U256::from(result_u128)
    }

    fn complement_balancer(x: U256) -> U256 {
        let one = U256::from(1_000_000_000_000_000_000u64);
        if x < one { one - x } else { U256::ZERO }
    }
}