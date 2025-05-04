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

    // ---------- Scaled Math Helpers (adapted from original, assuming 1e18 scaling) ----------
    // Renamed to avoid conflicts if other modules use similar names.

    // fn scale_balancer(value: U256, decimals: i8) -> U256 { // Scaling might be needed depending on DB values
    //     if decimals >= 0 {
    //         value * U256::from(10).pow(U256::from(decimals as u32))
    //     } else {
    //         value / U256::from(10).pow(U256::from((-decimals) as u32))
    //     }
    // }

    fn add_balancer(a: U256, b: U256) -> U256 {
        a + b
    }

    fn sub_balancer(a: U256, b: U256) -> U256 {
        a.saturating_sub(b)
    }

    // Scaled division a/b (result scaled by 1e18)
    fn div_down_balancer(a: U256, b: U256) -> U256 {
        if b.is_zero() { return U256::MAX; } // Or handle error appropriately
        if a.is_zero() { return U256::ZERO; }
        let one = U256::from(10).pow(U256::from(18));
        (a * one) / b
    }

     // Scaled division a/b, rounded up (result scaled by 1e18)
    fn div_up_balancer(a: U256, b: U256) -> U256 {
         if b.is_zero() { return U256::MAX; } // Or handle error appropriately
         if a.is_zero() { return U256::ZERO; }
         let one = U256::from(10).pow(U256::from(18));
         // (a * 1e18 + b - 1) / b
         let numerator = a * one + b - U256::from(1);
         numerator / b
    }


    // Scaled multiplication a*b (inputs scaled by 1e18, result scaled by 1e18)
    fn mul_down_balancer(a: U256, b: U256) -> U256 {
        if a.is_zero() || b.is_zero() { return U256::ZERO; }
        let one = U256::from(10).pow(U256::from(18));
        (a * b) / one
    }

    // Scaled multiplication a*b, rounded up (inputs scaled by 1e18, result scaled by 1e18)
    fn mul_up_balancer(a: U256, b: U256) -> U256 {
         if a.is_zero() || b.is_zero() { return U256::ZERO; }
         let one = U256::from(10).pow(U256::from(18));
         // (a * b + 1e18 - 1) / 1e18
         let numerator = a * b + one - U256::from(1);
         numerator / one
    }

    // Scaled power x^y (inputs scaled by 1e18, result scaled by 1e18)
    // WARNING: pow_up implementation using f64 is highly imprecise for financial calculations.
    // Consider using a dedicated fixed-point math library or integer exponentiation by squaring.
    fn pow_up_balancer(x: U256, y: U256) -> U256 {
        // Original f64 implementation (IMPRECISE - USE WITH EXTREME CAUTION)
        let one = U256::from(10).pow(U256::from(18));
        if x == one { return one; } // 1^y = 1
        if y.is_zero() { return one; } // x^0 = 1
        if x.is_zero() { return U256::ZERO; } // 0^y = 0 for y > 0

        // Convert to f64 - large loss of precision
        let x_f64 = x.to::<f64>() / 1e18;
        let y_f64 = y.to::<f64>() / 1e18;

        // Handle potential errors in f64 conversion or powf
        if x_f64.is_nan() || y_f64.is_nan() {
            warn!("NaN encountered in Balancer pow_up_balancer f64 conversion");
            return U256::MAX; // Or some error indicator
        }

        let result_f64 = x_f64.powf(y_f64);

        if result_f64.is_nan() || result_f64.is_infinite() || result_f64 < 0.0 {
             warn!("Invalid result from powf in Balancer pow_up_balancer: {}", result_f64);
             return U256::MAX; // Indicate error/overflow
        }

        // Convert back to U256 - more precision loss and potential overflow
        let result_scaled = result_f64 * 1e18;
        if result_scaled > U256::MAX.to::<f64>() {
            warn!("Overflow converting f64 result back to U256 in Balancer pow_up_balancer");
            return U256::MAX;
        }

        // Add small epsilon for rounding up? Balancer's FixedPoint math handles this better.
        // U256::try_from(result_scaled.ceil() as u128).unwrap_or(U256::MAX) // Example rounding up

        // Simple truncation conversion
        U256::try_from(result_scaled as u128).unwrap_or(U256::MAX)

        // TODO: Replace f64 pow with a precise fixed-point or integer power function.
    }

    // Scaled complement 1 - x (input scaled by 1e18, result scaled by 1e18)
    fn complement_balancer(x: U256) -> U256 {
        let one = U256::from(10).pow(U256::from(18));
        if x < one { one - x } else { U256::ZERO } // 1-x, or 0 if x >= 1
    }
}