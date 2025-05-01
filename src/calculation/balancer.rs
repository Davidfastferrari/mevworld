use alloy::network::Network;
use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use anyhow::Result;

use crate::calculation::Calculator;

/// Balancer V2 swap formula implemented on top of AMM math.
impl<N, P> Calculator<N, P>
where
N: Network,
P: Provider,
{
// Calculate output for a Balancer V2 style swap using exponentiation invariant
pub fn balancer_v2_out<N: Network, P: Provider<N>>(
    calculator: &Calculator<N, P>,
    amount_in: U256,
    token_in: Address,
    token_out: Address,
    pool_address: Address,
) -> U256{
let pool = self.pool_manager.get_balancer_pool(&pool_address)
.expect("Pool not found");

    let token_in_index = pool.get_token_index(&token_in)
        .expect("Token in not found in pool");
    let token_out_index = pool.get_token_index(&token_out)
        .expect("Token out not found in pool");

    let balance_in = pool.balances[token_in_index];
    let balance_out = pool.balances[token_out_index];
    let weight_in = pool.weights[token_in_index];
    let weight_out = pool.weights[token_out_index];
    let swap_fee = pool.swap_fee;

    // Scale based on token decimals
    let scaling_factor = 18 - pool.token0_decimals as i8;
    let scaled_amount_in = Self::scale(amount_in, scaling_factor);
    let scaled_amount_in_after_fee = Self::sub(
        scaled_amount_in,
        Self::mul_up(scaled_amount_in, swap_fee)
    );
    let amount_in_scaled = Self::scale(scaled_amount_in_after_fee, scaling_factor);

    let denominator = Self::add(balance_in, amount_in_scaled);
    let base = Self::div_up(balance_in, denominator);
    let exponent = Self::div_down(weight_in, weight_out);
    let power = Self::pow_up(base, exponent);

    Self::mul_down(balance_out, Self::complement(power))
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
    
    fn pow_up(x: U256, y: U256) -> U256 {
        // Implement pow function directly here using floating point approximation or integer math
        // For simplicity, convert to f64, compute powf, then convert back to U256
        let one = U256::from(1_000_000_000_000_000_000u64);
        let x_f64 = x.as_u128() as f64 / 1e18;
        let y_f64 = y.as_u128() as f64 / 1e18;
        let result_f64 = x_f64.powf(y_f64);
        let result_u128 = (result_f64 * 1e18) as u128;
        U256::from(result_u128)
    }
    
    fn complement(x: U256) -> U256 {
        let one = U256::from(1_000_000_000_000_000_000u64);
        if x < one {
            one - x
        } else {
            U256::ZERO
        }
    }
}   