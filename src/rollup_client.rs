use std::borrow::Borrow;
use std::sync::Arc;

use async_trait::async_trait;
use mpl_bubblegum::accounts::MerkleTree;
use mpl_bubblegum::instructions::{AddCanopyBuilder, FinalizeTreeWithRootBuilder, PrepareTreeBuilder};
use mpl_bubblegum::types::{ConcurrentMerkleTreeHeaderData, LeafSchema};
use solana_sdk::account::{Account, ReadableAccount};
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::instruction::AccountMeta;
use solana_sdk::signature::Signature;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::transaction::Transaction;

use crate::errors::RollupError;
use crate::merkle_tree_wrapper::{
    calc_merkle_tree_size, calc_tree_data_account_size, restore_canopy_depth_from_buffer,
};
use crate::model::{RolledMintInstruction, Rollup};
use crate::pubkey_util;
use crate::rollup_builder::RollupBuilder;

use solana_sdk::pubkey::Pubkey;
use solana_sdk::{system_instruction, system_program};

use solana_client::nonblocking::rpc_client::RpcClient;

const CANOPY_NODES_PER_TX: usize = 24;

/// The main controll point for rollup creation flows.
pub struct RollupClient {
    client: Arc<dyn AbstractSolanaClient>,
}

impl RollupClient {
    /// Creates a new instance that allows to create rollups.
    pub fn new(client: Arc<RpcClient>) -> RollupClient {
        RollupClient { client }
    }

    /// Mostly for testing purposes
    pub fn new_from_abstract_solana_client(client: Arc<dyn AbstractSolanaClient>) -> RollupClient {
        RollupClient { client }
    }

    pub fn client(&self) -> &dyn AbstractSolanaClient {
        self.client.borrow()
    }

    /// Prepares solana accounts (space) for future merkle tree.
    /// This is the first step of the flow of creating a compressed NFT aka Rollup.
    /// See https://developers.metaplex.com/bubblegum/create-trees
    /// for detail about merle tree.
    ///
    /// # Arguments
    /// * `tree_creator` - owner of tree account that to be created
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
    ) -> std::result::Result<Signature, RollupError> {
        if canopy_depth >= max_depth {
            return Err(RollupError::IllegalArgumets(
                "Canopy depth should be less than tree maximum depth".to_string(),
            ));
        }

        let required_canopy = max_depth.saturating_sub(bubblegum::state::MAX_ACC_PROOFS_SIZE);
        if canopy_depth < required_canopy {
            return Err(RollupError::IllegalArgumets(format!(
                "Three of depth={max_depth} reqiores as least canopy={required_canopy}"
            )));
        }

        let merkle_tree_size = calc_tree_data_account_size(max_depth, max_buf_size, canopy_depth)
            .ok_or(RollupError::UnexpectedTreeSize(max_depth, max_buf_size))?;

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
                PrepareTreeBuilder::new() // mpl_bubblegum::instructions::CreateTreeConfigBuilder
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

    pub async fn make_acc(&self, tree_creator: &Keypair,) -> std::result::Result<Signature, RollupError> {
        let new_acc = Keypair::new();
        let tx = Transaction::new_signed_with_payer(
            &[
                system_instruction::create_account(
                    // acquire space for future merkle tree
                    &tree_creator.pubkey(),
                    &new_acc.pubkey(),
                    self.client
                        .get_minimum_balance_for_rent_exemption(100)
                        .await?,
                    1 as u64,
                    &spl_account_compression::id(),
                ),
            ],
            Some(&tree_creator.pubkey()),
            &[tree_creator, &new_acc],
            self.client.get_latest_blockhash().await?,
        );

        let tx_signature = self.client.send_and_confirm_transaction(&tx).await?;

        Ok(tx_signature)
    }

    /// Creates a rollup builder object - a convenient wrapper for adding assets to rollups.
    pub async fn create_rollup_builder(
        &self,
        tree_account: &Pubkey,
    ) -> std::result::Result<RollupBuilder, RollupError> {
        let (max_depth, max_buffer_size, canopy_depth) = read_prepared_tree_size(self.client.borrow(), &tree_account).await?;
        RollupBuilder::new(tree_account.clone(), max_depth, max_buffer_size, canopy_depth)
    }

    /// Turns a Rollup object into a rollup builder, so it can be filled with additional assets.
    /// This can be useful if you have made your previuos builder into rollup, saved it into JSON,
    /// but then decided to add more assets.
    pub async fn restore_rollup_builder(&self, rollup: &Rollup) -> std::result::Result<RollupBuilder, RollupError> {
        let (max_depth, max_buffer_size, canopy_depth) = read_prepared_tree_size(self.client.borrow(), &rollup.tree_id).await?;
        let mut rollup_builder = RollupBuilder::new(rollup.tree_id, max_depth, max_buffer_size, canopy_depth)?;

        for rolled_mint in &rollup.rolled_mints {
            let RolledMintInstruction {
                tree_update: _,
                leaf_update,
                mint_args,
                authority: _,
            } = rolled_mint;
            let LeafSchema::V1 {
                id: _,
                owner,
                delegate,
                nonce: _,
                data_hash: _,
                creator_hash: _,
            } = leaf_update;
            rollup_builder.add_asset(owner, delegate, mint_args);
        }

        Ok(rollup_builder)
    }

    /// Writes given rollup to the solana tree account.
    /// ## Arguments
    /// * `staker` - can be same as payer
    pub async fn finalize_tree(
        &self,
        payer: &Keypair,
        metadata_url: &str,
        metadata_hash: &str,
        rollup_builder: &RollupBuilder,
        tree_creator: &Keypair,
        staker: &Keypair,
    ) -> Result<Signature, RollupError> {

        let tree_config_account = pubkey_util::derive_tree_config_account(&rollup_builder.tree_account);

        let canopy_depth = parse_tree_size(&self.client.get_account(&rollup_builder.tree_account).await?)?.2;
        if canopy_depth > 0 {
            let compute_budget = ComputeBudgetInstruction::set_compute_unit_limit(1000000);
            let canopy_leaves = &rollup_builder.canopy_leaves;

            for (ind, chunk) in canopy_leaves.chunks(CANOPY_NODES_PER_TX).enumerate() {
                let add_canopy_inst = AddCanopyBuilder::new()
                    .tree_config(tree_config_account)
                    .merkle_tree(rollup_builder.tree_account)
                    .incoming_tree_delegate(tree_creator.pubkey()) // Correct?
                    .canopy_nodes(chunk.to_vec())
                    .start_index((ind * CANOPY_NODES_PER_TX) as u32)
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

        let rollup = rollup_builder.build_rollup();
        // We're just using emaining_accounts to send proofs because they are of the same type
        let remaining_accounts = rollup_builder.merkle.get_rightmost_proof().iter()
            .map(|proof| AccountMeta {
                pubkey: Pubkey::new_from_array(proof.clone()),
                is_signer: false,
                is_writable: false,
            })
            .collect::<Vec<_>>();
        let finalize_instruction = FinalizeTreeWithRootBuilder::new()
            .payer(payer.pubkey())
            .merkle_tree(rollup.tree_id)
            .tree_config(tree_config_account)
            .staker(staker.pubkey())
            .fee_receiver(bubblegum::state::FEE_RECEIVER)
            .incoming_tree_delegate(tree_creator.pubkey()) // Correct?
            .registrar(pubkey_util::get_registrar_key())
            .voter(pubkey_util::get_voter_key(&pubkey_util::get_registrar_key(), &payer.pubkey()))
            .rightmost_root(rollup.merkle_root)
            .rightmost_leaf(rollup.last_leaf_hash)
            .rightmost_index((rollup.rolled_mints.len() as u32).saturating_sub(1))
            .metadata_url(metadata_url.to_string())
            .metadata_hash(metadata_hash.to_string())
            .add_remaining_accounts(&remaining_accounts)
            .log_wrapper(spl_noop::id())
            .compression_program(spl_account_compression::id())
            .system_program(system_program::id())
            .instruction();

        let tx = Transaction::new_signed_with_payer(
            &[finalize_instruction],
            Some(&tree_creator.pubkey()),
            &[payer, tree_creator, staker],
            self.client.get_latest_blockhash().await?,
        );


        let signature = self.client.send_and_confirm_transaction(&tx).await?;

        Ok(signature)
    }
}

/// Fetches max depth, max buffer size and canopy_depth for a tree identified by given account.
async fn read_prepared_tree_size(
    client: &dyn AbstractSolanaClient,
    tree_accout: &Pubkey,
) -> std::result::Result<(u32, u32, u32), RollupError> {
    let account = client.get_account(tree_accout).await?;
    parse_tree_size(&account)
}

fn parse_tree_size(tree_account: &Account) -> std::result::Result<(u32, u32, u32), RollupError> {
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
        .ok_or(RollupError::UnexpectedTreeSize(max_depth, max_buffer_size))?;
    let canopy_buf_size = merkle_tree.serialized_tree.len() - merkel_tree_size;
    let canopy_size = restore_canopy_depth_from_buffer(canopy_buf_size as u32);
    Ok((max_depth, max_buffer_size, canopy_size))
}


#[async_trait]
pub trait AbstractSolanaClient {
    async fn get_account(&self, pubkey: &Pubkey) -> std::result::Result<Account, RollupError>;

    async fn get_balance(&self, pubkey: &Pubkey) -> std::result::Result<u64, RollupError>;

    async fn get_minimum_balance_for_rent_exemption(
        &self,
        data_len: usize,
    ) -> std::result::Result<u64, RollupError>;

    async fn get_latest_blockhash(&self) -> std::result::Result<solana_program::hash::Hash, RollupError>;

    async fn send_and_confirm_transaction(
        &self,
        transaction: &Transaction,
    ) -> std::result::Result<Signature, RollupError>;
}

#[async_trait]
impl AbstractSolanaClient for RpcClient {
    async fn get_account(&self, pubkey: &Pubkey) -> std::result::Result<Account, RollupError> {
        let result = self.get_account(pubkey).await?;
        Ok(result)
    }

    async fn get_balance(&self, pubkey: &Pubkey) -> std::result::Result<u64, RollupError> {
        let result = self.get_balance(pubkey).await?;
        Ok(result) 
    }

    async fn get_minimum_balance_for_rent_exemption(
        &self,
        data_len: usize,
    ) -> std::result::Result<u64, RollupError> {
        let result = self.get_minimum_balance_for_rent_exemption(data_len).await?;
        Ok(result)
    }

    async fn get_latest_blockhash(&self) -> std::result::Result<solana_program::hash::Hash, RollupError> {
        let result = self.get_latest_blockhash().await?;
        Ok(result)
    }

    async fn send_and_confirm_transaction(
        &self,
        transaction: &Transaction,
    ) -> std::result::Result<Signature, RollupError> {
        let result = self.send_and_confirm_transaction(transaction).await?;
        Ok(result)
    }
}