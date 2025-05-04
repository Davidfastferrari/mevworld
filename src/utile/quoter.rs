use crate::utile::constant::AMOUNT;
use crate::utile::rgen::{FlashQuoter, FlashSwap};
use crate::utile::MarketState;
use alloy::rlp::Decodable;
use anyhow::Result;
use std::sync::Arc;
use tracing::{info, warn};
use alloy::network::Ethereum;
use alloy::primitives::{U256, address};
use alloy::providers::RootProvider;
use alloy::sol_types::SolCall;
use reth::revm::revm::ExecutionResult;
 use alloy_transport_http::Http;
use reth::revm::revm::context::Evm;
use reth::revm::revm::context::TransactTo;

/// Quoter – runs an EVM simulation to quote arbitrage profitability.
pub struct Quoter;

impl Quoter {
    /// Runs a simulated EVM call on the provided quote path.
    pub fn quote_path(
        quote_params: FlashQuoter::SwapParams,
        market_state: Arc<MarketState<Ethereum, RootProvider<Http>>>,
    ) -> Result<Vec<U256>, anyhow::Error> {
        let mut guard = market_state.db.write().unwrap();

        let mut evm = Evm::new(&mut *guard, (), ());

        evm.tx_mut().caller = address!("d8da6bf26964af9d7eed9e03e53415d37aa96045");
        evm.tx_mut().transact_to =
            TransactTo::Call(address!("0000000000000000000000000000000000001000"));

        let calldata = FlashQuoter::quoteArbitrageCall {
            params: quote_params,
        }
        .abi_encode();

        evm.tx_mut().data = calldata.into();

        // Run the transaction
        match evm.transact().map(|tx| tx.result) {
            Ok(ExecutionResult::Success { output, .. }) => {
                match Vec::<U256>::decode(output.data()) {
                    Ok(decoded) => Ok(decoded),
                    Err(e) => {
                        warn!("❌ ABI decode failed: {e:?}");
                        Err(anyhow::anyhow!("Failed to decode EVM output"))
                    }
                }
            }
            Ok(ExecutionResult::Revert { output, .. }) => {
                warn!("🚫 Simulation reverted with output: {:?}", output);
                Err(anyhow::anyhow!("Simulation reverted"))
            }
            Ok(_) => {
                warn!("🤔 Unexpected simulation result");
                Err(anyhow::anyhow!("Unexpected EVM result"))
            }
            Err(e) => {
                warn!("🔥 Simulation transaction failed: {:?}", e);
                Err(anyhow::anyhow!("Simulation failure"))
            }
        }
    }

    /// Optimizes the input amount via binary search to maximize profitability.
    /// Returns a `(best_input, best_output)` pair.
    pub fn optimize_input(
        mut quote_path: FlashQuoter::SwapParams,
        initial_out: U256,
        market_state: Arc<MarketState<Ethereum, RootProvider<Http>>>,
    ) -> (U256, U256) {
        let mut best_input = *AMOUNT.read().unwrap();
        let mut best_output = initial_out;
        let mut curr_input = *AMOUNT.read().unwrap();

        let step = U256::from(200000000000000u128); // ✅ precise 2e14 step

        for _ in 0..50 {
            curr_input += step;
            quote_path.amountIn = curr_input;

            match Self::quote_path(quote_path.clone(), market_state.clone()) {
                Ok(amounts) => {
                    if let Some(&output) = amounts.last() {
                        if output > curr_input && output > best_output {
                            best_output = output;
                            best_input = curr_input;
                            continue;
                        }
                    }
                    // If output not better, stop early
                    break;
                }
                Err(e) => {
                    info!("Binary search early exit: {e}");
                    break;
                }
            }
        }

        (best_input, best_output)
    }
}
