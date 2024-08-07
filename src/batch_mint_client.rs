use std::collections::HashMap;
use std::sync::Arc;

use mpl_bubblegum::accounts::MerkleTree;
use mpl_bubblegum::instructions::{
    AddCanopyBuilder, FinalizeTreeWithRootAndCollectionBuilder, FinalizeTreeWithRootBuilder, PrepareTreeBuilder,
};
use mpl_bubblegum::types::{ConcurrentMerkleTreeHeaderData, LeafSchema};
use solana_sdk::account::{Account, ReadableAccount};
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::AccountMeta;
use solana_sdk::signature::Signature;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;
use spl_merkle_tree_reference::Node;

use crate::batch_mint_builder::BatchMintBuilder;
use crate::errors::BatchMintError;
use crate::merkle_tree_wrapper::{
    calc_merkle_tree_size, calc_tree_data_account_size, restore_canopy_depth_from_buffer,
};
use crate::model::{BatchMint, BatchMintInstruction};
use crate::pubkey_util;
use crate::tree_data_acc::TreeDataInfo;

use solana_sdk::pubkey::Pubkey;
use solana_sdk::{system_instruction, system_program};

use solana_client::nonblocking::rpc_client::RpcClient;
use solana_program::instruction::Instruction;

const CANOPY_NODES_PER_TX: usize = 24;

/// The main controll point for batch mint creation flows.
/// It allows to:
/// 1) Create a merkle tree account for a batch mint
/// 2) Add assets (NFT) to the batch mint off-chain
/// 3) Push the batch mint to a Solana account in a form of a bubblegum tree
///
/// TODO: add link to batch mint documentation page.
pub struct BatchMintClient {
    client: Arc<RpcClient>,
}

impl BatchMintClient {
    /// Creates a new instance that allows to create batch mints.
    pub fn new(client: Arc<RpcClient>) -> BatchMintClient {
        BatchMintClient { client }
    }

    pub fn client(&self) -> &RpcClient {
        &self.client
    }

    /// Prepares solana accounts (space) for future merkle tree.
    /// This is the first step of the flow of creating a compressed NFT aka BatchMint.
    /// See https://developers.metaplex.com/bubblegum/create-trees
    /// for detail about merle tree.
    ///
    /// # Arguments
    /// * `payer` - account that pays for the operation
    /// * `tree_creator` - owner of tree account to be created
    /// * `tree_data_account` - a desired address for the account that will be created by the call
    ///   and used to store the merkle tree
    /// * `max_depth` - depth of desired merkle tree. Should be in range: TODO: add
    /// * `max_buf_size` - maximum buffer size which defines max. num. of concurrent changes
    /// * `canopy_depth` - desired depth of canopy tree
    ///
    /// Note, by design, an asset leaf cannot require more than 17 proofs, which means
    /// that for a big trees (bigger than 17), there should be a canopy at least
    /// of (tree depth - 17) size.
    pub async fn prepare_tree(
        &self,
        payer: &Keypair,
        tree_creator: &Keypair,
        tree_data_account: &Keypair,
        max_depth: u32,
        max_buf_size: u32,
        canopy_depth: u32,
    ) -> std::result::Result<Signature, BatchMintError> {
        if canopy_depth >= max_depth {
            return Err(BatchMintError::IllegalArgumets(
                "Canopy depth should be less than tree maximum depth".to_string(),
            ));
        }

        let required_canopy = max_depth.saturating_sub(bubblegum::state::MAX_ACC_PROOFS_SIZE);
        if canopy_depth < required_canopy {
            return Err(BatchMintError::IllegalArgumets(format!(
                "Three of depth={max_depth} requires as least canopy={required_canopy}"
            )));
        }

        let merkle_tree_size = calc_tree_data_account_size(max_depth, max_buf_size, canopy_depth)
            .ok_or(BatchMintError::UnexpectedTreeSize(max_depth, max_buf_size))?;

        let tree_config_account = pubkey_util::derive_tree_config_account(&tree_data_account.pubkey());

        let tx = Transaction::new_signed_with_payer(
            &[
                system_instruction::create_account(
                    // acquire space for future merkle tree
                    &payer.pubkey(),
                    &tree_data_account.pubkey(),
                    self.client
                        .get_minimum_balance_for_rent_exemption(merkle_tree_size)
                        .await?,
                    merkle_tree_size as u64,
                    &spl_account_compression::id(),
                ),
                PrepareTreeBuilder::new()
                    .payer(tree_creator.pubkey())
                    .tree_creator(tree_creator.pubkey())
                    .max_depth(max_depth)
                    .max_buffer_size(max_buf_size)
                    .merkle_tree(tree_data_account.pubkey())
                    .tree_config(tree_config_account)
                    .log_wrapper(spl_noop::id())
                    .compression_program(spl_account_compression::id())
                    .system_program(system_program::id())
                    .instruction(),
            ],
            Some(&payer.pubkey()),
            &[payer, tree_creator, tree_data_account],
            self.client.get_latest_blockhash().await?,
        );

        let tx_signature = self.client.send_and_confirm_transaction(&tx).await?;

        // PrepareTree is a well tested functionality, but still the call can use the signature
        // to check the transaction state and be sure it has been applied successfully.
        Ok(tx_signature)
    }

    /// Creates a batch mint builder object - a convenient wrapper for adding assets to batch mints.
    pub async fn create_batch_mint_builder(
        &self,
        tree_account: &Pubkey,
    ) -> std::result::Result<BatchMintBuilder, BatchMintError> {
        let (max_depth, max_buffer_size, canopy_depth) = read_prepared_tree_size(&self.client, &tree_account).await?;
        BatchMintBuilder::new(tree_account.clone(), max_depth, max_buffer_size, canopy_depth)
    }

    /// Turns a BatchMint object into a batch mint builder, so it can be filled with additional assets.
    /// This can be useful if you have made your previuos builder into batch mint, saved it into JSON,
    /// but then decided to add more assets.
    pub async fn restore_batch_mint_builder(
        &self,
        batch_mint: &BatchMint,
    ) -> std::result::Result<BatchMintBuilder, BatchMintError> {
        let (max_depth, max_buffer_size, canopy_depth) =
            read_prepared_tree_size(&self.client, &batch_mint.tree_id).await?;
        let mut batch_mint_builder =
            BatchMintBuilder::new(batch_mint.tree_id, max_depth, max_buffer_size, canopy_depth)?;

        for batch_mint in &batch_mint.batch_mints {
            let BatchMintInstruction {
                tree_update: _,
                leaf_update,
                mint_args,
                authority: _,
                creator_signature,
            } = batch_mint;
            let LeafSchema::V1 {
                id: _,
                owner,
                delegate,
                nonce: _,
                data_hash: _,
                creator_hash: _,
            } = leaf_update;

            let metadata_arg_hash = batch_mint_builder.add_asset(owner, delegate, &mint_args)?;

            if let Some(creator_signature) = creator_signature {
                let mut message_and_signature = HashMap::new();
                message_and_signature.insert(metadata_arg_hash.get_nonce(), creator_signature.clone());

                batch_mint_builder.add_signatures_for_verified_creators(message_and_signature)?;
            }
        }

        Ok(batch_mint_builder)
    }

    /// Writes given batch mint to the solana tree account.
    ///
    /// ## Arguments
    /// * `payer` - account that pays for the operation
    /// * `metadata_url` - URL of the batch mint JSON representation stored in an immutable storage
    /// * `metadata_hash` - hash of metadata uploaded to an immutable storage
    /// * `batch_mint_builder` - batch mint builder object created after prepare_tree
    /// * `tree_creator` - same tree creator that was used to prepare_tree
    /// * `staker` - can be same as payer
    pub async fn finalize_tree(
        &self,
        payer: &Keypair,
        metadata_url: &str,
        metadata_hash: &str,
        batch_mint_builder: &BatchMintBuilder,
        tree_creator: &Keypair,
        staker: &Keypair,
    ) -> Result<Signature, BatchMintError> {
        let tree_config_account = pubkey_util::derive_tree_config_account(&batch_mint_builder.tree_account);

        let tree_data_account = &self.client.get_account(&batch_mint_builder.tree_account).await?;
        let tree_data_info = TreeDataInfo::from_bytes(tree_data_account.data())?;

        if tree_data_info.canopy_depth > 0 {
            let (canopy_to_add, canopy_offset) = calc_canopy_to_add(&tree_data_info, &batch_mint_builder)?;

            let compute_budget = ComputeBudgetInstruction::set_compute_unit_limit(1000000);
            for (ind, chunk) in canopy_to_add.chunks(CANOPY_NODES_PER_TX).enumerate() {
                let add_canopy_inst = AddCanopyBuilder::new()
                    .tree_config(tree_config_account)
                    .merkle_tree(batch_mint_builder.tree_account)
                    .incoming_tree_delegate(tree_creator.pubkey()) // Correct?
                    .canopy_nodes(chunk.to_vec())
                    .start_index((canopy_offset + ind * CANOPY_NODES_PER_TX) as u32)
                    .log_wrapper(spl_noop::id())
                    .compression_program(spl_account_compression::id())
                    .system_program(system_program::id())
                    .instruction();

                let tx = Transaction::new_signed_with_payer(
                    &[compute_budget.clone(), add_canopy_inst],
                    Some(&tree_creator.pubkey()),
                    &[tree_creator],
                    self.client.get_latest_blockhash().await?,
                );

                self.client.send_and_confirm_transaction(&tx).await?;
            }
        }

        // We're just using remaining_accounts to send proofs because they are of the same type
        let remaining_accounts = batch_mint_builder
            .merkle
            .get_rightmost_proof()
            .iter()
            .map(|proof| AccountMeta {
                pubkey: Pubkey::new_from_array(proof.clone()),
                is_signer: false,
                is_writable: false,
            })
            .collect::<Vec<_>>();
        let finalize_instruction = self.finalize_tree_instruction(
            payer,
            batch_mint_builder,
            metadata_url,
            metadata_hash,
            &remaining_accounts,
            tree_config_account,
            staker.pubkey(),
            tree_creator.pubkey(),
        )?;
        let mut signing_keypairs = [payer, tree_creator, staker].to_vec();
        if let Some(ref collection_config) = batch_mint_builder.collection_config {
            signing_keypairs.push(&collection_config.collection_authority);
        }

        let compute_budget = ComputeBudgetInstruction::set_compute_unit_limit(1000000);

        let tx = Transaction::new_signed_with_payer(
            &[compute_budget, finalize_instruction],
            Some(&tree_creator.pubkey()),
            signing_keypairs.as_slice(),
            self.client.get_latest_blockhash().await?,
        );

        let signature = self.client.send_and_confirm_transaction(&tx).await?;

        Ok(signature)
    }

    fn finalize_tree_instruction(
        &self,
        payer: &Keypair,
        batch_mint_builder: &BatchMintBuilder,
        metadata_url: &str,
        metadata_hash: &str,
        remaining_accounts: &Vec<AccountMeta>,
        tree_config_account: Pubkey,
        staker: Pubkey,
        tree_creator: Pubkey,
    ) -> std::result::Result<Instruction, BatchMintError> {
        let batch_mint = batch_mint_builder.build_batch_mint()?;
        if let Some(ref collection_config) = batch_mint_builder.collection_config {
            return Ok(FinalizeTreeWithRootAndCollectionBuilder::new()
                .merkle_tree(batch_mint.tree_id)
                .tree_config(tree_config_account)
                .staker(staker)
                .fee_receiver(bubblegum::state::FEE_RECEIVER)
                .tree_creator_or_delegate(tree_creator) // Correct?
                .registrar(pubkey_util::get_registrar_key())
                .voter(pubkey_util::get_voter_key(
                    &pubkey_util::get_registrar_key(),
                    &payer.pubkey(),
                ))
                .root(batch_mint.merkle_root)
                .rightmost_leaf(batch_mint.last_leaf_hash)
                .rightmost_index((batch_mint.batch_mints.len() as u32).saturating_sub(1))
                .metadata_url(metadata_url.to_string())
                .metadata_hash(metadata_hash.to_string())
                .add_remaining_accounts(remaining_accounts)
                .log_wrapper(spl_noop::id())
                .compression_program(spl_account_compression::id())
                .system_program(system_program::id())
                .collection_authority(collection_config.collection_authority.pubkey())
                .collection_mint(collection_config.collection_mint)
                .collection_edition(collection_config.edition_account)
                .collection_metadata(collection_config.collection_metadata)
                .collection_authority_record_pda(collection_config.collection_authority_record_pda)
                .instruction());
        }
        Ok(FinalizeTreeWithRootBuilder::new()
            .merkle_tree(batch_mint.tree_id)
            .tree_config(tree_config_account)
            .staker(staker)
            .fee_receiver(bubblegum::state::FEE_RECEIVER)
            .tree_creator_or_delegate(tree_creator) // Correct?
            .registrar(pubkey_util::get_registrar_key())
            .voter(pubkey_util::get_voter_key(
                &pubkey_util::get_registrar_key(),
                &payer.pubkey(),
            ))
            .root(batch_mint.merkle_root)
            .rightmost_leaf(batch_mint.last_leaf_hash)
            .rightmost_index((batch_mint.batch_mints.len() as u32).saturating_sub(1))
            .metadata_url(metadata_url.to_string())
            .metadata_hash(metadata_hash.to_string())
            .add_remaining_accounts(remaining_accounts)
            .log_wrapper(spl_noop::id())
            .compression_program(spl_account_compression::id())
            .system_program(system_program::id())
            .instruction())
    }
}

/// Fetches max depth, max buffer size and canopy_depth for a tree identified by given account.
async fn read_prepared_tree_size(
    client: &RpcClient,
    tree_accout: &Pubkey,
) -> std::result::Result<(u32, u32, u32), BatchMintError> {
    let account = client.get_account(tree_accout).await?;
    parse_tree_size(&account)
}

fn parse_tree_size(tree_account: &Account) -> std::result::Result<(u32, u32, u32), BatchMintError> {
    let merkle_tree = MerkleTree::from_bytes(tree_account.data())?;
    let (max_depth, max_buffer_size) = match merkle_tree.tree_header {
        ConcurrentMerkleTreeHeaderData::V1 {
            max_buffer_size,
            max_depth,
            authority: _,
            creation_slot: _,
            padding: _,
        } => (max_depth, max_buffer_size),
    };
    let merkel_tree_size = calc_merkle_tree_size(max_depth, max_buffer_size, 0)
        .ok_or(BatchMintError::UnexpectedTreeSize(max_depth, max_buffer_size))?;
    let canopy_buf_size = merkle_tree.serialized_tree.len() - merkel_tree_size;
    let canopy_depth = restore_canopy_depth_from_buffer(canopy_buf_size as u32);
    Ok((max_depth, max_buffer_size, canopy_depth))
}

/// Because canopy nodes are added by separate transactions, we may fall into situation when a portion of nodes
/// were added and then the application crushed, and we were not able to add the rest of canopy.
/// That's why on the re-run, we must detect those previously created nodes, and add only nodes tha are missing.
/// ## Args
/// * `tree_data_info` - tree data account fetched from Solana
/// * `batch_mint_builder` - the batch mint builder object we are making batch mint from
fn calc_canopy_to_add<'a>(
    tree_data_info: &'a TreeDataInfo,
    batch_mint_builder: &'a BatchMintBuilder,
) -> std::result::Result<(&'a [Node], usize), BatchMintError> {
    let canopy_leaves: &Vec<Node> = &batch_mint_builder.canopy_leaves;

    let existing_canopy = tree_data_info.non_empty_canopy_leaves()?;
    let (canopy_to_skip, canopy_to_add) = canopy_leaves.split_at(existing_canopy.len());
    for (to_add, existing) in existing_canopy.into_iter().zip(canopy_to_skip) {
        if to_add != existing {
            return Ok((canopy_leaves, 0));
        }
    }
    let canopy_offset = canopy_to_skip.len();

    Ok((canopy_to_add, canopy_offset))
}
