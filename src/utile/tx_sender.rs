use crate::utile::events::Event;
use crate::utile::gas_station::GasStation;
use crate::utile::rgen::FlashSwap;
use alloy::hex;
use alloy::network::{Ethereum, Network, TransactionBuilder};
use alloy::primitives::{Address, B256, Bytes, U256};
use alloy::providers::{Provider, ProviderBuilder, RootProvider};
use alloy::rpc::types::{Signature, Transaction, TransactionRequest, TransactionReceipt};
use alloy::signers::wallet::{LocalWallet, Wallet};
use alloy::signers::PrivateKeySigner;
use alloy::transports::http::Http;
use alloy::transports::Transport;
use reqwest::{Client, Url};
use serde::{Serialize, Deserialize};
use serde_json::{json, Value};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::Receiver;
use tracing::{info, error};
use anyhow::{Context, Result};
use std::convert::TryInto;


pub struct TxSender<T> // Transport generic
where
    T: Transport + Clone + Send + Sync + 'static,
    <T as Transport>::Error: Send + Sync + 'static,
{
    provider: Arc<RootProvider<T>>,
    wallet: LocalWallet,
    contract_address: Address,
    chain_id: u64,
}


impl<T> TxSender<T>
where
    T: Transport + Clone + Send + Sync + 'static,
    <T as Transport>::Error: Send + Sync + 'static,
{
    pub async fn new(
        http_url: String,
        pk_hex: String,
        contract_address: Address,
    ) -> Result<Self> {
        // Setup Provider
        let url = Url::parse(&http_url).context("Invalid HTTP URL")?;
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .context("Failed to create HTTP client")?;
            
        let http = Http::new_with_client(url, client);
        let provider = ProviderBuilder::new()
            .provider(http);
            
        let provider = Arc::new(provider);

        // Setup Wallet
        let key_bytes = hex::decode(pk_hex.trim_start_matches("0x"))
            .context("Invalid private key hex")?;
        let wallet = LocalWallet::from_bytes(&key_bytes)
            .context("Failed to create wallet from private key bytes")?;

        // Get chain ID
        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        Ok(Self {
            provider,
            wallet,
            contract_address,
            chain_id,
        })
    }
    
    // Gets current nonce for the wallet address
    pub async fn get_nonce(&self) -> Result<u64> {
        self.provider
            .get_transaction_count(self.wallet.address())
            .await
            .context("Failed to get nonce")
    }
}



impl<T> TxSender<T>
where
    T: Transport + Clone + Send + Sync + 'static,
    <T as Transport>::Error: Send + Sync + 'static,
{
    // Builds and signs a transaction
    pub async fn build_and_sign_tx(&self, calldata: Vec<u8>) -> Result<(TransactionRequest, Signature)> {
        let nonce = self.provider
            .get_transaction_count(self.wallet.address())
            .await
            .context("Failed to get nonce for transaction")?;

        // Create transaction request with EIP-1559 fields
        let tx = TransactionRequest::default()
            .with_to(self.contract_address)
            .with_nonce(nonce)
            .with_chain_id(self.chain_id)
            .with_gas_limit(500_000)
            .with_max_fee_per_gas(U256::from(20_000_000_000u128)) // 20 gwei
            .with_max_priority_fee_per_gas(U256::from(1_000_000_000u128)) // 1 gwei
            .with_input(Bytes::from(calldata));

        // Calculate transaction hash and sign it
        use alloy::rpc::types::tx::TxEnvelope;
        let envelope: TxEnvelope = tx.clone().try_into()
            .context("Failed to convert TransactionRequest to TxEnvelope")?;
            
        // Set chain ID if not already set
        let tx_hash = envelope.tx_hash();
        let signature = self.wallet.sign_hash(&tx_hash)
            .await
            .context("Failed to sign transaction hash")?;

        Ok((tx, signature))
    }

    // Gets RLP bytes for a signed transaction
    pub fn get_signed_rlp(&self, tx: &TransactionRequest, signature: &Signature) -> Result<Bytes> {
        use alloy::rpc::types::Signed;
        use alloy::rlp::Encodable;

        // Create a Signed transaction object
        let signed_tx = Signed::new_unchecked(
            tx.clone(),
            *signature,
            tx.tx_hash(),
        );

        let mut rlp_buf = Vec::new();
        signed_tx.encode(&mut rlp_buf);
        Ok(Bytes::from(rlp_buf))
    }

    // Sends a single pre-signed, RLP-encoded transaction
    pub async fn send_raw_tx(&self, rlp_bytes: Bytes) -> Result<B256> {
        self.provider
            .send_raw_transaction(&rlp_bytes)
            .await
            .context("Failed to send raw transaction")
    }

    // Main method to send a transaction
    pub async fn send_tx(&self, calldata: Vec<u8>) -> Result<B256> {
        // Build and sign the transaction
        let (tx, signature) = self.build_and_sign_tx(calldata).await?;
        
        // Get RLP encoded bytes
        let rlp_bytes = self.get_signed_rlp(&tx, &signature)?;
        
        // Send the transaction
        let tx_hash = self.send_raw_tx(rlp_bytes).await?;
        
        info!("Transaction sent: {}", tx_hash);
        
        Ok(tx_hash)
    }
    
    // Optional: Monitor transaction receipt
    pub async fn wait_for_receipt(&self, tx_hash: B256) -> Result<Option<TransactionReceipt>> {
        let receipt = self.provider
            .get_transaction_receipt(tx_hash)
            .await
            .context("Failed to get transaction receipt")?;
            
        if let Some(inner) = &receipt {
            if let Some(block_num) = inner.block_number {
                info!("Transaction included in block: {}", block_num);
            }
        }
        
        Ok(receipt)
    }
}
