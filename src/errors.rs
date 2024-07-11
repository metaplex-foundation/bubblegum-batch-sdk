use anchor_lang::error;
use solana_sdk::pubkey::ParsePubkeyError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RollupError {
    #[error("Solana client error: {0}")]
    SolanaClientErr(#[from] solana_rpc_client_api::client_error::Error),
    // #[error("Solana RPC client error: {0}")]
    // SolanaRpcCleintErr(#[from] solana_client::client_error::ClientError),
    #[error("Merkle tree bytes parsing error: {0}")]
    UnableToParseTreeErr(#[from] std::io::Error),
    #[error("Unexpected tree depth={0} and max size={1}")]
    UnexpectedTreeSize(u32, u32),
    #[error("Illegal arguments: {0}")]
    IllegalArgumets(String),
    #[error("I/O error: {0}")]
    IoError(std::io::Error),
    #[error("Cannot parse pubkey: {0}")]
    InvalidPubKey(#[from] ParsePubkeyError),
    #[error("Generic error: {0}")]
    GenricErr(String),
    #[error("Nester error: {0}")]
    NestedErr(Box<dyn std::error::Error>),
    #[error("Failed signature verification for creator: {0}")]
    InvalidCreatorsSignature(String),
    #[error("Missing rolled mint with ID: {0}")]
    MissingRolledMint(u64),
    #[error("Extra creators were passed for verification")]
    ExtraCreatorsReceived,
    #[error("Missed signatures for asset: {0}")]
    MissedSignaturesForAsset(String),
    #[error("Missed signature from creator: {0}")]
    MissedSignatureFromCreator(String),
    #[error("Cannot add signature for unverified creator: {0}")]
    CannotAddSignatureForUnverifiedCreator(String),
    #[error("Cannot parse canopy leaf nodes from tree data account")]
    CanopyCoercionErr,
}
