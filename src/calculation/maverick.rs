use crate::calculation::Calculator;
use crate::utile::MarketState; // Assuming MarketState provides db access

use alloy::network::Network;
use alloy::primitives::{address, Address, Bytes, Log, StorageKey, StorageValue, U256, B256}; // Added Log, B256, StorageKey, StorageValue
use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};

// Correct imports for revm (adjust version if needed)
use revm::primitives::{
    Account, AccountInfo, Bytecode, ExecutionResult, Output, State, // Added State, Account, AccountInfo, Bytecode, Output
    TransactTo, TxEnv, CfgEnv, Env, KECCAK_EMPTY, // Added KECCAK_EMPTY
};
use revm::{Database, Evm};

use tracing::{debug, info, warn};
use std::collections::BTreeMap; // Use BTreeMap for ordered state diff output


sol! {
    #[sol(rpc)]
    contract MaverickPool {
        function calculateSwap(
            uint128 amount,
            bool tokenAIn,
            bool exactOutput,
            int32 tickLimit
        ) external view returns (uint256 amountIn, uint256 amountOut);
    }
}


impl<N, P> Calculator<N, P>
where
    N: Network,
    P: Provider<N>,
{
    /// Simulates a Maverick V1 swap and returns the output amount.
    pub fn maverick_v1_out(
        &self,
        amount_in: U256,
        pool: Address,
        token_a_in: bool,
        tick_limit: i32,
    ) -> U256 {
        let (_sim_in, sim_out, _gas_used) = self._simulate_maverick_v1_detailed(amount_in, pool, token_a_in, false, tick_limit);
        sim_out
    }

    /// Finds the optimal tick limit for a Maverick swap by simulating across a range.
    pub fn optimize_tick_limit_maverick(
        &self,
        pool: Address,
        amount: U256,
        token_a_in: bool,
        exact_output: bool,
    ) -> i32 {
        let default_tick = if token_a_in { -887272 } else { 887272 };
        let mut best_tick = default_tick;
        let mut best_output = U256::ZERO;
        let mut best_input = U256::MAX;

        let tick_step = 1000;
        let search_range = 50000 / tick_step;
        let (start_tick_idx, end_tick_idx) = if token_a_in {
            (default_tick / tick_step - search_range, default_tick / tick_step + search_range)
        } else {
            (default_tick / tick_step + search_range, default_tick / tick_step - search_range)
        };

        let ticks_to_check = if token_a_in {
            start_tick_idx..=end_tick_idx
        } else {
            // Iterate backwards correctly
            (end_tick_idx..=start_tick_idx).rev()
        }.map(|i| (i * tick_step).clamp(-887272, 887272)) // Calculate and clamp tick
         .chain(std::iter::once(default_tick)); // Ensure default is checked

        for tick in ticks_to_check {
            let (sim_in, sim_out, _gas_used) = self._simulate_maverick_v1_detailed(amount, pool, token_a_in, exact_output, tick);

            if exact_output {
                if sim_in > U256::ZERO && sim_in < best_input {
                    best_input = sim_in;
                    best_tick = tick;
                }
            } else {
                if sim_out > best_output {
                    best_output = sim_out;
                    best_tick = tick;
                }
            }
        }

        info!(?pool, %amount, %token_a_in, %exact_output, optimized_tick=%best_tick, "Optimized Maverick tickLimit");
        best_tick
    }

    /// Simulates a Maverick V1 transaction and inspects the state changes.
    /// Returns the state diff as serialized BTreeMaps for accounts and storage.
    /// Note: `calculateSwap` is view, so the diff *should* be empty unless revm tracks reads.
    /// To inspect a real swap, simulate the actual swap transaction calldata.
    pub fn state_diff_inspect(
        &self,
        pool: Address,
        amount: U256,
        token_a_in: bool,
        exact_output: bool,
        tick_limit: i32,
    ) -> Result<(Vec<u8>, Vec<u8>), String> { // Return Result for better error handling
        let calldata = self.build_maverick_v1_calldata(amount, token_a_in, exact_output, tick_limit);

        let mut db_guard = self.market_state.db.write().map_err(|_| "Failed to lock DB".to_string())?;
        let db = &mut *db_guard;

        let cfg = CfgEnv::default();
        let block = self.market_state.block_env.read().map_err(|_| "Failed to lock BlockEnv".to_string())?.clone();
        let tx = TxEnv {
             caller: address!("0000000000000000000000000000000000000001"),
             transact_to: TransactTo::Call(pool),
             data: calldata,
             value: U256::ZERO,
             gas_limit: 1_000_000, // Adjust if needed for actual swaps
             gas_price: U256::ZERO,
             ..Default::default()
        };

        let mut evm = Evm::builder()
            .with_db(db)
            .with_env(Box::new(Env { cfg, block, tx }))
            .build();

        // Use transact_commit to get the state diff back
        match evm.transact_commit() {
            Ok(result) => match result {
                ExecutionResult::Success { state, logs, .. } => {
                    debug!("State diff inspect successful. State changes: {}, Logs: {}", state.len(), logs.len());

                    // Convert the revm::State (HashMap) to BTreeMap for ordered serialization
                    let accounts_diff: BTreeMap<Address, Account> = state.into_iter().collect();

                    // Serialize accounts diff (consider using serde/bincode for more robust serialization)
                    let accounts_bytes = bincode::serialize(&accounts_diff)
                        .map_err(|e| format!("Failed to serialize accounts diff: {}", e))?;

                    // For storage, revm::Account contains storage: HashMap<U256, StorageSlot>.
                    // We need to extract and potentially serialize this per account.
                    // Let's serialize the storage for each account in the diff separately.
                    // The second Vec<u8> could represent a map from Address to serialized storage map.
                    // For simplicity, let's serialize the whole accounts_diff which includes storage.
                    // Returning two identical Vecs might be redundant based on this serialization.
                    // Let's return serialized accounts map and an empty vec for storage for now.
                    // TODO: Refine the return type and serialization if specific storage diff format is needed.
                    let storage_bytes = Vec::new(); // Placeholder

                    Ok((accounts_bytes, storage_bytes))
                }
                ExecutionResult::Revert { output, .. } => {
                    let reason = String::from_utf8_lossy(output.data());
                    Err(format!("State diff inspect reverted: '{}'", reason))
                }
                ExecutionResult::Halt { reason, .. } => {
                    Err(format!("State diff inspect halted: {:?}", reason))
                }
                 other => {
                     Err(format!("State diff inspect unknown execution result: {:?}", other))
                 }
            },
            Err(e) => {
                 Err(format!("State diff inspect EVM error: {:?}", e))
            }
        }
    }


    /// Generates a gas estimate heatmap for Maverick V1 calculateSwap over a range of input amounts.
    pub fn gas_estimate_heatmap(
        &self,
        pool: Address,
        token_a_in: bool,
        tick_limit: i32,
        start_amount: U256,
        end_amount: U256,
        steps: u32,
    ) -> Result<Vec<(U256, u64)>, String> { // Return Result and use u64 for gas
        if steps == 0 || end_amount < start_amount {
            return Err("Invalid range or zero steps for heatmap".to_string());
        }

        let mut results = Vec::with_capacity(steps as usize + 1);
        let step_size = (end_amount - start_amount) / U256::from(steps);

        for i in 0..=steps {
            let current_amount = start_amount + step_size * U256::from(i);
            // Ensure last step uses exact end_amount if step_size causes rounding issues
            let amount_to_simulate = if i == steps { end_amount } else { current_amount };

            // Simulate, but only need gas_used
             // Use _simulate_maverick_v1_detailed which returns gas
            let (_sim_in, _sim_out, gas_used_opt) = self._simulate_maverick_v1_detailed(
                amount_to_simulate,
                pool,
                token_a_in,
                false, // Assuming exact input for heatmap
                tick_limit,
            );

            match gas_used_opt {
                Some(gas) => {
                    results.push((amount_to_simulate, gas));
                }
                None => {
                    // Simulation failed for this amount, add entry with 0 gas? Or skip?
                    warn!(%amount_to_simulate, "Simulation failed for gas estimate heatmap point");
                    results.push((amount_to_simulate, 0)); // Indicate failure with 0 gas
                }
            }
        }

        Ok(results)
    }


    /// Builds calldata for Maverick V1 `calculateSwap`.
    fn build_maverick_v1_calldata(
        &self,
        amount: U256,
        token_a_in: bool,
        exact_output: bool,
        tick_limit: i32,
    ) -> Bytes {
        let amount_u128 = match amount.try_into() {
            Ok(a) => a,
            Err(_) => {
                warn!(%amount, "Maverick amount exceeds u128::MAX, using u128::MAX");
                u128::MAX
            }
        };

        let call = MaverickPool::calculateSwapCall {
            amount: amount_u128,
            tokenAIn: token_a_in,
            exactOutput: exact_output,
            tickLimit: tick_limit,
        };
        Bytes::from(call.abi_encode())
    }

    /// Internal helper for Maverick V1 swap simulation using revm, returning detailed results including gas.
    fn _simulate_maverick_v1_detailed(
        &self,
        amount: U256,
        pool: Address,
        token_a_in: bool,
        exact_output: bool,
        tick_limit: i32,
    ) -> (U256, U256, Option<u64>) { // Returns (amountIn, amountOut, Option<gas_used>)
        let calldata = self.build_maverick_v1_calldata(amount, token_a_in, exact_output, tick_limit);

        let mut db_guard = match self.market_state.db.write() {
            Ok(guard) => guard,
            Err(_) => {
                warn!("Failed to lock DB for Maverick simulation");
                return (U256::ZERO, U256::ZERO, None);
            }
        };
        let db = &mut *db_guard;

        let cfg = CfgEnv::default();
        let block = match self.market_state.block_env.read() {
             Ok(b_guard) => b_guard.clone(),
             Err(_) => {
                 warn!("Failed to lock BlockEnv for Maverick simulation");
                 return (U256::ZERO, U256::ZERO, None);
             }
        };
        let tx = TxEnv {
             caller: address!("0000000000000000000000000000000000000001"),
             transact_to: TransactTo::Call(pool),
             data: calldata,
             value: U256::ZERO,
             gas_limit: 1_000_000,
             gas_price: U256::ZERO,
             ..Default::default()
         };

        let mut evm = Evm::builder()
            .with_db(db)
            .with_env(Box::new(Env { cfg, block, tx }))
            .build();

        match evm.transact() { // Use transact, not transact_commit, for view calls/gas estimation
            Ok(ref_tx) => match ref_tx.result {
                ExecutionResult::Success { output, gas_used, .. } => {
                    match <(U256, U256)>::abi_decode(output.data(), true) {
                        Ok((sim_amount_in, sim_amount_out)) => {
                            debug!(
                                "✅ Maverick V1 Sim Detailed: Target Amt={}, Pool={}, TokenAIn={}, ExactOut={}, TickLimit={} -> In={}, Out={}, GasUsed={}",
                                amount, pool, token_a_in, exact_output, tick_limit, sim_amount_in, sim_amount_out, gas_used
                            );
                            (sim_amount_in, sim_amount_out, Some(gas_used))
                        }
                        Err(e) => {
                            warn!("⚠️ Maverick V1 Sim Detailed Decode error: {:?}. Pool: {}, Output: {:?}", e, pool, output.data());
                            (U256::ZERO, U256::ZERO, Some(gas_used)) // Still return gas used if decode fails
                        }
                    }
                }
                ExecutionResult::Revert { output, gas_used, .. } => {
                    let reason = String::from_utf8_lossy(output.data());
                    warn!("⚠️ Maverick V1 Sim Detailed Reverted: '{}'. Pool: {}, Gas Used: {}", reason, pool, gas_used);
                    (U256::ZERO, U256::ZERO, Some(gas_used)) // Return gas used on revert
                }
                ExecutionResult::Halt { reason, gas_used, .. } => { // Halt includes gas_used
                     warn!("⚠️ Maverick V1 Sim Detailed Halted: {:?}. Pool: {}, Gas Used: {}", reason, pool, gas_used);
                    (U256::ZERO, U256::ZERO, Some(gas_used)) // Return gas used on halt
                }
                 other => {
                     warn!("⚠️ Maverick V1 Sim Detailed Unknown execution result: {:?}. Pool: {}", other, pool);
                     (U256::ZERO, U256::ZERO, None) // Gas unclear in other states
                 }
            },
            Err(e) => {
                warn!("❌ Maverick V1 Sim Detailed EVM error: {:?}. Pool: {}", e, pool);
                (U256::ZERO, U256::ZERO, None)
            }
        }
    }

     // Keep the original simulation function if needed elsewhere, or remove if detailed replaces it fully
     /*
     fn _simulate_maverick_v1(
         &self,
         amount: U256, // Input amount if exact_output=false, output amount if true
         pool: Address, // Maverick Pool contract address
         token_a_in: bool,
         exact_output: bool,
         tick_limit: i32,
     ) -> (U256, U256) { // Returns (amountIn, amountOut)
         let (sim_in, sim_out, _gas) = self._simulate_maverick_v1_detailed(amount, pool, token_a_in, exact_output, tick_limit);
         (sim_in, sim_out)
     }
     */
}