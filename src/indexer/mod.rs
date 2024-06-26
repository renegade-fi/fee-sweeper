//! The indexer handles the indexing and redemption of fee notes

use arbitrum_client::{client::ArbitrumClient, constants::Chain};
use aws_config::SdkConfig as AwsConfig;
use diesel::PgConnection;
use renegade_circuit_types::elgamal::DecryptionKey;

use crate::relayer_client::RelayerClient;

pub mod index_fees;
pub mod queries;
pub mod redeem_fees;

/// Stores the dependencies needed to index the chain
pub(crate) struct Indexer {
    /// The id of the chain this indexer targets
    pub chain_id: u64,
    /// The chain this indexer targets
    pub chain: Chain,
    /// A client for interacting with the relayer
    pub relayer_client: RelayerClient,
    /// The Arbitrum client
    pub arbitrum_client: ArbitrumClient,
    /// The decryption key
    pub decryption_key: DecryptionKey,
    /// A connection to the DB
    pub db_conn: PgConnection,
    /// The AWS config
    pub aws_config: AwsConfig,
}

impl Indexer {
    /// Constructor
    pub fn new(
        chain_id: u64,
        chain: Chain,
        aws_config: AwsConfig,
        arbitrum_client: ArbitrumClient,
        decryption_key: DecryptionKey,
        db_conn: PgConnection,
        relayer_client: RelayerClient,
    ) -> Self {
        Indexer {
            chain_id,
            chain,
            arbitrum_client,
            decryption_key,
            db_conn,
            relayer_client,
            aws_config,
        }
    }
}
