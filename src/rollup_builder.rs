use std::collections::HashMap;

use anchor_lang::prelude::*;

use mpl_bubblegum::types::{Creator, LeafSchema, MetadataArgs};
use solana_sdk::signature::Signature;

use crate::errors::RollupError;
use crate::merkle_tree_wrapper::{make_concurrent_merkle_tree, IChangeLog, ITree};

use crate::model::{ChangeLogEventV1, RolledMintInstruction, Rollup};

use solana_sdk::keccak;
use solana_sdk::pubkey::Pubkey;

/// Builder that allows to easily build an offline compressed NFT,
/// that can be efficiently (cheap) saved onchain afterward.
///
/// It helps to add assets to the wrapped merkle tree,
/// generate a rollup that can be uploaded to an immutable storage,
///
/// TODO: Add link to the rollup documentation.
pub struct RollupBuilder {
    /// Public key of solana account that contains merkle data
    pub tree_account: Pubkey,
    /// depth of merkle tree
    pub max_depth: u32,
    /// Size of changelogs buffer = the maximum amount of concurrent changes to merkel tree
    pub max_buffer_size: u32,
    /// level of merkle tree (not counting root) that contains canopy leaf nodes
    pub canopy_depth: u32,
    /// encapsulates [ConcurrentMerkleTree]
    pub merkle: Box<dyn ITree>,
    /// See [Rollup::rolled_mints]
    pub mints: Vec<RolledMintInstruction>,
    /// See [Rollup::last_leaf_hash]
    pub last_leaf_hash: [u8; 32],
    /// canopy leaf nodes
    pub(crate) canopy_leaves: Vec<[u8; 32]>,
}

impl RollupBuilder {
    /// Create a builder with an empty merkle tree of given depth and max buffer size inside.
    pub(crate) fn new(
        tree_account: Pubkey,
        max_depth: u32,
        max_buffer_size: u32,
        canopy_depth: u32,
    ) -> std::result::Result<RollupBuilder, RollupError> {
        let mut merkle = make_concurrent_merkle_tree(max_depth, max_buffer_size)?;
        merkle.initialize().unwrap();

        Ok(RollupBuilder {
            tree_account: tree_account,
            max_depth: max_depth,
            max_buffer_size: max_buffer_size,
            canopy_depth: canopy_depth,
            merkle: merkle,
            mints: Vec::new(),
            last_leaf_hash: [0; 32],
            //tree_buf: TreeBuf::<[u8; 32]>::new_with_default(max_depth + 1),
            canopy_leaves: Vec::new(),
        })
    }

    /// Add an asset to the merkle tree
    /// ## Arguments:
    /// - `owner` - asset owner
    /// - `delegate` - ???
    /// - `metadata_args` - asset details as [MetadataArgs]
    /// - `creators_signatures` - HashMap with creators Pubkeys and signatures
    pub fn add_asset(&mut self, owner: &Pubkey, delegate: &Pubkey, metadata_args: &MetadataArgs, creators_signatures: &Option<HashMap<Pubkey, Signature>>) -> std::result::Result<(), RollupError> {
        let metadata_args_hash = hash_metadata_args(
            self.mints.len() as u64,
            &self.tree_account,
            owner,
            delegate,
            metadata_args,
        );
        let MetadataArgsHash {
            id,
            nonce,
            data_hash,
            creator_hash,
            hashed_leaf,
        } = metadata_args_hash;

        verify_signatures(&metadata_args.creators, hashed_leaf.as_ref(), creators_signatures.clone())?;

        self.merkle.append(hashed_leaf).unwrap();

        self.last_leaf_hash = hashed_leaf;
        let changelog = self.merkle.change_logs(self.merkle.active_index() as usize);
        let path = make_changelog_path(changelog.as_ref());

        if self.canopy_depth > 0 {
            let path_slice = changelog.path_slice();
            let path_ind = path_slice.len() - (self.canopy_depth as usize);
            let canopy_ind = changelog.index() >> (self.max_depth - self.canopy_depth);

            if self.canopy_leaves.len() < (canopy_ind + 1) as usize {
                self.canopy_leaves.push(path_slice[path_ind]);
            } else {
                self.canopy_leaves[canopy_ind as usize] = path_slice[path_ind];
            }
        }

        let rolled_mint = RolledMintInstruction {
            tree_update: ChangeLogEventV1 {
                id: self.tree_account.clone(),
                path: path.into_iter().map(Into::into).collect::<Vec<_>>(),
                seq: self.merkle.sequence_number(),
                index: changelog.index(),
            },
            leaf_update: LeafSchema::V1 {
                id,
                owner: *owner,
                delegate: *delegate,
                nonce,
                data_hash: data_hash,
                creator_hash: creator_hash,
            },
            mint_args: metadata_args.clone(),
            authority: owner.clone(),
            creator_signature: creators_signatures.clone(),
        };
        self.mints.push(rolled_mint);

        Ok(())
    }

    pub fn build_rollup(&self) -> Rollup {
        Rollup {
            tree_id: self.tree_account,
            raw_metadata_map: HashMap::new(), // TODO: fill?
            max_depth: self.max_depth,
            rolled_mints: self.mints.clone(),
            merkle_root: self.merkle.get_root(),
            last_leaf_hash: self.last_leaf_hash,
            max_buffer_size: self.max_buffer_size,
        }
    }
}

/// Validates signatures for asset creators who marked as verified
/// 
/// ## Arguments
/// `asset_creators` - list of asset creators
/// `msg` - leaf hash. Basically it's hash of such asset values as id, owner, delegate, nonce, data_hash, creator_hash
/// `creators_signatures` - HashMap with asset creators pubkeys and signatures
fn verify_signatures(asset_creators: &Vec<Creator>, msg: &[u8], creators_signatures: Option<HashMap<Pubkey, Signature>>) -> std::result::Result<(), RollupError> {
    let signatures = creators_signatures.unwrap_or_default();

    for creator in asset_creators {
        if creator.verified {
            if let Some(signature) = signatures.get(&creator.address) {
                if !signature.verify(creator.address.to_bytes().as_ref(), msg) {
                    return Err(RollupError::InvalidCreatorsSignature(creator.address.to_string()));
                }
            } else {
                return Err(RollupError::MissingCreatorsSignature(creator.address.to_string()));
            }
        }
    }

    Ok(())
}

/// Return value for asset leaf hasher function (Helper type that helps to simplify code)
struct MetadataArgsHash {
    id: Pubkey,
    nonce: u64,
    data_hash: [u8; 32],
    creator_hash: [u8; 32],
    hashed_leaf: [u8; 32],
}

/// Hashes given merkle tree leaf asset.
///
/// ## Arguments
/// `nonce` - should be `rollup_builder.mints.len() as u64`
fn hash_metadata_args(
    nonce: u64,
    tree_account: &Pubkey,
    owner: &Pubkey,
    delegate: &Pubkey,
    metadata_args: &MetadataArgs,
) -> MetadataArgsHash {
    let id: Pubkey = mpl_bubblegum::utils::get_asset_id(&tree_account, nonce);

    let metadata_args_hash = keccak::hashv(&[metadata_args.try_to_vec().unwrap().as_slice()]);
    let data_hash = keccak::hashv(&[
        &metadata_args_hash.to_bytes(),
        &metadata_args.seller_fee_basis_points.to_le_bytes(),
    ]);
    let creator_data = metadata_args
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
        &[1], // FIXME: What to specify here? self.version().to_bytes()?
        id.as_ref(),
        owner.as_ref(),
        delegate.as_ref(),
        nonce.to_le_bytes().as_ref(),
        data_hash.as_ref(),
        creator_hash.as_ref(),
    ])
    .to_bytes();

    MetadataArgsHash {
        id,
        nonce,
        data_hash: data_hash.to_bytes(),
        creator_hash: creator_hash.to_bytes(),
        hashed_leaf,
    }
}

/// Takes the changelog entry and constructs the path from the leaf (the asset,
/// the changelog entry is created for) up to the root of the merkel tree.
fn make_changelog_path(changelog: &dyn IChangeLog) -> Vec<spl_account_compression::state::PathNode> {
    let path_len = changelog.path_len();
    let mut path: Vec<spl_account_compression::state::PathNode> = changelog
        .path_iter()
        .enumerate()
        .map(|(lvl, n)| {
            spl_account_compression::state::PathNode::new(
                *n,
                (1 << (path_len - lvl as u32)) + (changelog.index() >> lvl), // maybe parent
            )
        })
        .collect();
    path.push(spl_account_compression::state::PathNode::new(changelog.root(), 1));
    path
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::model::Rollup;
    use solana_sdk::pubkey::Pubkey;
    use std::io::BufWriter;

    #[test]
    fn test_create_empty_rollup() {
        // Creating rollup builder
        let builder = RollupBuilder::new(Pubkey::new_unique(), 10, 32, 0).unwrap();

        // converting into rollup without adding any assets
        let rollup = builder.build_rollup();

        // serializing into JSON, in real flow this JSON probably would be written to a file
        let mut buffer = BufWriter::new(Vec::new());
        rollup.write_as_json(&mut buffer).unwrap();

        // restoring rollup from the JSON
        let restored_rollup = Rollup::read_as_json(buffer.buffer()).unwrap();

        assert_eq!(rollup, restored_rollup);
    }

    #[test]
    fn test_canopy_depth_4_for_tree_depth_5() {
        let owner = Pubkey::new_unique();
        let delegate = Pubkey::new_unique();

        let mut rollup_builder = RollupBuilder::new(Pubkey::new_unique(), 5, 8, 4).unwrap();

        for i in 1u8..=32 {
            let ma = test_metadata_args(i);
            rollup_builder.add_asset(&owner, &delegate, &ma, &None).unwrap();
        }

        let canopy_4 = &rollup_builder.canopy_leaves;
        assert_eq!(canopy_4.len(), 16);

        let leaf_1_hash = hash_metadata_args(
            0,
            &rollup_builder.tree_account,
            &owner,
            &delegate,
            &test_metadata_args(1u8),
        )
        .hashed_leaf;
        let leaf_2_hash = hash_metadata_args(
            1,
            &rollup_builder.tree_account,
            &owner,
            &delegate,
            &test_metadata_args(2u8),
        )
        .hashed_leaf;
        assert_eq!(canopy_4[0], keccak::hashv(&[&leaf_1_hash, &leaf_2_hash]).to_bytes());

        let leaf_31_hash = hash_metadata_args(
            30,
            &rollup_builder.tree_account,
            &owner,
            &delegate,
            &test_metadata_args(31u8),
        )
        .hashed_leaf;
        let leaf_32_hash = hash_metadata_args(
            31,
            &rollup_builder.tree_account,
            &owner,
            &delegate,
            &test_metadata_args(32u8),
        )
        .hashed_leaf;
        assert_eq!(canopy_4[15], keccak::hashv(&[&leaf_31_hash, &leaf_32_hash]).to_bytes());
    }

    #[test]
    fn test_get_canopy_on_patially_filled_tree() {
        let owner = Pubkey::new_unique();
        let delegate = Pubkey::new_unique();

        let mut rollup_builder = RollupBuilder::new(Pubkey::new_unique(), 5, 8, 4).unwrap();

        for i in 1u8..=((1u8 << 5) / 2) {
            let ma = test_metadata_args(i);
            rollup_builder.add_asset(&owner, &delegate, &ma, &None).unwrap();
        }

        assert_eq!(rollup_builder.canopy_leaves.len(), 8);
    }

    fn test_metadata_args(i: u8) -> MetadataArgs {
        MetadataArgs {
            name: format!("{i}"),
            symbol: format!("symbol-{i}"),
            uri: format!("https://immutable-storage/asset/{i}"),
            seller_fee_basis_points: 0,
            primary_sale_happened: false,
            is_mutable: false,
            edition_nonce: None,
            token_standard: Some(mpl_bubblegum::types::TokenStandard::NonFungible),
            collection: None,
            uses: None,
            token_program_version: mpl_bubblegum::types::TokenProgramVersion::Original,
            creators: Vec::new(),
        }
    }
}
