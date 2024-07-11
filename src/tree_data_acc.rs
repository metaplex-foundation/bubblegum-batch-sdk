//! This module contains util functionality for parsing out the information
//! from a merkle tree data account

use std::mem::size_of;

use crate::{
    errors::RollupError,
    merkle_tree_wrapper::{calc_merkle_tree_size, restore_canopy_depth_from_buffer},
};
use mpl_bubblegum::{accounts::MerkleTree, types::ConcurrentMerkleTreeHeaderData};
use spl_account_compression::state::CONCURRENT_MERKLE_TREE_HEADER_SIZE_V1;
use spl_merkle_tree_reference::{Node, EMPTY};

/// Information about merkle tree stored in a solana account
pub struct TreeDataInfo<'a> {
    pub max_depth: u32,
    pub max_buffer_size: u32,
    pub canopy_depth: u32,
    pub canopy_leaves_count: usize,
    pub canopy_buffer: &'a [u8],
}

impl<'a> TreeDataInfo<'a> {
    /// Parses raw bytes taken from the Solana account that contains merkle tree
    /// create by prepare_tree bubblegum instruction.
    ///
    /// ## Arguments:
    /// * `bytes` - raw bytes received as `solana_client.get_account(pubkey).unwrap().data()`
    pub fn from_bytes(bytes: &'a [u8]) -> std::result::Result<TreeDataInfo, RollupError> {
        let merkle_tree = MerkleTree::from_bytes(bytes)?;
        let (max_depth, max_buffer_size) = match merkle_tree.tree_header {
            ConcurrentMerkleTreeHeaderData::V1 {
                max_buffer_size,
                max_depth,
                authority: _,
                creation_slot: _,
                padding: _,
            } => (max_depth, max_buffer_size),
        };

        // Calculate the size of the merkle tree without the canopy. This will define the offset of the canopy buffer.
        let merkel_tree_size = calc_merkle_tree_size(max_depth, max_buffer_size, 0)
            .ok_or(RollupError::UnexpectedTreeSize(max_depth, max_buffer_size))?;

        let (_header, rest) = bytes.split_at(CONCURRENT_MERKLE_TREE_HEADER_SIZE_V1);
        let (_tree_body, canopy_buffer) = rest.split_at(merkel_tree_size);

        let canopy_depth = restore_canopy_depth_from_buffer(canopy_buffer.len() as u32);
        let canopy_leaves_count = 1 << canopy_depth;

        Ok(TreeDataInfo {
            max_depth,
            max_buffer_size,
            canopy_depth,
            canopy_leaves_count,
            canopy_buffer,
        })
    }

    /// Returns a sequence of non-empy canopy leaf nodes that previously had been added
    /// using `add_canopy` bubblegum instruction.
    pub fn non_empty_canopy_leaves(&self) -> std::result::Result<Vec<&'a Node>, RollupError> {
        let node_size = size_of::<Node>();
        let leaves_start_position = self.canopy_buffer.len() - (1 << self.canopy_depth) * node_size;
        let leaves_buffer = &self.canopy_buffer[leaves_start_position..];

        let mut canopy_leaves: Vec<&'a Node> = Vec::with_capacity(self.canopy_leaves_count);
        for i in 0..self.canopy_leaves_count {
            match leaves_buffer[node_size * i..node_size * i + node_size].try_into() {
                Ok(canopy_leaf) => {
                    if canopy_leaf == &EMPTY {
                        break;
                    }
                    canopy_leaves.push(canopy_leaf);
                }
                Err(_) => return Err(RollupError::CanopyCoercionErr),
            }
        }
        Ok(canopy_leaves)
    }
}
