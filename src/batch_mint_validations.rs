use crate::batch_mint_builder::{make_changelog_path, verify_signature, MetadataArgsHash};
use crate::errors::BatchMintError;
use crate::merkle_tree_wrapper::make_concurrent_merkle_tree;
use crate::model::{BatchMint, BatchMintInstruction, ChangeLogEventV1, PathNode};
use anchor_lang::AnchorSerialize;
use bubblegum::utils::get_asset_id;
use mpl_bubblegum::types::{Collection, LeafSchema, MetadataArgs, TokenProgramVersion, TokenStandard};
use rand::{thread_rng, Rng};
use solana_program::keccak;
use solana_program::keccak::Hash;
use solana_program::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use spl_concurrent_merkle_tree::concurrent_merkle_tree::ConcurrentMerkleTree;
use std::collections::HashMap;
use std::ops::Deref;
use std::str::FromStr;

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum BatchMintValidationError {
    #[error("PDACheckFail: expected: {0}, got: {1}")]
    PDACheckFail(String, String),
    #[error("InvalidDataHash: expected: {0}, got: {1}")]
    InvalidDataHash(String, String),
    #[error("InvalidCreatorsHash: expected: {0}, got: {1}")]
    InvalidCreatorsHash(String, String),
    #[error("InvalidRoot: expected: {0}, got: {1}")]
    InvalidRoot(String, String),
    #[error("NoRelevantRolledMint: index {0}")]
    NoRelevantRolledMint(u64),
    #[error("WrongAssetPath: id {0}")]
    WrongAssetPath(String),
    #[error("StdIo {0}")]
    StdIo(String),
    #[error("WrongTreeIdForChangeLog: asset: {0}, expected: {1}, got: {2}")]
    WrongTreeIdForChangeLog(String, String, String),
    #[error("WrongChangeLogIndex: asset: {0}, expected: {0}, got: {1}")]
    WrongChangeLogIndex(String, u32, u32),
    #[error("SplCompression: {0}")]
    SplCompression(#[from] spl_account_compression::ConcurrentMerkleTreeError),
    #[error("Unexpected tree depth={0} and max size={1}")]
    UnexpectedTreeSize(u32, u32),
    #[error("BatchMintError: {0}")]
    BatchMint(String),
    #[error("Failed creator's signature verification: {0}")]
    FailedCreatorVerification(String),
    #[error("Missing creator's signature in batch mint: {0}")]
    MissingCreatorSignature(String),
    #[error("WrongCollectionVerified: {0}")]
    WrongCollectionVerified(String),
    #[error("VerifiedCollectionMismatch: expected :{0}, got :{1}")]
    VerifiedCollectionMismatch(String, String),
}

impl From<std::io::Error> for BatchMintValidationError {
    fn from(err: std::io::Error) -> Self {
        BatchMintValidationError::StdIo(err.to_string())
    }
}
impl From<BatchMintError> for BatchMintValidationError {
    fn from(err: BatchMintError) -> Self {
        BatchMintValidationError::BatchMint(err.to_string())
    }
}

fn validate_change_logs(
    max_depth: u32,
    max_buffer_size: u32,
    leaves: &[[u8; 32]],
    batch_mint: &BatchMint,
) -> Result<(), BatchMintValidationError> {
    let mut tree = make_concurrent_merkle_tree(max_depth, max_buffer_size)?;
    tree.initialize()?;
    for (i, leaf_hash) in leaves.iter().enumerate() {
        tree.append(*leaf_hash)?;
        let changelog = tree.change_logs(tree.active_index() as usize);
        let path = make_changelog_path(changelog.deref());
        match batch_mint.batch_mints.get(i) {
            Some(mint) => {
                if mint.tree_update.path != path.into_iter().map(Into::<PathNode>::into).collect::<Vec<_>>() {
                    return Err(BatchMintValidationError::WrongAssetPath(
                        mint.leaf_update.id().to_string(),
                    ));
                }
                if mint.tree_update.id != batch_mint.tree_id {
                    return Err(BatchMintValidationError::WrongTreeIdForChangeLog(
                        mint.leaf_update.id().to_string(),
                        batch_mint.tree_id.to_string(),
                        mint.tree_update.id.to_string(),
                    ));
                }
                if mint.tree_update.index != changelog.index() {
                    return Err(BatchMintValidationError::WrongChangeLogIndex(
                        mint.leaf_update.id().to_string(),
                        changelog.index(),
                        mint.tree_update.index,
                    ));
                }
            }
            None => return Err(BatchMintValidationError::NoRelevantRolledMint(i as u64)),
        }
    }
    if tree.get_root() != batch_mint.merkle_root {
        return Err(BatchMintValidationError::InvalidRoot(
            Hash::new(tree.get_root().as_slice()).to_string(),
            Hash::new(batch_mint.merkle_root.as_slice()).to_string(),
        ));
    }
    Ok(())
}

fn get_leaf_hash(asset: &BatchMintInstruction, tree_id: &Pubkey) -> Result<[u8; 32], BatchMintValidationError> {
    let asset_id = get_asset_id(tree_id, asset.leaf_update.nonce());
    if asset_id != asset.leaf_update.id() {
        return Err(BatchMintValidationError::PDACheckFail(
            asset_id.to_string(),
            asset.leaf_update.id().to_string(),
        ));
    }

    // @dev: seller_fee_basis points is encoded twice so that it can be passed to marketplace
    // instructions, without passing the entire, un-hashed MetadataArgs struct
    let metadata_args_hash = keccak::hashv(&[asset.mint_args.try_to_vec()?.as_slice()]);
    let data_hash = keccak::hashv(&[
        &metadata_args_hash.to_bytes(),
        &asset.mint_args.seller_fee_basis_points.to_le_bytes(),
    ]);
    if asset.leaf_update.data_hash() != data_hash.to_bytes() {
        return Err(BatchMintValidationError::InvalidDataHash(
            data_hash.to_string(),
            Hash::new(asset.leaf_update.data_hash().as_slice()).to_string(),
        ));
    }

    // Use the metadata auth to check whether we can allow `verified` to be set to true in the
    // creator Vec.
    let creator_data = asset
        .mint_args
        .creators
        .iter()
        .map(|c| [c.address.as_ref(), &[c.verified as u8], &[c.share]].concat())
        .collect::<Vec<_>>();

    // Calculate creator hash.
    let creator_hash = keccak::hashv(
        creator_data
            .iter()
            .map(|c| c.as_slice())
            .collect::<Vec<&[u8]>>()
            .as_ref(),
    );
    if asset.leaf_update.creator_hash() != creator_hash.to_bytes() {
        return Err(BatchMintValidationError::InvalidCreatorsHash(
            creator_hash.to_string(),
            Hash::new(asset.leaf_update.creator_hash().as_slice()).to_string(),
        ));
    }

    Ok(asset.leaf_update.hash())
}

fn verify_creators_signatures(
    tree_key: &Pubkey,
    batch_mint: &BatchMintInstruction,
    creator_signatures: HashMap<Pubkey, Signature>,
) -> Result<(), BatchMintValidationError> {
    let metadata_hash = MetadataArgsHash::new(&batch_mint.leaf_update, tree_key, &batch_mint.mint_args);

    for creator in &batch_mint.mint_args.creators {
        if creator.verified {
            if let Some(signature) = creator_signatures.get(&creator.address) {
                if !verify_signature(&creator.address, &metadata_hash.get_message(), signature) {
                    return Err(BatchMintValidationError::FailedCreatorVerification(
                        creator.address.to_string(),
                    ));
                }
            } else {
                return Err(BatchMintValidationError::MissingCreatorSignature(
                    creator.address.to_string(),
                ));
            }
        }
    }

    Ok(())
}

pub async fn validate_batch_mint(
    batch_mint: &BatchMint,
    collection_mint: Option<Pubkey>,
) -> Result<(), BatchMintValidationError> {
    let mut leaf_hashes = Vec::new();
    for asset in batch_mint.batch_mints.iter() {
        let leaf_hash = match get_leaf_hash(asset, &batch_mint.tree_id) {
            Ok(leaf_hash) => leaf_hash,
            Err(e) => {
                return Err(e);
            }
        };
        leaf_hashes.push(leaf_hash);

        if let Some(ref collection) = asset.mint_args.collection {
            match collection_mint {
                None => {
                    if collection.verified {
                        return Err(BatchMintValidationError::WrongCollectionVerified(
                            collection.key.to_string(),
                        ));
                    }
                }
                Some(collection_mint) => {
                    if collection.verified && collection_mint != collection.key {
                        return Err(BatchMintValidationError::VerifiedCollectionMismatch(
                            collection_mint.to_string(),
                            collection.key.to_string(),
                        ));
                    }
                }
            }
        }

        verify_creators_signatures(
            &batch_mint.tree_id,
            asset,
            asset.creator_signature.clone().unwrap_or_default(),
        )?;
    }

    validate_change_logs(
        batch_mint.max_depth,
        batch_mint.max_buffer_size,
        &leaf_hashes,
        batch_mint,
    )
}

pub fn generate_batch_mint(size: usize) -> BatchMint {
    let authority = Pubkey::from_str("3VvLDXqJbw3heyRwFxv8MmurPznmDVUJS9gPMX2BDqfM").unwrap();
    let tree = Pubkey::from_str("HxhCw9g3kZvrdg9zZvctmh6qpSDg1FfsBXfFvRkbCHB7").unwrap();
    let mut mints = Vec::new();
    let mut merkle = ConcurrentMerkleTree::<10, 32>::new();
    merkle.initialize().unwrap();

    let mut last_leaf_hash = [0u8; 32];
    for i in 0..size {
        let mint_args = MetadataArgs {
            name: thread_rng()
                .sample_iter(rand::distributions::Alphanumeric)
                .take(15)
                .map(char::from)
                .collect(),
            symbol: thread_rng()
                .sample_iter(rand::distributions::Alphanumeric)
                .take(5)
                .map(char::from)
                .collect(),
            uri: format!(
                "https://arweave.net/{}",
                thread_rng()
                    .sample_iter(rand::distributions::Alphanumeric)
                    .take(43)
                    .map(char::from)
                    .collect::<String>()
            ),
            seller_fee_basis_points: thread_rng().sample(rand::distributions::Uniform::new(0, 10000)),
            primary_sale_happened: thread_rng().gen_bool(0.5),
            is_mutable: thread_rng().gen_bool(0.5),
            edition_nonce: if thread_rng().gen_bool(0.5) {
                None
            } else {
                Some(thread_rng().sample(rand::distributions::Uniform::new(0, 255)))
            },
            token_standard: if thread_rng().gen_bool(0.5) {
                None
            } else {
                Some(TokenStandard::NonFungible)
            },
            collection: if thread_rng().gen_bool(0.5) {
                None
            } else {
                Some(Collection {
                    verified: false,
                    key: Pubkey::new_unique(),
                })
            },
            uses: None, // todo
            token_program_version: TokenProgramVersion::Original,
            creators: (0..thread_rng().sample(rand::distributions::Uniform::new(1, 5)))
                .map(|_| mpl_bubblegum::types::Creator {
                    address: Pubkey::new_unique(),
                    verified: false,
                    share: thread_rng().sample(rand::distributions::Uniform::new(0, 100)),
                })
                .collect(),
        };
        let nonce = i as u64;
        let id = mpl_bubblegum::utils::get_asset_id(&tree, nonce);
        let owner = authority;
        let delegate = authority;

        let metadata_args_hash = keccak::hashv(&[mint_args.try_to_vec().unwrap().as_slice()]);
        let data_hash = keccak::hashv(&[
            &metadata_args_hash.to_bytes(),
            &mint_args.seller_fee_basis_points.to_le_bytes(),
        ]);
        let creator_data = mint_args
            .creators
            .iter()
            .map(|c| [c.address.as_ref(), &[c.verified as u8], &[c.share]].concat())
            .collect::<Vec<_>>();
        let creator_hash = keccak::hashv(
            creator_data
                .iter()
                .map(|c| c.as_slice())
                .collect::<Vec<&[u8]>>()
                .as_ref(),
        );

        let hashed_leaf = keccak::hashv(&[
            &[1], //self.version().to_bytes()
            id.as_ref(),
            owner.as_ref(),
            delegate.as_ref(),
            nonce.to_le_bytes().as_ref(),
            data_hash.as_ref(),
            creator_hash.as_ref(),
        ])
        .to_bytes();
        merkle.append(hashed_leaf).unwrap();
        last_leaf_hash = hashed_leaf;
        let changelog = merkle.change_logs[merkle.active_index as usize];
        let path_len = changelog.path.len() as u32;
        let mut path: Vec<spl_account_compression::state::PathNode> = changelog
            .path
            .iter()
            .enumerate()
            .map(|(lvl, n)| {
                spl_account_compression::state::PathNode::new(
                    *n,
                    (1 << (path_len - lvl as u32)) + (changelog.index >> lvl),
                )
            })
            .collect();
        path.push(spl_account_compression::state::PathNode::new(changelog.root, 1));

        let rolled_mint = BatchMintInstruction {
            tree_update: ChangeLogEventV1 {
                id: tree,
                path: path.into_iter().map(Into::into).collect::<Vec<_>>(),
                seq: merkle.sequence_number,
                index: changelog.index,
            },
            leaf_update: LeafSchema::V1 {
                id,
                owner,
                delegate,
                nonce,
                data_hash: data_hash.to_bytes(),
                creator_hash: creator_hash.to_bytes(),
            },
            mint_args,
            authority,
            creator_signature: None,
        };
        mints.push(rolled_mint);
    }

    BatchMint {
        tree_id: tree,
        raw_metadata_map: HashMap::new(),
        max_depth: 10,
        batch_mints: mints,
        merkle_root: merkle.get_root(),
        last_leaf_hash,
        max_buffer_size: 32,
    }
}

#[cfg(test)]
pub mod tests {
    use crate::batch_mint_validations::{generate_batch_mint, validate_batch_mint, BatchMintValidationError};
    use crate::errors::BatchMintError;
    use crate::model::PathNode;
    use mpl_bubblegum::types::LeafSchema;
    use solana_program::pubkey::Pubkey;

    #[tokio::test]
    async fn batch_mint_validation_test() {
        let mut batch_mint = generate_batch_mint(1000);
        let processing_result = validate_batch_mint(&batch_mint, None).await;
        assert_eq!(processing_result, Ok(()));

        let old_root = batch_mint.merkle_root;
        let new_root = Pubkey::new_unique();
        batch_mint.merkle_root = new_root.to_bytes();
        let processing_result = validate_batch_mint(&batch_mint, None).await;

        assert_eq!(
            processing_result,
            Err(BatchMintValidationError::InvalidRoot(
                Pubkey::from(old_root).to_string(),
                new_root.to_string()
            ))
        );

        batch_mint.merkle_root = old_root;
        let leaf_idx = 111;
        let old_leaf_data_hash = batch_mint.batch_mints[leaf_idx].leaf_update.data_hash();
        let new_leaf_data_hash = Pubkey::new_unique();
        batch_mint.batch_mints[leaf_idx].leaf_update = LeafSchema::V1 {
            id: batch_mint.batch_mints[leaf_idx].leaf_update.id(),
            owner: batch_mint.batch_mints[leaf_idx].leaf_update.owner(),
            delegate: batch_mint.batch_mints[leaf_idx].leaf_update.delegate(),
            nonce: batch_mint.batch_mints[leaf_idx].leaf_update.nonce(),
            data_hash: new_leaf_data_hash.to_bytes(),
            creator_hash: batch_mint.batch_mints[leaf_idx].leaf_update.creator_hash(),
        };
        let processing_result = validate_batch_mint(&batch_mint, None).await;

        assert_eq!(
            processing_result,
            Err(BatchMintValidationError::InvalidDataHash(
                Pubkey::from(old_leaf_data_hash).to_string(),
                new_leaf_data_hash.to_string()
            ))
        );

        batch_mint.batch_mints[leaf_idx].leaf_update = LeafSchema::V1 {
            id: batch_mint.batch_mints[leaf_idx].leaf_update.id(),
            owner: batch_mint.batch_mints[leaf_idx].leaf_update.owner(),
            delegate: batch_mint.batch_mints[leaf_idx].leaf_update.delegate(),
            nonce: batch_mint.batch_mints[leaf_idx].leaf_update.nonce(),
            data_hash: old_leaf_data_hash,
            creator_hash: batch_mint.batch_mints[leaf_idx].leaf_update.creator_hash(),
        };
        let old_tree_depth = batch_mint.max_depth;
        let new_tree_depth = 100;
        batch_mint.max_depth = new_tree_depth;
        let processing_result = validate_batch_mint(&batch_mint, None).await;

        assert_eq!(
            processing_result,
            Err(BatchMintValidationError::BatchMint(
                BatchMintError::UnexpectedTreeSize(new_tree_depth, batch_mint.max_buffer_size).to_string()
            ))
        );

        batch_mint.max_depth = old_tree_depth;
        let new_asset_id = Pubkey::new_unique();
        let old_asset_id = batch_mint.batch_mints[leaf_idx].leaf_update.id();
        batch_mint.batch_mints[leaf_idx].leaf_update = LeafSchema::V1 {
            id: new_asset_id,
            owner: batch_mint.batch_mints[leaf_idx].leaf_update.owner(),
            delegate: batch_mint.batch_mints[leaf_idx].leaf_update.delegate(),
            nonce: batch_mint.batch_mints[leaf_idx].leaf_update.nonce(),
            data_hash: batch_mint.batch_mints[leaf_idx].leaf_update.data_hash(),
            creator_hash: batch_mint.batch_mints[leaf_idx].leaf_update.creator_hash(),
        };
        let processing_result = validate_batch_mint(&batch_mint, None).await;

        assert_eq!(
            processing_result,
            Err(BatchMintValidationError::PDACheckFail(
                old_asset_id.to_string(),
                new_asset_id.to_string()
            ))
        );

        batch_mint.batch_mints[leaf_idx].leaf_update = LeafSchema::V1 {
            id: old_asset_id,
            owner: batch_mint.batch_mints[leaf_idx].leaf_update.owner(),
            delegate: batch_mint.batch_mints[leaf_idx].leaf_update.delegate(),
            nonce: batch_mint.batch_mints[leaf_idx].leaf_update.nonce(),
            data_hash: batch_mint.batch_mints[leaf_idx].leaf_update.data_hash(),
            creator_hash: batch_mint.batch_mints[leaf_idx].leaf_update.creator_hash(),
        };
        let old_path = batch_mint.batch_mints[leaf_idx]
            .tree_update
            .path
            .iter()
            .map(|path| PathNode {
                node: path.node,
                index: path.index,
            })
            .collect::<Vec<_>>();
        let new_path = Vec::new();
        batch_mint.batch_mints[leaf_idx].tree_update.path = new_path;
        let processing_result = validate_batch_mint(&batch_mint, None).await;

        assert_eq!(
            processing_result,
            Err(BatchMintValidationError::WrongAssetPath(
                batch_mint.batch_mints[leaf_idx].leaf_update.id().to_string()
            ))
        );

        batch_mint.batch_mints[leaf_idx].tree_update.path = old_path;
        let old_tree_id = batch_mint.batch_mints[leaf_idx].tree_update.id;
        let new_tree_id = Pubkey::new_unique();
        batch_mint.batch_mints[leaf_idx].tree_update.id = new_tree_id;
        let processing_result = validate_batch_mint(&batch_mint, None).await;

        assert_eq!(
            processing_result,
            Err(BatchMintValidationError::WrongTreeIdForChangeLog(
                batch_mint.batch_mints[leaf_idx].leaf_update.id().to_string(),
                old_tree_id.to_string(),
                new_tree_id.to_string()
            ))
        );

        batch_mint.batch_mints[leaf_idx].tree_update.id = old_tree_id;
        let old_index = batch_mint.batch_mints[leaf_idx].tree_update.index;
        let new_index = 1;
        batch_mint.batch_mints[leaf_idx].tree_update.index = new_index;
        let processing_result = validate_batch_mint(&batch_mint, None).await;

        assert_eq!(
            processing_result,
            Err(BatchMintValidationError::WrongChangeLogIndex(
                batch_mint.batch_mints[leaf_idx].leaf_update.id().to_string(),
                old_index,
                new_index
            ))
        );
    }
}
