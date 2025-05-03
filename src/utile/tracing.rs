use alloy::sol_types::sol;
use alloy::primitives::Address;
use alloy::rpc::types::trace::geth::GethDebugTracingOptions;
use alloy::rpc::types::trace::common::TraceResult;
use alloy::eips::BlockNumberOrTag;
use std::collections::BTreeMap;
use std::sync::Arc;

use alloy::providers::Network;
use tracing::{trace, warn, error};
use reth_node_ethereum::DebugApi;
use reth::revm::revm::bytecode::Bytecode;
use reth::revm::revm::primitives::Bytes;
use reth::revm::revm::state::AccountInfo;
use alloy::consensus::constants::KECCAK_EMPTY;
use reth::revm::db::AccountState;
use reth::rpc::api::DebugApiServer::debug_trace_block;
use reth_tracing::RethTracer;
use reth_config::config::PruneStageConfig;

/// Vector of address-to-account-state maps representing post-trace changes.
pub async fn debug_trace_block<N>(
    client: Arc<impl DebugApi<N> + Send + Sync>,
    block_tag: BlockNumberOrTag,
    diff_mode: bool,
) -> Vec<BTreeMap<Address, AccountState>>
where
    N: Network,
{
    // Create a tracer instance
    let tracer = RethTracer::default();

    // Call debug_trace_block on the client with the tracer and options
    let results = client
        .debug_trace_block(block_tag, tracer, PruneStageConfig::default())
        .await
        .expect("Failed to trace block");

    // Process results to extract post-trace changes
    let mut post: Vec<BTreeMap<Address, AccountState>> = Vec::new();

    for trace_result in results.into_iter() {
        post.push(trace_result);
    }

    post
}
