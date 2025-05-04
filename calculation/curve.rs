use crate::calculation::Calculator;
// Import necessary types from state_db module
use crate::state_db::{BlockStateDB, blockstate_db::AccountInfo}; // Adjust path/name if needed

use alloy::network::Network;
use alloy::primitives::{address, Address, Bytes, U256}; // Fix: Import Bytes struct
use alloy::providers::Provider;
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue}; // SolValue needed for <U256>::abi_decode

// Correct imports for revm v22.0.1
use revm::primitives::{ExecutionResult, TransactTo};
use revm::{Database, Evm}; // Use top-level Evm and Database trait

use std::collections::HashMap;
use tracing::{info, warn};

sol! {
    #[sol(rpc)]
    contract CurveOut {
        // i: index of token in
        // j: index of token out
        // dx: amount of token in
        function get_dy(uint256 i, uint256 j, uint256 dx) external view returns (uint256);
    }
}

impl<N, P> Calculator<N, P>
where
    N: Network,
    P: Provider<N>, // Fix: Add correct Provider trait bound
{
    /// Simulates Curve's `get_dy` offchain using revm.
    /// Assumes the `pool` address is the Curve pool contract.
    pub fn curve_out(
        &self,
        index_in: U256,
        index_out: U256,
        amount_in: U256,
        pool: Address,
    ) -> U256 {
        // Prepare calldata for the get_dy view call
        let calldata = CurveOut::get_dyCall {
            i: index_in,
            j: index_out,
            dx: amount_in,
        }
        .abi_encode(); // Returns Vec<u8>

        // Get write access to the database via market_state
        let mut db_guard = self.market_state.db.write().expect("Failed to acquire DB write lock");
        let db = &mut *db_guard; // Get mutable reference to the DB

        // Setup EVM for simulation
        let mut evm = Evm::builder()
            .with_db(db) // Provide the database implementation
            .modify_tx_env(|tx| {
                tx.caller = address!("0000000000000000000000000000000000000001"); // Arbitrary caller
                tx.transact_to = TransactTo::Call(pool); // Target Curve pool contract
                tx.data = Bytes::from(calldata); // Convert Vec<u8> to revm::primitives::Bytes
                tx.value = U256::ZERO;
                tx.gas_limit = 1_000_000; // Set a reasonable gas limit for the view call
                // tx.gas_price = U256::ZERO; // For view calls, gas price isn't strictly needed
            })
            // Potentially modify_block_env if needed (e.g., timestamp)
            .build();

        // --- Optional: Snapshot before execution ---
        // Cloning the accounts map might be expensive depending on its size.
        // let pre_snapshot = db.accounts.clone(); // Assuming db has 'accounts' field

        // Execute the transaction simulation
        let tx_result = match evm.transact() {
            Ok(ref_tx) => ref_tx.result, // ResultAndState or similar
            Err(err) => {
                warn!(?pool, %amount_in, "CurveOut simulation EVM error: {:?}", err);
                return U256::ZERO;
            }
        };

        // --- Optional: State delta analysis ---
        // self.analyze_state_changes(&pre_snapshot, db, pool); // Pass the post-state db


        // Process the simulation result
        match tx_result {
            ExecutionResult::Success { output, gas_used, .. } => {
                debug!(?pool, %amount_in, %gas_used, "CurveOut simulation success.");
                // Decode the output Bytes
                match <U256>::abi_decode(output.as_ref(), false) { // Use as_ref() on revm::Bytes
                    Ok(amount_out) => amount_out,
                    Err(e) => {
                        warn!(?pool, %amount_in, "CurveOut decoding failed: {:?}. Output: {:?}", e, output);
                        U256::ZERO
                    }
                }
            }
            ExecutionResult::Revert { output, gas_used, .. } => {
                // Try to decode revert reason?
                warn!(?pool, %amount_in, %gas_used, "CurveOut simulation reverted: {:?}", output);
                U256::ZERO
            }
            other => {
                warn!(?pool, %amount_in, "Unexpected CurveOut execution result: {:?}", other);
                U256::ZERO
            }
        }
    }

    /// Checks if a Curve swap results in zero output (potential edge case).
    pub fn is_curve_edge_case_zero(
        &self,
        index_in: U256,
        index_out: U256,
        amount_in: U256,
        pool: Address,
    ) -> bool {
        let out = self.curve_out(index_in, index_out, amount_in, pool);
        if out == U256::ZERO && amount_in > U256::ZERO { // Only log if input > 0
            info!(
                "⚠️ Detected edge case in Curve pool {:?}: get_dy({}, {}, {}) == 0",
                pool, index_in, index_out, amount_in
            );
            true
        } else {
            false
        }
    }

    /// Calculates Curve output amount after applying a fee to the input amount.
    pub fn curve_out_with_fee_adjustment(
        &self,
        index_in: U256,
        index_out: U256,
        amount_in: U256,
        pool: Address,
        fee_basis_points: u64, // e.g., 4 for 0.04%
    ) -> U256 {
        // Curve fees are typically basis points (out of 10,000)
        let fee = (amount_in * U256::from(fee_basis_points)) / U256::from(10_000u64);
        let adjusted_amount = amount_in.saturating_sub(fee);
        if adjusted_amount.is_zero() && amount_in > U256::ZERO {
            return U256::ZERO; // Entire amount taken as fee
        }
        self.curve_out(index_in, index_out, adjusted_amount, pool)
    }

    /// Helper to analyze EVM state difference after call.
    /// Requires the specific structure of your BlockStateDB and its AccountInfo.
    fn analyze_state_changes(
        &self,
        pre_state: &HashMap<Address, AccountInfo>, // Use AccountInfo from state_db::blockstate_db
        post_state_db: &BlockStateDB<N, P>,       // Pass the db *after* transact
        pool: Address,
    ) {
        // Access the accounts map in the post-state DB
        if let Some(post_acc_info) = post_state_db.accounts.get(&pool) {
            if let Some(pre_acc_info) = pre_state.get(&pool) {
                // Compare storage slots
                for (slot, post_val) in &post_acc_info.storage {
                    match pre_acc_info.storage.get(slot) {
                        Some(pre_val) => {
                            if pre_val.value != post_val.value {
                                info!(
                                    "🔍 Pool {} - Slot {} changed from {} -> {}",
                                    pool, slot, pre_val.value, post_val.value
                                );
                            }
                        }
                        None => {
                            info!(
                                "🆕 New slot {} added to pool {}: {}",
                                slot, pool, post_val.value
                            );
                        }
                    }
                }
                 // Compare other account fields if needed (balance, nonce, code_hash)
                 if pre_acc_info.info.balance != post_acc_info.info.balance {
                    info!(
                        "💰 Pool {} - Balance changed from {} -> {}",
                        pool, pre_acc_info.info.balance, post_acc_info.info.balance
                    );
                 }
                // Add more comparisons as needed...
            } else {
                info!("⚠️ Account for pool {} was created during simulation!", pool);
            }
        } else if pre_state.contains_key(&pool) {
             info!("⚠️ Account for pool {} was deleted during simulation!", pool);
        }
        // else: Account didn't exist before or after, no changes related to it.
    }
}