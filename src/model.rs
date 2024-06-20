use std::{
    collections::HashMap,
    io::{Read, Write},
};

use mpl_bubblegum::types::{LeafSchema, MetadataArgs};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use solana_sdk::{pubkey::Pubkey, signature::Signature};

/// Represents an off-chain compressed NFT merkle tree.
#[derive(Serialize, Deserialize, Debug)]
pub struct Rollup {
    #[serde(with = "serde_with::As::<serde_with::DisplayFromStr>")]
    pub tree_id: Pubkey,
    pub rolled_mints: Vec<RolledMintInstruction>,
    pub raw_metadata_map: HashMap<String, Box<RawValue>>, // URL of metadata -> JSON text
    pub max_depth: u32,
    pub max_buffer_size: u32,

    // derived data
    pub merkle_root: [u8; 32],    // validate
    pub last_leaf_hash: [u8; 32], // validate
}

impl Rollup {
    /// Serializes the rollup object into given destination.
    pub fn write_as_json(&self, writer: &mut dyn Write) -> serde_json::error::Result<()> {
        serde_json::to_writer(writer, self)
    }

    pub fn read_as_json(reader: impl Read) -> serde_json::error::Result<Rollup> {
        let rollup = serde_json::from_reader(reader)?;
        Ok(rollup)
    }
}

impl PartialEq for Rollup {
    fn eq(&self, other: &Self) -> bool {
        self.tree_id == other.tree_id
            && self.rolled_mints == other.rolled_mints
            //&& self.raw_metadata_map == other.raw_metadata_map
            && self.max_depth == other.max_depth
            && self.max_buffer_size == other.max_buffer_size
            && self.merkle_root == other.merkle_root
            && self.last_leaf_hash == other.last_leaf_hash
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RolledMintInstruction {
    pub tree_update: ChangeLogEventV1, // validate // derive from nonce
    pub leaf_update: LeafSchema,       // validate
    pub mint_args: MetadataArgs,
    // V0.1: enforce collection.verify == false
    // V0.1: enforce creator.verify == false
    // V0.2: add pub collection_signature: Option<Signature> - sign asset_id with collection authority
    // V0.2: add pub creator_signature: Option<Map<Pubkey, Signature>> - sign asset_id with creator authority to ensure verified creator
    #[serde(with = "serde_with::As::<serde_with::DisplayFromStr>")]
    pub authority: Pubkey,
    pub creator_signature: Option<HashMap<Pubkey, Signature>>, // signatures of the asset with the creator pubkey to ensure verified creator
}

#[derive(Default, Clone)]
pub struct BatchMintInstruction {
    pub max_depth: u32,
    pub max_buffer_size: u32,
    pub num_minted: u64,
    pub root: [u8; 32],
    pub leaf: [u8; 32],
    pub index: u32,
    pub metadata_url: String,
    pub file_checksum: String,
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
