
use crate::calculation::state_db::blockstate_db::{BlockStateDB, InsertionType};
use crate::utile::constant::AMOUNT;
use crate::utile::events::Event;
use crate::utile::rgen::{ERC20Token, FlashQuoter};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use alloy::network::Network;
use alloy::primitives::Keccak256;
use alloy::primitives::{Address, U256, address};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol_types::{SolCall, SolValue};
use alloy::transports::http::Http, Client, RootProvider;
use anyhow::{Context, Result};
use pool_sync::{Pool, PoolInfo};
use reth::primitives::Bytecode;
use reth::revm::revm::context::Evm;
use reth::revm::revm::context::TransactTo;
use reth::revm::revm::state::AccountInfo;
use reth::rpc::types::BlockNumberOrTag;
use tokio::sync::{
    RwLock,
    mpsc::{Receiver, Sender},
};
use tracing::{debug, error, info};
use tracing::debug_trace_block;

pub struct NamedAccountInfo {
    pub name: String,
}

// State manager for live blockchain pool information
pub struct MarketState<N, P>
where
    N: Network,
    P: Provider<N>,
{
    pub db: RwLock<BlockStateDB<N, P>>,
}

impl<N, P> MarketState<N, P>
where
    N: Network,
    P: Provider<N> + Clone + Send + Sync + 'static,
{
    pub async fn init_state_and_start_stream(
        pools: Vec<Pool>,
        block_rx: Receiver<Event>, // ✅ Must match tokio::sync::mpsc
        address_tx: Sender<Event>, // <-- must be tokio::mpsc::Sender
        last_synced_block: u64,
        provider: P,
        caught_up: Arc<AtomicBool>,
    ) -> Result<Arc<Self>> {
        debug!("Populating the db with {} pools", pools.len());

        let mut db = BlockStateDB::new(provider).context("Failed to initialize BlockStateDB")?;
        Self::warm_up_database(&pools, &mut db);
        Self::populate_db_with_pools(pools, &mut db);

        let market_state = Arc::new(Self {
            db: RwLock::new(db),
        });

        tokio::spawn(Self::state_updater(
            market_state.clone(),
            block_rx,
            address_tx,
            last_synced_block,
            caught_up,
        ));

        Ok(market_state)
    }

    fn warm_up_database(pools: &[Pool], db: &mut BlockStateDB<N, P>) {
        let account = address!("d8da6bf26964af9d7eed9e03e53415d37aa96045");
        let quoter = address!("0000000000000000000000000000000000001000");

        let ten_units = U256::from(10_000_000_000_000_000_000u128);
        let balance_slot = Keccak256::hash(&(account, U256::from(3)).abi_encode());

        let quoter_bytecode = FlashQuoter::DEPLOYED_BYTECODE.clone();
        let quoter_info = AccountInfo {
            nonce: 0,
            balance: U256::ZERO,
            code_hash: Keccak256::hash(&quoter_bytecode),
            code: Some(Bytecode::new_raw(quoter_bytecode)),
        };
        db.insert_account_info(quoter, quoter_info, InsertionType::Custom);

        for pool in pools {
            db.insert_account_storage(
                pool.token0_address(),
                balance_slot.into(),
                ten_units,
                InsertionType::OnChain,
            )
            .unwrap();

            let approve = ERC20Token::approveCall {
                spender: quoter,
                amount: U256::from(1e18),
            }
            .abi_encode();

            let mut evm = Evm::new(&mut *db, (), ());
            evm.modify_tx_env(|tx| {
                tx.caller = account;
                tx.data = approve.into();
                tx.transact_to = TransactTo::Call(pool.token0_address());
            });

            evm.transact_commit().unwrap();

            let quote_path = FlashQuoter::SwapParams {
                pools: vec![pool.address()],
                poolVersions: vec![if pool.is_v3() { 1 } else { 0 }],
                amountIn: *AMOUNT.read().unwrap(),
            };

            let quote_call = FlashQuoter::quoteArbitrageCall { params: quote_path }.abi_encode();

            evm.tx_mut().data = quote_call.into();
            evm.tx_mut().transact_to = TransactTo::Call(quoter);

            evm.transact().unwrap();
        }
    }

    async fn state_updater(
        self: Arc<Self>,
        mut block_rx: Receiver<Event>,
        address_tx: Sender<Event>,
        mut last_synced_block: u64,
        caught_up: Arc<AtomicBool>,
    ) {
        let http: Arc<RootProvider<Http<Client>>> = Arc::new(...)
        let http_url = std::env::var("FULL").unwrap(); // assumed validated externally
        let http = Arc::new(ProviderBuilder::connect_http(http_url.parse().unwrap()).await);

        let mut current_block = http.get_block_number().await.unwrap();

        while last_synced_block < current_block {
            debug!(
                "Catching up from {} to {}",
                last_synced_block, current_block
            );
            for block_num in (last_synced_block + 1)..=current_block {
                let _ = self.update_state(http.clone(), block_num).await;
            }
            last_synced_block = current_block;
            current_block = http.get_block_number().await.unwrap();
        }

        caught_up.store(true, Ordering::Relaxed);
        while let Some(Event::NewBlock(block_header)) = block_rx.recv().await {
            let start = Instant::now();
            let block_number = block_header.inner.number;

            if block_number <= last_synced_block {
                debug!("Skipping duplicate block {}", block_number);
                continue;
            }

            info!("New block received: {}", block_number);
            let updated = self.update_state(http.clone(), block_number).await;

            if let Err(e) = address_tx
                .send(Event::PoolsTouched(updated.clone(), block_number))
                .await
            {
                error!("Error sending updates: {}", e);
            } else {
                info!("Block {} processed in {:?}", block_number, start.elapsed());
            }

            last_synced_block = block_number;
        }
    }

    fn populate_db_with_pools(pools: Vec<Pool>, db: &mut BlockStateDB<N, P>) {
        for pool in pools {
            if pool.is_v2() {
                db.insert_v2(pool);
            } else if pool.is_v3() {
                db.insert_v3(pool).unwrap();
            }
        }
    }

    async fn update_state(
        &self,
        provider: Arc<dyn Provider<Http<Client>>>,
        block_num: u64,
    ) -> HashSet<Address> {
        let mut updated_pools = HashSet::new();
        let updates = debug_trace_block(provider, BlockNumberOrTag::Number(block_num), true).await;

        let mut db = self.db.write().unwrap();
        for (addr, state) in updates.iter().flat_map(|map| map.iter()) {
            if db.tracking_pool(addr) {
                db.update_all_slots(*addr, state.clone()).unwrap();
                updated_pools.insert(*addr);
            }
        }

        updated_pools
    }
}
