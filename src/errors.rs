use thiserror::Error;

#[derive(Error, Debug)]
pub enum RollupError {
    #[error("Solana client error: {0}")]
    SolanaClientErr(#[from] solana_rpc_client_api::client_error::Error),
    #[error("Merkle tree bytes parsing error: {0}")]
    UnableToParseTreeErr(#[from] std::io::Error),
    #[error("Unexpected tree depth={0} and max buffer size={1}")]
    UnexpectedTreeSize(u32, u32),
    #[error("Illegal arguments: {0}")]
    IllegalArgumets(String),
}
