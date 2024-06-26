use std::collections::{BTreeMap, HashMap, HashSet};

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
    pub mints: BTreeMap<u64, RolledMintInstruction>,
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
            mints: BTreeMap::new(),
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
    pub fn add_asset(&mut self, owner: &Pubkey, delegate: &Pubkey, metadata_args: &MetadataArgs) -> std::result::Result<MetadataArgsHash, RollupError> {
        for creator in &metadata_args.creators {
            if creator.verified {
                return Err(RollupError::UnverifiedCreator(creator.address.to_string()));
            }
        }

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
            creator_signature: None,
        };
        self.mints.insert(nonce, rolled_mint);

        Ok(metadata_args_hash)
    }

    /// Adds verified creators to the Rollup.
    /// It takes creator's signatures and verifies them.
    /// Only if signature is valid it sets `creator.verified` value to `true`
    /// 
    /// ## Arguments
    /// - `nonce_and_creator_signatures` - hashMap with creators signatures for assets. As a key in first hashMap
    /// asset nonce is using. Nested hashMap contains pairs of creator Pubkey and signature.
    pub fn add_verified_creators_for_asset(&mut self, nonce_and_creator_signatures: HashMap<u64, HashMap<Pubkey, Signature>>) -> std::result::Result<(), RollupError> {
        for (asset_nonce, creator_signature) in nonce_and_creator_signatures {
            if let Some(rolled_mint) = self.mints.get_mut(&asset_nonce) {
                Self::check_extra_creators(&rolled_mint.mint_args.creators, &creator_signature)?;

                let mut rolled_signatures = rolled_mint.creator_signature.clone().unwrap_or_default();

                let metadata_hash = MetadataArgsHash::new(
                    &rolled_mint.leaf_update,
                    &self.tree_account,
                    &rolled_mint.mint_args,
                );
                let signed_message = metadata_hash.get_message();

                for creator in rolled_mint.mint_args.creators.iter_mut() {
                    if let Some(signature) = creator_signature.get(&creator.address) {
                        if !verify_signature(&creator.address, &signed_message, signature) {
                            return Err(RollupError::InvalidCreatorsSignature(creator.address.to_string()));
                        }
                        creator.verified = true;

                        rolled_signatures.insert(creator.address, *signature);
                    }
                }

                rolled_mint.creator_signature = Some(rolled_signatures);
            } else {
                return Err(RollupError::MissingRolledMint(asset_nonce));
            }
        }

        Ok(())
    }

    fn check_extra_creators(asset_creators: &[Creator], creator_signatures: &HashMap<Pubkey, Signature>) -> std::result::Result<(), RollupError> {
        let asset_creator_keys: HashSet<_> = asset_creators.iter().map(|c| &c.address).collect();
        let creator_keys_from_signatures: HashSet<_> = creator_signatures.keys().collect();

        let extra_creators: HashSet<_> = creator_keys_from_signatures.difference(&asset_creator_keys).collect();

        if !extra_creators.is_empty() {
            return Err(RollupError::ExtraCreatorsReceived);
        }

        Ok(())
    }

    pub fn build_rollup(&self) -> Rollup {
        Rollup {
            tree_id: self.tree_account,
            raw_metadata_map: HashMap::new(), // TODO: fill?
            max_depth: self.max_depth,
            rolled_mints: self.mints.values().cloned().collect(), // TODO: maybe it's better to move out mints not clone all of it
            merkle_root: self.merkle.get_root(),
            last_leaf_hash: self.last_leaf_hash,
            max_buffer_size: self.max_buffer_size,
        }
    }
}

/// Verifies that received message was signed by pointed signer
pub fn verify_signature(signer: &Pubkey, msg: &[u8], signature: &Signature) -> bool {
    signature.verify(signer.to_bytes().as_ref(), msg)
}

/// Return value for asset leaf hasher function (Helper type that helps to simplify code)
pub struct MetadataArgsHash {
    id: Pubkey,
    nonce: u64,
    data_hash: [u8; 32],
    creator_hash: [u8; 32],
    hashed_leaf: [u8; 32],
}

impl MetadataArgsHash {
    /// Creates new MetadataArgsHahs object
    pub fn new(leaf_schema: &LeafSchema, tree: &Pubkey, metadata_args: &MetadataArgs) -> Self {
        match leaf_schema {
            LeafSchema::V1 { id: _, owner, delegate, nonce, data_hash: _, creator_hash: _ } => {
                hash_metadata_args(*nonce, tree, owner, delegate, metadata_args)
            }
        }
    }

    /// It builds a message which should be signed by creator
    /// to verify asset.
    /// Message consist of asset's nonce in Big Endian + asset's leaf hash
    pub fn get_message(&self) -> Vec<u8> {
        [self.nonce.to_be_bytes().to_vec(), self.hashed_leaf.to_vec()].concat()
    }

    /// It takes raw message which were built by `get_message()` method and
    /// takes from there asset's nonce.
    /// 
    /// ## Arguments
    /// `message` - should be a message returned by `get_message()` method
    pub fn get_nonce_from_message(message: Vec<u8>) -> u64 {
        let mut buf = [0u8; 8];
        let len = 8.min(message.len());
        buf[..len].copy_from_slice(&message[..len]);
        u64::from_be_bytes(buf)
    }

    /// Returns asset nonce
    pub fn get_nonce(&self) -> u64 {
        self.nonce
    }
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
    let id: Pubkey = mpl_bubblegum::utils::get_asset_id(tree_account, nonce);

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
    use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};
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
            let ma = test_metadata_args(i, vec![]);
            rollup_builder.add_asset(&owner, &delegate, &ma).unwrap();
        }

        let canopy_4 = &rollup_builder.canopy_leaves;
        assert_eq!(canopy_4.len(), 16);

        let leaf_1_hash = hash_metadata_args(
            0,
            &rollup_builder.tree_account,
            &owner,
            &delegate,
            &test_metadata_args(1u8, vec![]),
        )
        .hashed_leaf;
        let leaf_2_hash = hash_metadata_args(
            1,
            &rollup_builder.tree_account,
            &owner,
            &delegate,
            &test_metadata_args(2u8, vec![]),
        )
        .hashed_leaf;
        assert_eq!(canopy_4[0], keccak::hashv(&[&leaf_1_hash, &leaf_2_hash]).to_bytes());

        let leaf_31_hash = hash_metadata_args(
            30,
            &rollup_builder.tree_account,
            &owner,
            &delegate,
            &test_metadata_args(31u8, vec![]),
        )
        .hashed_leaf;
        let leaf_32_hash = hash_metadata_args(
            31,
            &rollup_builder.tree_account,
            &owner,
            &delegate,
            &test_metadata_args(32u8, vec![]),
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
            let ma = test_metadata_args(i, vec![]);
            rollup_builder.add_asset(&owner, &delegate, &ma).unwrap();
        }

        assert_eq!(rollup_builder.canopy_leaves.len(), 8);
    }


    #[test]
    fn test_verify_one_creator() {
        let tree_account = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let delegate = Pubkey::new_unique();

        let creator_key = Keypair::new();

        let mut asset_creators = vec![
            Creator{
                address: creator_key.pubkey(),
                verified: true,
                share: 100,
            },
        ];

        let metadata_args = test_metadata_args(1u8, asset_creators.clone());

        let mut rollup_builder = RollupBuilder::new(tree_account, 5, 8, 4).unwrap();

        // we cannot add assets with verified=true field
        match rollup_builder.add_asset(&owner, &delegate, &metadata_args) {
            Ok(_) => panic!("Action should fail"),
            Err(err) => {
                match err {
                    RollupError::UnverifiedCreator(key) => {
                        assert_eq!(key, creator_key.pubkey().to_string());
                    }
                    _ => panic!("Method returned wrong error"),
                }
            }
        }

        // set verified to false to add asset and get metadata hash args for message creation
        asset_creators.get_mut(0).unwrap().verified = false;
        let metadata_args = test_metadata_args(1u8, asset_creators.clone());
        let metadata_args_hash = rollup_builder.add_asset(&owner, &delegate, &metadata_args).unwrap();

        let signature = creator_key.sign_message(&metadata_args_hash.get_message());

        let mut creators_signatures = HashMap::new();
        creators_signatures.insert(creator_key.pubkey(), signature);

        let mut message_and_signatures = HashMap::new();
        message_and_signatures.insert(metadata_args_hash.get_nonce(), creators_signatures);

        rollup_builder.add_verified_creators_for_asset(message_and_signatures).unwrap();

        // we have only one asset added and its creator should be verified now
        let assets: Vec<RolledMintInstruction> = rollup_builder.mints.values().cloned().collect();
        let asset = assets.get(0).unwrap();
        let asset_creator = asset.mint_args.creators.get(0).unwrap();

        assert_eq!(asset_creator.address, creator_key.pubkey());
        assert_eq!(asset_creator.verified, true);

        let asset_creators = vec![
            Creator{
                address: creator_key.pubkey(),
                verified: false,
                share: 100,
            },
        ];

        let metadata_args = test_metadata_args(2u8, asset_creators);

        let metadata_args_hash = rollup_builder.add_asset(&owner, &delegate, &metadata_args).unwrap();

        // sign wrong message
        let signature = creator_key.sign_message([1;32].as_ref());

        let mut creators_signatures = HashMap::new();
        creators_signatures.insert(creator_key.pubkey(), signature);

        let mut message_and_signatures = HashMap::new();
        message_and_signatures.insert(metadata_args_hash.get_nonce(), creators_signatures);

        match rollup_builder.add_verified_creators_for_asset(message_and_signatures) {
            Ok(_) => panic!("Action should fail"),
            Err(err) => {
                match err {
                    RollupError::InvalidCreatorsSignature(key) => {
                        assert_eq!(key, creator_key.pubkey().to_string());
                    }
                    _ => panic!("Method returned wrong error"),
                }
            }
        }

        let malicious_creator = Keypair::new();

        // sign correct message but with wrong creator key
        let signature = malicious_creator.sign_message(&metadata_args_hash.get_message());

        let mut creators_signatures = HashMap::new();
        creators_signatures.insert(malicious_creator.pubkey(), signature);

        let mut message_and_signatures = HashMap::new();
        message_and_signatures.insert(metadata_args_hash.get_nonce(), creators_signatures);

        match rollup_builder.add_verified_creators_for_asset(message_and_signatures) {
            Ok(_) => panic!("Action should fail"),
            Err(err) => {
                match err {
                    RollupError::ExtraCreatorsReceived => {}
                    _ => panic!("Method returned wrong error"),
                }
            }
        }
    }

    #[test]
    fn test_verify_few_creators() {
        let tree_account = Pubkey::new_unique();
        let owner = Pubkey::new_unique();
        let delegate = Pubkey::new_unique();

        let creator_key_1 = Keypair::new();
        let creator_key_2 = Keypair::new();

        let asset_creators = vec![
            Creator{
                address: creator_key_1.pubkey(),
                verified: false,
                share: 50,
            },
            Creator{
                address: creator_key_2.pubkey(),
                verified: false,
                share: 50,
            },
        ];

        let mut rollup_builder = RollupBuilder::new(tree_account, 5, 8, 4).unwrap();

        let metadata_args = test_metadata_args(1u8, asset_creators.clone());
        
        let metadata_hash = rollup_builder.add_asset(&owner, &delegate, &metadata_args).unwrap();

        let mut creators_signatures = HashMap::new();

        let signature = creator_key_1.sign_message(&metadata_hash.get_message());
        creators_signatures.insert(creator_key_1.pubkey(), signature);

        let signature = creator_key_2.sign_message(&metadata_hash.get_message());
        creators_signatures.insert(creator_key_2.pubkey(), signature);

        let mut message_and_signatures = HashMap::new();
        message_and_signatures.insert(metadata_hash.get_nonce(), creators_signatures);

        rollup_builder.add_verified_creators_for_asset(message_and_signatures).unwrap();

        // both creators should be verified now
        let rolled_mint_instrs: Vec<RolledMintInstruction> = rollup_builder.mints.values().cloned().collect();
        for creator in &rolled_mint_instrs.get(0).unwrap().mint_args.creators {
            assert_eq!(creator.verified, true);
        }

        let metadata_args = test_metadata_args(2u8, asset_creators.clone());

        let metadata_hash = rollup_builder.add_asset(&owner, &delegate, &metadata_args).unwrap();

        let mut creators_signatures = HashMap::new();

        let signature = creator_key_1.sign_message(&metadata_hash.get_message());
        creators_signatures.insert(creator_key_1.pubkey(), signature);

        let mut message_and_signatures = HashMap::new();
        message_and_signatures.insert(metadata_hash.get_nonce(), creators_signatures);

        rollup_builder.add_verified_creators_for_asset(message_and_signatures).unwrap();

        let rolled_mint_instrs: Vec<RolledMintInstruction> = rollup_builder.mints.values().cloned().collect();
        for creator in &rolled_mint_instrs.get(1).unwrap().mint_args.creators {
            if creator.address == creator_key_1.pubkey() {
                assert_eq!(creator.verified, true);
            } else {
                assert_eq!(creator.verified, false);
            }
        }

        let malicious_creator = Keypair::new();

        let metadata_args = test_metadata_args(3u8, asset_creators.clone());

        let metadata_hash = rollup_builder.add_asset(&owner, &delegate, &metadata_args).unwrap();

        let mut creators_signatures = HashMap::new();

        let signature = creator_key_1.sign_message(&metadata_hash.get_message());
        creators_signatures.insert(creator_key_1.pubkey(), signature);

        let signature = malicious_creator.sign_message(&metadata_hash.get_message());
        creators_signatures.insert(malicious_creator.pubkey(), signature);

        let mut message_and_signatures = HashMap::new();
        message_and_signatures.insert(metadata_hash.get_nonce(), creators_signatures);

        match rollup_builder.add_verified_creators_for_asset(message_and_signatures) {
            Ok(_) => panic!("Action should fail"),
            Err(err) => {
                match err {
                    RollupError::ExtraCreatorsReceived => {}
                    _ => panic!("Method returned wrong error"),
                }
            }
        }
    }

    fn test_metadata_args(i: u8, creators: Vec<Creator>) -> MetadataArgs {
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
            creators,
        }
    }
}
