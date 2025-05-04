use crate::utile::events::Event;
//use alloy::providers::{IpcConnect, Provider, ProviderBuilder};
use alloy_provider::ProviderBuilder;
use alloy_transport_ipc::IpcConnect; // Add impor
use futures::StreamExt;
use log::{debug, warn};
use tokio::sync::broadcast::Sender;

/// Subscribes to new block headers over IPC and broadcasts them via a channel.
pub async fn stream_new_blocks(block_sender: Sender<Event>) {
        
        // ...
let ipc_conn: String = ...;
let ipc_builder = IpcConnect::new(ipc_conn.clone()); // Create builder specific to IPC
let ipc_transport = ipc_builder.connect().await.context("Failed to connect IPC")?;
let ipc_provider = ProviderBuilder::new().provider(ipc_transport);
let ipc = Arc::new(ipc_provider);
// ...
    // 👇 Attempt to load IPC path from environment
    let ipc_path = std::env::var("IPC").expect("IPC path not set in environment");

    // 👇 Subscribe to new block headers
    let sub = match ipc.subscribe_blocks().await {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to subscribe to new blocks: {:?}", e);
            return;
        }
    };

    let mut stream = sub.into_stream();

    // 👇 Stream and broadcast each new block as an Event
    while let Some(block) = stream.next().await {
        match block_sender.send(Event::NewBlock(block)) {
            Ok(_) => debug!("New block event sent"),
            Err(e) => warn!("Failed to broadcast new block: {:?}", e),
        }
    }
}
