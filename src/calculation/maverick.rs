use crate::calculation::Calculator;
use crate::utile::MarketState; // Assuming MarketState provides db access

use alloy::network::Network;
use alloy::primitives::{address, Address, Bytes, U256}; // Fix: Import Bytes struct
use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue}; // SolValue needed for decoding

// Correct imports for revm v22.0.1
use revm::primitives::{ExecutionResult, TransactTo};
use revm::{Database, Evm}; // Use top-level Evm and Database trait

use tracing::{debug, info, warn};

sol! {
    #[sol(rpc)]
    contract MaverickPool { // Renamed contract for clarity, assuming this is the pool contract
        // Function signature from Maverick V1 Pool contract
        // Note: Maverick V2 might have a different interface (e.g., via Periphery)
        function calculateSwap(
            // address pool, // Pool address might be implicit (this contract)
            uint128 amount, // Amount to swap (can be input or output)
            bool tokenAIn, // True if swapping tokenA for tokenB
            bool exactOutput, // True if `amount` is output amount, false if input
            int32 tickLimit // Price limit as tick index
            // V1 returns (amountIn, amountOut). V2 might differ.
        ) external view returns (uint256 amountIn, uint256 amountOut);
        // Maverick V1 doesn't return gasEstimate here. The error message's contract might be different.
        // Sticking to V1 signature for now based on file context. Adjust if using V2.
    }
}


impl<N, P> Calculator<N, P>
where
    N: Network,
    P: Provider<N>, // Fix: Add correct Provider trait bound
{
    /// Simulates a Maverick V1 swap and returns the output amount.
    /// Assumes `pool` is the Maverick Pool contract address.
    pub fn maverick_v1_out( // Renamed to reflect V1 assumption
        &self,
        amount_in: U256,
        pool: Address,
        token_a_in: bool, // Matches calculateSwap parameter
        tick_limit: i32, // Pass sqrtPriceLimitX96 if using V2 periphery
    ) -> U256 {
        // For V1, exactOutput is false when simulating amount_in
        let (_sim_in, sim_out) = self._simulate_maverick_v1(amount_in, pool, token_a_in, false, tick_limit);
        // Verify if sim_in matches amount_in? Could be slightly different due to fees/rounding.
        sim_out
    }

    /// Finds the optimal tick limit for a Maverick swap by simulating across a range.
    /// WARNING: Simple linear search, might not be optimal.
    pub fn optimize_tick_limit_maverick( // Renamed for clarity
        &self,
        pool: Address,
        amount_in: U256,
        token_a_in: bool,
        exact_output: bool, // Added parameter
    ) -> i32 {
        let mut best_tick = if token_a_in { -887272 } else { 887272 }; // Default to max range?
        let mut best_output = U256::ZERO; // If exact_output=false
        let mut best_input = U256::MAX;   // If exact_output=true

        // Define tick range based on token_a_in - search towards expected price move
        let tick_step = 1000; // Adjust step size as needed
        let (start_tick, end_tick) = if token_a_in { (-887272, 887272) } else { (887272, -887272) };
        let range = (start_tick..=end_tick).step_by(if token_a_in { tick_step } else { -tick_step });

        // Add current tick? Requires fetching pool state.

        for tick in range {
            let (sim_in, sim_out) = self._simulate_maverick_v1(amount_in, pool, token_a_in, exact_output, tick);

            if exact_output {
                // Minimize input for exact output
                if sim_in > U256::ZERO && sim_in < best_input {
                    best_input = sim_in;
                    best_tick = tick;
                }
            } else {
                // Maximize output for exact input
                if sim_out > best_output {
                    best_output = sim_out;
                    best_tick = tick;
                }
            }
        }

        info!(?pool, %amount_in, %token_a_in, %exact_output, optimized_tick=%best_tick, "Optimized Maverick tickLimit");
        best_tick
    }


    // --- state_diff_inspect requires significant changes to work with revm::Database ---
    // Commenting out as it's complex and depends on DB internals not exposed easily.
    /*
    pub fn state_diff_inspect( ... ) -> (Vec<u8>, Vec<u8>) { ... }
    */

    // --- gas_estimate_heatmap needs revision based on chosen simulation function ---
    // Commenting out for now. Simulation should return gas_used.
    /*
    pub fn gas_estimate_heatmap(...) -> Vec<(U256, U256)> { ... }
    */

    /// Builds calldata for Maverick V1 `calculateSwap`.
    fn build_maverick_v1_calldata(
        &self,
        // pool: Address, // Pool is `self` in the call context
        amount: U256,
        token_a_in: bool,
        exact_output: bool,
        tick_limit: i32,
    ) -> Bytes { // Return Bytes
        let call = MaverickPool::calculateSwapCall {
            amount: amount.try_into().expect("u128 overflow for Maverick amount"),
            tokenAIn: token_a_in,
            exactOutput: exact_output,
            tickLimit: tick_limit,
        };
        Bytes::from(call.abi_encode()) // Encode and convert to Bytes
    }

    /// Internal helper for Maverick V1 swap simulation.
    fn _simulate_maverick_v1(
        &self,
        amount: U256, // Input amount if exact_output=false, output amount if true
        pool: Address, // Maverick Pool contract address
        token_a_in: bool,
        exact_output: bool,
        tick_limit: i32,
    ) -> (U256, U256) { // Returns (amountIn, amountOut)
        let calldata = self.build_maverick_v1_calldata(amount, token_a_in, exact_output, tick_limit);

        let mut db_guard = self.market_state.db.write().expect("lock DB");
        let db = &mut *db_guard;

        let mut evm = Evm::builder()
            .with_db(db)
            .modify_tx_env(|tx| {
                 tx.caller = address!("0000000000000000000000000000000000000001");
                 tx.transact_to = TransactTo::Call(pool); // Target Maverick Pool contract
                 tx.data = calldata; // Already Bytes
                 tx.value = U256::ZERO;
                 tx.gas_limit = 1_000_000; // Set a reasonable gas limit for simulation
            })
            .build();

        match evm.transact() {
            Ok(ref_tx) => match ref_tx.result {
                ExecutionResult::Success { output, gas_used, .. } => {
                    // Decode the output (amountIn, amountOut)
                    match <(U256, U256)>::abi_decode(output.as_ref(), false) {
                        Ok((sim_amount_in, sim_amount_out)) => {
                            debug!(
                                "✅ Maverick V1 Sim: Target Amt={}, Pool={}, TokenAIn={}, ExactOut={}, TickLimit={} -> In={}, Out={}, GasUsed={}",
                                amount, pool, token_a_in, exact_output, tick_limit, sim_amount_in, sim_amount_out, gas_used
                            );
                            (sim_amount_in, sim_amount_out)
                        }
                        Err(e) => {
                            warn!("⚠️ Maverick V1 Sim Decode error: {:?}. Pool: {}, Output: {:?}", e, pool, output);
                            (U256::ZERO, U256::ZERO)
                        }
                    }
                }
                ExecutionResult::Revert { output, gas_used, .. } => {
                    warn!("⚠️ Maverick V1 Sim Reverted: {:?}. Pool: {}, Gas Used: {}", output, pool, gas_used);
                    (U256::ZERO, U256::ZERO)
                }
                other => {
                    warn!("⚠️ Maverick V1 Sim Unknown result: {:?}. Pool: {}", other, pool);
                    (U256::ZERO, U256::ZERO)
                }
            },
            Err(e) => {
                warn!("❌ Maverick V1 Sim EVM error: {:?}. Pool: {}", e, pool);
                (U256::ZERO, U256::ZERO)
            }
        }
    }
}