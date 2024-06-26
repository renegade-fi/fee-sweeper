//! Client code for interacting with a configured relayer

use std::time::Duration;

use base64::engine::{general_purpose as b64_general_purpose, Engine};
use ethers::{
    core::k256::ecdsa::{signature::Signer, Signature, SigningKey},
    signers::LocalWallet,
};
use http::{HeaderMap, HeaderValue};
use renegade_api::{
    http::{
        price_report::{GetPriceReportRequest, GetPriceReportResponse, PRICE_REPORT_ROUTE},
        task::{GetTaskStatusResponse, GET_TASK_STATUS_ROUTE},
        wallet::{
            CreateWalletRequest, CreateWalletResponse, FindWalletRequest, FindWalletResponse,
            GetWalletResponse, RedeemNoteRequest, RedeemNoteResponse, CREATE_WALLET_ROUTE,
            FIND_WALLET_ROUTE, GET_WALLET_ROUTE, REDEEM_NOTE_ROUTE,
        },
    },
    RENEGADE_AUTH_HEADER_NAME, RENEGADE_SIG_EXPIRATION_HEADER_NAME,
};
use renegade_circuit_types::keychain::SecretSigningKey;
use renegade_common::types::{
    exchange::PriceReporterState,
    token::Token,
    wallet::{
        derivation::{
            derive_blinder_seed, derive_share_seed, derive_wallet_id, derive_wallet_keychain,
        },
        Wallet, WalletIdentifier,
    },
};
use renegade_crypto::fields::scalar_to_biguint;
use renegade_util::{get_current_time_millis, raw_err_str};
use reqwest::{Body, Client};
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

/// The interval at which to poll relayer task status
const POLL_INTERVAL_MS: u64 = 1000;
/// The amount of time (ms) to declare a wallet signature value for
const SIG_EXPIRATION_BUFFER_MS: u64 = 5000;

/// A client for interacting with a configured relayer
pub struct RelayerClient {
    /// The base URL of the relayer
    base_url: String,
    /// The mind of the USDC token
    usdc_mint: String,
}

impl RelayerClient {
    /// Create a new relayer client
    pub fn new(base_url: &str, usdc_mint: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            usdc_mint: usdc_mint.to_string(),
        }
    }

    /// Get the price for a given mint
    pub async fn get_binance_price(&self, mint: &str) -> Result<Option<f64>, String> {
        if mint == self.usdc_mint {
            return Ok(Some(1.0));
        }

        let body = GetPriceReportRequest {
            base_token: Token::from_addr(mint),
            quote_token: Token::from_addr(&self.usdc_mint),
        };
        let response: GetPriceReportResponse = self.post_relayer(PRICE_REPORT_ROUTE, &body).await?;

        match response.price_report {
            PriceReporterState::Nominal(report) => Ok(Some(report.price)),
            state => {
                warn!("Price report state: {state:?}");
                Ok(None)
            }
        }
    }

    // ------------------
    // | Wallet Methods |
    // ------------------

    /// Check that the relayer has a given wallet, lookup the wallet if not
    pub async fn check_wallet_indexed(
        &self,
        wallet_id: WalletIdentifier,
        chain_id: u64,
        eth_key: &LocalWallet,
    ) -> Result<(), String> {
        let mut path = GET_WALLET_ROUTE.to_string();
        path = path.replace(":wallet_id", &wallet_id.to_string());

        let keychain = derive_wallet_keychain(eth_key, chain_id).unwrap();
        let root_key = keychain.secret_keys.sk_root.unwrap();
        if self
            .get_relayer_with_auth::<GetWalletResponse>(&path, &root_key)
            .await
            .is_ok()
        {
            return Ok(());
        }

        // Otherwise lookup the wallet
        self.lookup_wallet(chain_id, eth_key).await
    }

    /// Lookup a wallet in the configured relayer
    async fn lookup_wallet(&self, chain_id: u64, eth_key: &LocalWallet) -> Result<(), String> {
        let path = FIND_WALLET_ROUTE.to_string();
        let wallet_id = derive_wallet_id(eth_key).unwrap();
        let blinder_seed = derive_blinder_seed(eth_key).unwrap();
        let share_seed = derive_share_seed(eth_key).unwrap();
        let keychain = derive_wallet_keychain(eth_key, chain_id).unwrap();
        let root_key = keychain.secret_keys.sk_root.clone().unwrap();

        let body = FindWalletRequest {
            wallet_id,
            secret_share_seed: scalar_to_biguint(&share_seed),
            blinder_seed: scalar_to_biguint(&blinder_seed),
            key_chain: keychain.into(),
        };

        let resp: FindWalletResponse = self.post_relayer_with_auth(&path, &body, &root_key).await?;
        self.await_relayer_task(resp.task_id).await
    }

    /// Create a new wallet via the configured relayer
    pub(crate) async fn create_new_wallet(&self, wallet: Wallet) -> Result<(), String> {
        let body = CreateWalletRequest {
            wallet: wallet.into(),
        };

        let resp: CreateWalletResponse = self.post_relayer(CREATE_WALLET_ROUTE, &body).await?;
        self.await_relayer_task(resp.task_id).await
    }

    /// Redeem a note into a wallet
    pub(crate) async fn redeem_note(
        &self,
        wallet_id: WalletIdentifier,
        req: RedeemNoteRequest,
        root_key: &SecretSigningKey,
    ) -> Result<(), String> {
        let mut path = REDEEM_NOTE_ROUTE.to_string();
        path = path.replace(":wallet_id", &wallet_id.to_string());

        let resp: RedeemNoteResponse = self.post_relayer_with_auth(&path, &req, root_key).await?;
        self.await_relayer_task(resp.task_id).await
    }

    // -----------
    // | Helpers |
    // -----------

    /// Post to the relayer URL
    async fn post_relayer<Req, Resp>(&self, path: &str, body: &Req) -> Result<Resp, String>
    where
        Req: Serialize,
        Resp: for<'de> Deserialize<'de>,
    {
        self.post_relayer_with_headers(path, body, &HeaderMap::new())
            .await
    }

    /// Post to the relayer with wallet auth
    async fn post_relayer_with_auth<Req, Resp>(
        &self,
        path: &str,
        body: &Req,
        root_key: &SecretSigningKey,
    ) -> Result<Resp, String>
    where
        Req: Serialize,
        Resp: for<'de> Deserialize<'de>,
    {
        let body_ser =
            serde_json::to_vec(body).map_err(raw_err_str!("Failed to serialize body: {}"))?;
        let headers = build_auth_headers(root_key, &body_ser)?;
        self.post_relayer_with_headers(path, body, &headers).await
    }

    /// Post to the relayer with given headers
    async fn post_relayer_with_headers<Req, Resp>(
        &self,
        path: &str,
        body: &Req,
        headers: &HeaderMap,
    ) -> Result<Resp, String>
    where
        Req: Serialize,
        Resp: for<'de> Deserialize<'de>,
    {
        // Send a request
        let client = reqwest_client()?;
        let route = format!("{}{}", self.base_url, path);
        let resp = client
            .post(route)
            .json(body)
            .headers(headers.clone())
            .send()
            .await
            .map_err(raw_err_str!("Failed to send request: {}"))?;

        // Deserialize the response
        if !resp.status().is_success() {
            return Err(format!("Failed to send request: {}", resp.status()));
        }

        resp.json::<Resp>()
            .await
            .map_err(raw_err_str!("Failed to parse response: {}"))
    }

    /// Get from the relayer URL
    async fn get_relayer<Resp>(&self, path: &str) -> Result<Resp, String>
    where
        Resp: for<'de> Deserialize<'de>,
    {
        self.get_relayer_with_headers(path, &HeaderMap::new()).await
    }

    /// Get from the relayer URL with wallet auth
    async fn get_relayer_with_auth<Resp>(
        &self,
        path: &str,
        root_key: &SecretSigningKey,
    ) -> Result<Resp, String>
    where
        Resp: for<'de> Deserialize<'de>,
    {
        let headers = build_auth_headers(root_key, &[])?;
        self.get_relayer_with_headers(path, &headers).await
    }

    /// Get from the relayer URL with given headers
    async fn get_relayer_with_headers<Resp>(
        &self,
        path: &str,
        headers: &HeaderMap,
    ) -> Result<Resp, String>
    where
        Resp: for<'de> Deserialize<'de>,
    {
        let client = reqwest_client()?;
        let url = format!("{}{}", self.base_url, path);
        let resp = client
            .get(url)
            .headers(headers.clone())
            .send()
            .await
            .map_err(raw_err_str!("Failed to get relayer path: {}"))?;

        // Parse the response
        if !resp.status().is_success() {
            return Err(format!("Failed to get relayer path: {}", resp.status()));
        }

        resp.json::<Resp>()
            .await
            .map_err(raw_err_str!("Failed to parse response: {}"))
    }

    /// Await a relayer task
    async fn await_relayer_task(&self, task_id: Uuid) -> Result<(), String> {
        let mut path = GET_TASK_STATUS_ROUTE.to_string();
        path = path.replace(":task_id", &task_id.to_string());

        // Enter a polling loop until the task finishes
        let poll_interval = Duration::from_millis(POLL_INTERVAL_MS);
        loop {
            // For now, we assume that an error is a 404 in which case the task has completed
            // TODO: Improve this break condition if it proves problematic
            if self
                .get_relayer::<GetTaskStatusResponse>(&path)
                .await
                .is_err()
            {
                break;
            }

            // Sleep for a bit before polling again
            std::thread::sleep(poll_interval);
        }

        Ok(())
    }
}

// -----------
// | Helpers |
// -----------

/// Build a reqwest client
fn reqwest_client() -> Result<Client, String> {
    Client::builder()
        .user_agent("fee-sweeper")
        .build()
        .map_err(raw_err_str!("Failed to create reqwest client: {}"))
}

/// Build authentication headers for a request
fn build_auth_headers(key: &SecretSigningKey, req_bytes: &[u8]) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    let expiration = get_current_time_millis() + SIG_EXPIRATION_BUFFER_MS;
    headers.insert(RENEGADE_SIG_EXPIRATION_HEADER_NAME, expiration.into());

    let root_key: SigningKey = key.try_into()?;

    // Sign the concatenation of the message and the expiration timestamp
    let body = Body::from(req_bytes.to_vec());
    let msg_bytes = body.as_bytes().unwrap();
    let payload = [msg_bytes, &expiration.to_le_bytes()].concat();

    let signature: Signature = root_key.sign(&payload);
    let encoded_sig = b64_general_purpose::STANDARD_NO_PAD.encode(signature.to_bytes());

    headers.insert(
        RENEGADE_AUTH_HEADER_NAME,
        HeaderValue::from_str(&encoded_sig).unwrap(),
    );

    Ok(headers)
}
