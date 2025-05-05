use alloy::consensus::constants::KECCAK_EMPTY;
use alloy::primitives::{Address, B256, StorageKey, U256};
use eyre::{Context, Result};
use reth::api::NodeTypesWithDBAdapter;
use reth::primitives::{Bytecode, H256};
use reth::providers::{
    AccountReader, ProviderFactory, StateProviderBox, StateProviderFactory,
    providers::StaticFileProvider, BytecodeReader, StorageReader, ProviderError,
};
use revm::primitives::{AccountInfo, Bytecode as RevmBytecode, DBErrorMarker};
use revm::{Database, DatabaseRef};
use reth::utils::open_db_read_only;
use reth_chainspec::ChainSpecBuilder;
use reth_db::{ClientVersion, DatabaseEnv, mdbx::DatabaseArguments};
use reth_node_ethereum::EthereumNode;
use std::{path::Path, sync::Arc, fmt};
use thiserror::Error;

// --- Custom Error Type ---
#[derive(Error, Debug)]
pub enum HistoryDbError {
    #[error("Reth provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("Eyre error: {0}")]
    Eyre(#[from] eyre::Report),

    #[error("Data conversion error: {0}")]
    Conversion(String),
}

// Implement DBErrorMarker for revm::Database::Error bound
impl DBErrorMarker for HistoryDbError {}

/// Core struct that provides access to historical state from Reth database.
pub struct HistoryDB {
    db_provider: StateProviderBox,
    provider_factory: ProviderFactory<NodeTypesWithDBAdapter<EthereumNode, Arc<DatabaseEnv>>>,
}

impl HistoryDB {
    /// Constructs a new HistoryDB for a given database path and block number
    pub fn new(db_path: String, block: u64) -> Result<Self> {
        let db_path = Path::new(&db_path);

        // Open the database in read-only mode
        let db = Arc::new(
            open_db_read_only(
                db_path.join("db"),
                DatabaseArguments::new(ClientVersion::default()),
            )
            .wrap_err("Failed to open DB in read-only mode")?,
        );

        // Construct the mainnet ChainSpec
        let spec = Arc::new(ChainSpecBuilder::mainnet().build());

        // Load static file provider (used for history lookups)
        let static_provider = StaticFileProvider::read_only(db_path.join("static_files"), true)
            .wrap_err("Failed to open StaticFileProvider")?;

        // Construct ProviderFactory for state access
        let factory = ProviderFactory::new(db.clone(), spec.clone(), static_provider);

        let provider = factory
            .history_by_block_number(block)
            .wrap_err_with(|| format!("Failed to load historical state at block {}", block))?;

        Ok(Self {
            db_provider: provider,
            provider_factory: factory,
        })
    }
}

// === revm Database Implementation ===
impl Database for HistoryDB {
    type Error = HistoryDbError;

    fn basic(&mut self, address: Address) -> std::result::Result<Option<AccountInfo>, Self::Error> {
        DatabaseRef::basic_ref(self, address)
    }

    fn code_by_hash(
        &mut self,
        code_hash: B256,
    ) -> std::result::Result<RevmBytecode, Self::Error> {
        DatabaseRef::code_by_hash_ref(self, code_hash)
    }

    fn storage(&mut self, address: Address, index: U256) -> std::result::Result<U256, Self::Error> {
        DatabaseRef::storage_ref(self, address, index)
    }

    fn block_hash(&mut self, number: u64) -> std::result::Result<B256, Self::Error> {
        DatabaseRef::block_hash_ref(self, number)
    }
}

// === revm DatabaseRef Implementation ===
impl DatabaseRef for HistoryDB {
    type Error = HistoryDbError;

    fn basic_ref(&self, address: Address) -> std::result::Result<Option<AccountInfo>, Self::Error> {
        let reth_account_opt = self.db_provider.basic_account(&address)
            .map_err(HistoryDbError::Provider)?;

        match reth_account_opt {
            Some(account) => {
                // Get code hash or use empty hash if not available
                let code_hash = account.bytecode_hash.unwrap_or_else(|| H256::from_slice(&KECCAK_EMPTY.0));
                let code_hash_b256 = B256::from(code_hash.0);

                // Fetch code
                let code = self.db_provider.account_code(&address)
                    .map_err(HistoryDbError::Provider)?;

                let account_info = match code {
                    Some(code) => AccountInfo {
                        balance: account.balance,
                        nonce: account.nonce,
                        code_hash: code_hash_b256,
                        code: Some(RevmBytecode::new_raw(code.original_bytes())),
                    },
                    None => AccountInfo {
                        balance: account.balance,
                        nonce: account.nonce,
                        code_hash: code_hash_b256,
                        code: Some(RevmBytecode::new()),
                    },
                };

                Ok(Some(account_info))
            },
            None => Ok(None),
        }
    }

    fn code_by_hash_ref(
        &self,
        code_hash: B256,
    ) -> std::result::Result<RevmBytecode, Self::Error> {
        if code_hash == KECCAK_EMPTY {
            return Ok(RevmBytecode::new());
        }
        
        let code_hash_h256 = H256::from(code_hash.0);
        let bytecode = self.db_provider.bytecode_by_hash(code_hash_h256)
            .map_err(HistoryDbError::Provider)?;
            
        match bytecode {
            Some(code) => Ok(RevmBytecode::new_raw(code.bytes().clone())),
            None => {
                // Return empty bytecode if not found
                Ok(RevmBytecode::new())
            }
        }
    }

    fn storage_ref(&self, address: Address, index: U256) -> std::result::Result<U256, Self::Error> {
        let key = StorageKey::from(index);
        let value = self.db_provider.storage(address, key)
            .map_err(HistoryDbError::Provider)?;
        Ok(value.unwrap_or_default())
    }

    fn block_hash_ref(&self, number: u64) -> std::result::Result<B256, Self::Error> {
        match self.db_provider.block_hash(number)
            .map_err(HistoryDbError::Provider)? {
            Some(hash) => Ok(B256::from(hash.0)),
            None => Ok(KECCAK_EMPTY),
        }
    }
}
