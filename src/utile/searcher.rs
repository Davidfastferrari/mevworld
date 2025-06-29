use crate::calculation::calculator;
use crate::utile::constant::AMOUNT;
use crate::utile::estimator::Estimator;
use crate::utile::events::Event;
use crate::utile::market_state::MarketState;
use crate::utile::swap::SwapPath;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::{debug, info};
use alloy::network::Network;
use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use anyhow::Context;
//use super::utills::calculation::calculator;

/// Top-level search engine for arbitrage cycles
pub struct Searchoor<N, P>
where
    N: Network,
    P: Provider<N>,
{
    calculator: calculator::Calculator<N, P>,
    estimator: Estimator<N, P>,
    path_index: HashMap<Address, Vec<usize>>,
    cycles: Vec<SwapPath>,
    min_profit: U256,
}

impl<N, P> Searchoor<N, P>
where
    N: Network,
    P: Provider<N>,
{
    pub fn new(
        cycles: Vec<SwapPath>,
        market_state: Arc<MarketState<N, P>>,
        estimator: Estimator<N, P>,
    ) -> Self {
        let calculator = calculator::Calculator::new(market_state);

        // 🧠 Precompute pool index mapping
        let mut index: HashMap<Address, Vec<usize>> = HashMap::new();
        for (i, path) in cycles.iter().enumerate() {
            for step in &path.steps {
                index.entry(step.pool_address).or_default().push(i);
            }
        }

        // 💰 Minimum profit is loan repayment + 1% buffer
        let initial_amount = *AMOUNT.read().unwrap();
        let flash_loan_fee = (initial_amount * U256::from(9)) / U256::from(10000);
        let repayment_amount = initial_amount + flash_loan_fee;
        let min_profit_percentage = (initial_amount * U256::from(1)) / U256::from(100);
        let min_profit = repayment_amount + min_profit_percentage;

        Self {
            calculator,
            estimator,
            cycles,
            path_index: index,
            min_profit,
        }
    }

    /// Search for profitable paths whenever a new block update is received

    pub async fn search_paths(
        &mut self,
        mut paths_tx: Sender<Event>,
        mut address_rx: Receiver<Event>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let _sim: bool = std::env::var("SIM")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(false);

        while let Some(Event::PoolsTouched(pools, block_number)) = address_rx.recv().await {
            info!("🧠 Searching block {}...", block_number);
            let res = Instant::now();

            self.calculator.invalidate_cache(&pools);
            self.estimator.update_rates(&pools);
            info!("📈 Estimations updated");

            // 🧠 Collect only relevant paths
            let affected_paths: HashSet<&SwapPath> = pools
                .iter()
                .filter_map(|pool| self.path_index.get(pool))
                .flatten()
                .map(|&idx| &self.cycles[idx])
                .collect();

            info!("🔍 {} paths touched", affected_paths.len());

            let profitable_paths: Vec<(SwapPath, U256)> = affected_paths
                .par_iter()
                .filter_map(|path| {
                    let output_est = self.estimator.estimate_output_amount(path);
                    if output_est >= self.min_profit
                        && output_est < U256::from_str("1000000000000000000").unwrap()
                    {
                        Some(((*path).clone(), output_est))
                    } else {
                        None
                    }
                })
                .collect();

            info!("⏱️ Estimation took {:?}", res.elapsed());
            info!("💎 {} profitable paths found", profitable_paths.len());

            if let Some(best_path) = profitable_paths.iter().max_by_key(|(_, amt)| amt) {
                let swap_path: &SwapPath = &best_path.0;
                let first_step = swap_path.steps.first().context("Empty path")?;
                let input_amount = swap_path.input_amount;
                let pool_address = first_step.pool_address;
                let token_in = first_step.token_in;
                let pool_type = first_step.pool_type;
                let fee = first_step.fee;
                
                let calculated_out = self.calculator.compute_amount_out(
                     input_amount,
                     pool_address,
                     token_in,
                     pool_type,
                     fee
                );

                if calculated_out >= self.min_profit {
                    info!("✅ Best estimated {}, real {}", best_path.1, calculated_out);

                    if let Err(e) = paths_tx
                        .send(Event::ArbPath((
                            best_path.0.clone(),
                            calculated_out,
                            block_number,
                        )))
                        .await
                    {
                        debug!("⚠️ Failed to send path: {:?}", e);
                    } else {
                        debug!("📤 Sent profitable path");
                    }
                }
            }
        }
        Ok(())
    }
}
