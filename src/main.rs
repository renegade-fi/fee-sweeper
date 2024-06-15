//! The fee sweeper, sweeps for unredeemed fees in the Renegade protocol and redeems them
#![deny(missing_docs)]
#![deny(clippy::missing_docs_in_private_items)]
#![deny(unsafe_code)]
#![deny(clippy::needless_pass_by_ref_mut)]
#![feature(trivial_bounds)]

pub mod index_fees;
pub mod models;
#[allow(missing_docs)]
pub mod schema;

use diesel::{pg::PgConnection, Connection};
use ethers::signers::LocalWallet;
use renegade_circuit_types::elgamal::DecryptionKey;
use renegade_util::telemetry::{setup_system_logger, LevelFilter};

use std::{error::Error, str::FromStr};

use arbitrum_client::{
    client::{ArbitrumClient, ArbitrumClientConfig},
    constants::Chain,
};
use clap::Parser;

/// The block polling interval for the Arbitrum client
const BLOCK_POLLING_INTERVAL_MS: u64 = 100;
/// The metadata key for the last indexed block
pub(crate) const LAST_INDEXED_BLOCK_KEY: &str = "latest_block";

/// The cli for the fee sweeper
#[derive(Debug, Parser)]
struct Cli {
    /// The Arbitrum RPC url to use
    #[clap(short, long)]
    rpc_url: String,
    /// The address of the darkpool contract
    #[clap(short = 'a', long)]
    darkpool_address: String,
    /// The chain to redeem fees for
    #[clap(long, default_value = "mainnet")]
    chain: Chain,
    /// The fee decryption key to use
    #[clap(short, long)]
    decryption_key: String,
    /// The arbitrum private key used to submit transactions
    #[clap(long = "pkey")]
    arbitrum_private_key: String,
    /// The database url
    #[clap(long)]
    db_url: String,
}

impl Cli {
    /// Build a connection to the DB
    pub fn build_db_conn(&self) -> Result<PgConnection, String> {
        PgConnection::establish(&self.db_url).map_err(|e| e.to_string())
    }
}

/// Stores the dependencies needed to index the chain
pub(crate) struct Indexer {
    /// The Arbitrum client
    pub client: ArbitrumClient,
    /// The decryption key
    pub decryption_key: DecryptionKey,
    /// A connection to the DB
    pub db_conn: PgConnection,
}

impl Indexer {
    /// Constructor
    pub fn new(
        client: ArbitrumClient,
        decryption_key: DecryptionKey,
        db_conn: PgConnection,
    ) -> Self {
        Indexer {
            client,
            decryption_key,
            db_conn,
        }
    }
}

/// Main
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    setup_system_logger(LevelFilter::INFO);
    let cli = Cli::parse();
    let db_conn = cli.build_db_conn()?;

    // Build an Arbitrum client
    let wallet = LocalWallet::from_str(&cli.arbitrum_private_key)?;
    let conf = ArbitrumClientConfig {
        darkpool_addr: cli.darkpool_address,
        chain: cli.chain,
        rpc_url: cli.rpc_url,
        arb_priv_keys: vec![wallet],
        block_polling_interval_ms: BLOCK_POLLING_INTERVAL_MS,
    };
    let client = ArbitrumClient::new(conf).await?;

    // Build the indexer
    let key = DecryptionKey::from_hex_str(&cli.decryption_key)?;
    let mut indexer = Indexer::new(client, key, db_conn);

    // 1. Index all new fees in the DB
    indexer.index_fees().await?;

    // 2. Redeem fees according to the redemption policy
    // TODO: Implement this

    Ok(())
}
