use crate::utile::events::Event;
use crate::utile::gas_station::GasStation;
use crate::utile::rgen::FlashSwap;
use alloy::hex;
use alloy::network::{Ethereum, Network, TransactionBuilder};
use alloy::primitives::{Address, B256, Bytes, U256};
use alloy::providers::{Provider, ProviderBuilder, RootProvider};
use alloy::rpc::types::TransactionRequest; // Use concrete type
use alloy::signers::wallet::{LocalWallet, Wallet};
use alloy::signers::PrivateKeySigner;
use alloy::transports::http::Http; // Or other transport
use alloy::transports::Transport;
use reqwest::{Client, Url}; // If using Http transport
use std::str::FromStr;
use std::sync::Arc;
use anyhow::{Context, Result};
use std::convert::TryInto; // For slice conversion



#[derive(Serialize, Deserialize, Debug)]
struct Point {
    x: i32,
    y: i32,
}

pub struct TxSender<T> // Simplified generic for Transport
where
    // T: Transport + Clone, // Transport bound needed for provider
     T: Transport + Clone + Send + Sync + 'static, // Add Send/Sync static
{
    provider: Arc<RootProvider<T>>,
    wallet: LocalWallet, // Use LocalWallet which wraps PrivateKeySigner
    contract_address: Address,
    chain_id: u64,
}

// Handles sending transactions
pub struct TransactionSender<HttpClient> {
    wallet: EthereumWallet<PrivateKeySigner>,
    gas_station: Arc<GasStation>,
    contract_address: Address,
    client: Arc<Client>,
    provider: Arc<RootProvider<impl Network>>,
    nonce: u64,
}

impl<T> TxSender<T>
where
    // T: Transport + Clone,
     T: Transport + Clone + Send + Sync + 'static,
     // Add error type bounds if RootProvider requires them
     <T as alloy::transports::Transport>::Error: Send + Sync + 'static,

{
    pub async fn new(
        http_url: String,
        pk_hex: String,
        contract_address: Address,
    ) -> Result<Self> {
        // Setup Provider
        let url = Url::parse(&http_url).context("Invalid HTTP URL")?;
        // Assuming Http<Client> transport
        let client = Client::new();
        // Define provider type explicitly for annotation below
        let provider : RootProvider<Http<Client>> = ProviderBuilder::new()
             // .with_recommended_fillers() // Consider fillers
             .provider(Http::new_with_client(url, client));

        let provider = Arc::new(provider);

        // Setup Wallet
        let key_bytes = hex::decode(pk_hex.trim_start_matches("0x"))
            .context("Invalid private key hex")?;
        // Use LocalWallet::from_bytes
         let wallet = LocalWallet::from_bytes(&key_bytes)
             .context("Failed to create wallet from private key bytes")?;


        // Get chain ID and nonce
        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        // let nonce = provider
        //     .get_transaction_count(wallet.address())
        //     .await
        //     .context("Failed to get initial nonce")?;

        Ok(Self {
            provider: provider as Arc<RootProvider<T>>, // May need unsafe cast or different structure if T isn't Http<Client>
            wallet,
            contract_address,
            chain_id,
        })
    }
}

impl<HttpClient> TransactionSender<HttpClient> {
    pub async fn new(gas_station: Arc<GasStation>) -> Self {
        // construct a wallet
        let key = std::env::var("PRIVATE_KEY").expect("PRIVATE_KEY not set");
        let key_bytes = hex::decode(&key).expect("Invalid hex");
        let signer = PrivateKeySigner::from_bytes(&key_bytes).expect("Invalid private key bytes");
        let wallet = EthereumWallet::from(signer);

        // Create persistent reqwest client
        let client = Client::builder()
            .pool_max_idle_per_host(10)
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("Failed to create reqwest client");

        // Warm-up request
        let warmup_json = json!({
            "jsonrpc": "2.0",
            "method": "eth_blockNumber",
            "params": [],
            "id": 1
        });
        let _ = client
            .post("https://mainnet-sequencer.base.org")
            .json(&warmup_json)
            .send()
            .await
            .unwrap();

        // setup provider
        let http_url = std::env::var("FULL").expect("FULL env var not set");
        let provider =
            Arc::new(ProviderBuilder::connect_http(Url::parse(&http_url).unwrap()).await);

        let nonce = provider
            .get_transaction_count(std::env::var("ACCOUNT").unwrap().parse().unwrap())
            .await
            .unwrap();

        Self {
            wallet,
            gas_station,
            contract_address: std::env::var("SWAP_CONTRACT").unwrap().parse().unwrap(),
            client: Arc::new(client),
            provider,
            nonce,
        }
    }

    pub async fn send_transactions(&mut self, mut tx_receiver: Receiver<Event>) {
        while let Some(Event::ValidPath((arb_path, profit, block_number))) =
            tx_receiver.recv().await
        {
            info!("Sending path...");

            let converted_path: FlashSwap::SwapParams = arb_path.clone().into();
            let calldata = FlashSwap::executeArbitrageCall {
                arb: converted_path,
            }
            .abi_encode();

            let (max_fee, priority_fee) = self.gas_station.get_gas_fees(profit);

            let tx = <dyn TransactionBuilder<_>>::default()
                .with_to(self.contract_address)
                .with_nonce(self.nonce)
                .with_gas_limit(2_000_000)
                .with_chain_id(8453)
                .with_max_fee_per_gas(max_fee)
                .with_max_priority_fee_per_gas(priority_fee)
                .transaction_type(2)
                .with_input(AlloyBytes::from(calldata));
            self.nonce += 1;

            let tx_envelope = tx.build(&self.wallet).await.unwrap();
            let mut encoded_tx = vec![];
            tx_envelope.encode_2718(&mut encoded_tx);
            let rlp_hex = hex::encode(encoded_tx);

            let tx_data = json!({
                "jsonrpc": "2.0",
                "method": "eth_sendRawTransaction",
                "params": [rlp_hex],
                "id": 1
            });

            info!("Sending on block {}", block_number);
            let start = Instant::now();

            let req = self
                .client
                .post("https://mainnet-sequencer.base.org")
                .json(&tx_data)
                .send()
                .await
                .unwrap();
            let req_response: Value = req.json().await.unwrap();
            info!("Took {:?} to send tx and receive response", start.elapsed());
            let tx_hash =
            B256::from_str(req_response["result"].as_str().unwrap()).unwrap();

            let provider = self.provider.clone();
            tokio::spawn(async move {
                Self::send_and_monitor(provider, tx_hash, block_number).await;
            });
        }
    }

    pub async fn send_and_monitor(
        provider: Arc<RootProvider<impl Network>>,
        tx_hash: FixedBytes<32>,
        block_number: u64,
    ) {
        let mut attempts = 0;
        while attempts < 10 {
            let receipt = provider.get_transaction_receipt(tx_hash).await;
            if let Ok(Some(inner)) = receipt {
                info!(
                    "Sent on block {:?}, Landed on block {:?}",
                    block_number,
                    inner.block_number.unwrap()
                );
                return;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
            attempts += 1;
        }
    }
}


pub async fn send_tx(&self, calldata: Vec<u8>) -> Result<B256> {
    let nonce = self.provider
        .get_transaction_count(self.wallet.address())
        .await
        .context("Failed to get nonce for transaction")?;

    // Use TransactionRequest instead of dyn TransactionBuilder
    let tx = TransactionRequest::default()
        .with_to(self.contract_address) // Use with_to
        .with_nonce(nonce)
        .with_chain_id(self.chain_id)
        .with_gas_limit(500_000) // Set appropriate gas limit
        // .with_gas_price(...) // Set gas price or EIP-1559 fields
        .with_max_fee_per_gas(U256::from(20_000_000_000u128)) // Example EIP-1559 (20 gwei)
        .with_max_priority_fee_per_gas(U256::from(1_000_000_000u128)) // Example EIP-1559 (1 gwei)
        .with_input(Bytes::from(calldata)); // Use alloy::Bytes

    // Sign transaction
     let signature = self.wallet.sign_transaction(&tx.clone().into()).await // Convert TxRequest to SignableTx
         .context("Failed to sign transaction")?;

    // Send raw transaction
    let tx_hash = self.provider
        .send_raw_transaction(&tx.clone().into_signed(signature)) // Use into_signed
        .await
        .context("Failed to send raw transaction")?;

    info!("Transaction sent: {}", tx_hash);

    // Wait for receipt (optional)
    // let receipt = self.provider.get_transaction_receipt(tx_hash).await.context("Failed to get receipt")?;
    // if let Some(inner) = receipt {
    //      let block_num = inner.block_number.unwrap_or_default(); // Access block_number
    //      info!("Transaction included in block: {}", block_num);
    // } else {
    //      warn!("Transaction receipt not found yet.");
    // }

    Ok(tx_hash) // Return hash immediately
}