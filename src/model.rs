use std::{
    collections::HashMap,
    io::{Read, Write},
};

use mpl_bubblegum::types::{LeafSchema, MetadataArgs};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_with::DisplayFromStr;
use solana_sdk::signature::Keypair;
use solana_sdk::{pubkey::Pubkey, signature::Signature};

/// Represents an off-chain compressed NFT merkle tree, that can be uploaded to
/// an immutable storage, and picked up by DAS validatiors, that verify the correctness
/// of a batch mint.
/// This type is used only for providing the batch mint data to DAS validators,
/// all the off-chain batch mint changes should be done via BatchMintBuilder.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BatchMint {
    #[serde(with = "serde_with::As::<serde_with::DisplayFromStr>")]
    pub tree_id: Pubkey,
    pub batch_mints: Vec<BatchMintInstruction>,
    pub raw_metadata_map: HashMap<String, Box<RawValue>>, // URL of metadata -> JSON text
    pub max_depth: u32,
    pub max_buffer_size: u32,

    // derived data
    pub merkle_root: [u8; 32],    // validate
    pub last_leaf_hash: [u8; 32], // validate
}

impl BatchMint {
    /// Serializes the batch mint object into given destination.
    pub fn write_as_json(&self, writer: &mut dyn Write) -> serde_json::error::Result<()> {
        serde_json::to_writer(writer, self)
    }

    pub fn read_as_json(reader: impl Read) -> serde_json::error::Result<BatchMint> {
        let batch_mint = serde_json::from_reader(reader)?;
        Ok(batch_mint)
    }
}

impl PartialEq for BatchMint {
    fn eq(&self, other: &Self) -> bool {
        self.tree_id == other.tree_id
            && self.batch_mints == other.batch_mints
            && self.max_depth == other.max_depth
            && self.max_buffer_size == other.max_buffer_size
            && self.merkle_root == other.merkle_root
            && self.last_leaf_hash == other.last_leaf_hash
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BatchMintInstruction {
    pub tree_update: ChangeLogEventV1, // validate // derive from nonce
    pub leaf_update: LeafSchema,       // validate
    pub mint_args: MetadataArgs,
    #[serde(with = "serde_with::As::<serde_with::DisplayFromStr>")]
    pub authority: Pubkey,
    #[serde(with = "serde_with::As::<Option<HashMap<DisplayFromStr, DisplayFromStr>>>")]
    pub creator_signature: Option<HashMap<Pubkey, Signature>>, // signatures of the asset with the creator pubkey to ensure verified creator
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChangeLogEventV1 {
    #[serde(with = "serde_with::As::<serde_with::DisplayFromStr>")]
    pub id: Pubkey,
    pub path: Vec<PathNode>,
    pub seq: u64,
    pub index: u32,
}

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug)]
pub struct PathNode {
    pub node: [u8; 32],
    pub index: u32,
}

impl From<spl_account_compression::state::PathNode> for PathNode {
    fn from(value: spl_account_compression::state::PathNode) -> Self {
        Self {
            node: value.node,
            index: value.index,
        }
    }
}

pub struct CollectionConfig {
    pub collection_authority: Keypair,
    pub collection_authority_record_pda: Option<Pubkey>,
    pub collection_mint: Pubkey,
    pub collection_metadata: Pubkey,
    pub edition_account: Pubkey,
}
