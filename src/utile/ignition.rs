use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::Duration,
};
// Removed unstable std mpmc channel import
// use std::sync::mpmc::channel;
use crate::utile::{
    estimator::Estimator, events::Event, filter::filter_pools, gas_station::GasStation,
    graph::ArbGraph, market_state::MarketState, searcher::Searchoor, stream::stream_new_blocks,
    tx_sender::TransactionSender,
};
use alloy::providers::ProviderBuilder;
//use alloy_provider::{ProviderBuilder, Provider};
use log::{error, info, warn};
use pool_sync::{Chain, Pool};
use tokio::signal;
use tokio::sync::{
    broadcast,
    mpsc::{Receiver, Sender, channel},
};
use alloy_transport_http::Http;
use reqwest::Client;
use anyhow::Context;
use alloy::providers::Provider;
use alloy::network::Network;

/// Bootstraps the entire system: syncing, simulation, and arbitrage search
pub async fn start_workers(pools: Vec<Pool>, last_synced_block: u64) {
    let (block_sender, _) = broadcast::channel::<Event>(100);
    let (block_tx, mut block_rx): (Sender<Event>, Receiver<Event>) = channel(100);
    let (address_sender, address_receiver): (Sender<Event>, Receiver<Event>) = channel(100);
    let (paths_sender, paths_receiver): (Sender<Event>, Receiver<Event>) = channel(100);
    let (profitable_sender, profitable_receiver): (Sender<Event>, Receiver<Event>) = channel(100);

    // Graceful shutdown channel
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

    // --- Pool Filtering ---
    info!("Pool count before filtering: {}", pools.len());
    let pools = filter_pools(pools, 4000, Chain::Base).await.context("Failed to filter pools")?;
    info!("Pool count after filtering: {}", pools.len());

    // --- Block Event Proxy ---
    {
        let mut block_subscriber = block_sender.subscribe();
        let block_tx = block_tx.clone();
        tokio::spawn(async move {
            while let Ok(event) = block_subscriber.recv().await {
                if block_tx.send(event).await.is_err() {
                    break;
                }
            }
        });
    }

    // --- Streamer to push new blocks into broadcast ---
    tokio::spawn(stream_new_blocks(block_sender.clone()));

    // --- Gas Station ---
    let gas_station = Arc::new(GasStation::new());
    {
        let gas_station = Arc::clone(&gas_station);
        let mut block_gas_sub = block_sender.subscribe();
        tokio::spawn(async move {
            gas_station.update_gas(block_gas_sub).await;
        });
    }

    // --- State Catch-up Flag ---
    let caught_up = Arc::new(AtomicBool::new(false));

    // --- Market State ---
    info!("Initializing market state...");
    let http_url_str = std::env::var("FULL").context("FULL env var not set")?;
    let http_url = http_url_str.parse::<reqwest::Url>().context("Failed to parse FULL env var as URL")?;
    // Assuming Http transport using reqwest client
    let http_client = Client::new();
    let provider = ProviderBuilder::new()
        // .with_recommended_fillers() // Consider adding fillers
        .provider(alloy_transport_http::Http::new_with_client(http_url, http_client));
    let provider = Arc::new(provider); // Wrap in Arc

    let market_state = MarketState::init_state_and_start_stream(
        pools.clone(),
        block_rx,
        address_sender.clone(),
        last_synced_block,
        provider,
        Arc::clone(&caught_up),
    )
    .await
    .expect("Failed to initialize market state");

    info!("Market state initialized!");

    // --- Wait for catch-up ---
    info!("Waiting for block sync before initializing estimator...");
    while !caught_up.load(Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // --- Estimator Init ---
    info!("Calculating initial rates...");
    let mut estimator = Estimator::new(Arc::clone(&market_state));
    estimator.process_pools(pools.clone());
    info!("Initial rates calculated!");

    // --- Arbitrage Cycles ---
    info!("Generating arbitrage cycles...");
    let cycles = ArbGraph::generate_cycles(pools.clone()).await;
    info!("Generated {} arbitrage cycles", cycles.len());

    // --- Simulator ---
    {
        let ms = Arc::clone(&market_state);
        let profitable_sender = profitable_sender.clone();
        tokio::spawn(simulate_paths(profitable_sender, paths_receiver, ms));
    }

    // --- Searcher ---
    {
        let mut searcher = Searchoor::new(cycles, Arc::clone(&market_state), estimator);
        tokio::spawn(async move {
            if let Err(e) = searcher.search_paths(paths_sender, address_receiver).await {
                error!("Searcher failed: {:?}", e);
            }
        });
    }

    // --- Transaction Sender ---
    {
        let mut tx_sender = TransactionSender::new(Arc::clone(&gas_station)).await;
        tokio::spawn(async move {
            tx_sender.send_transactions(profitable_receiver).await;
        });
    }

    // --- Graceful Shutdown Handler ---
    tokio::spawn(async move {
        if let Err(err) = signal::ctrl_c().await {
            error!("Failed to listen for shutdown: {:?}", err);
        }
        info!("🛑 Ctrl-C detected. Shutting down...");
        let _ = shutdown_tx.send(());
    });

    // --- Await Shutdown Signal ---
    let _ = shutdown_rx.recv().await;
    info!("🚪 All workers will now terminate.");
}

async fn simulate_paths(
    // Define channel types precisely
    profitable_sender: tokio::sync::mpsc::Sender<()>,
    paths_receiver: tokio::sync::mpsc::Receiver<()>,
    ms: Arc<crate::utile::MarketState<impl Network + Send + Sync + 'static, impl Provider<impl Network + Send + Sync + 'static> + Send + Sync + 'static>> // Adjust generics
) {
     warn!("simulate_paths function is not implemented");
     // Loop paths_receiver, simulate, send to profitable_sender
}


