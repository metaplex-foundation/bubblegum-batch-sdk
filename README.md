# Rollup SDK

See [Integration tests](it.md)

---

This library allows to easily create a rollup (a compressed NFT tree that is initialized off-chain).

The [merkle tree](https://developers.metaplex.com/bubblegum/concurrent-merkle-trees)
is fully compatible with [Metaplex Bubblegum](https://developers.metaplex.com/bubblegum/mint-cnfts).

## Usage

Example of complete rollup creation flow:

```rust
let owner: Keypair = todo!("read/initialize tree owner key pair");
let staker: Keypair = todo!("read MPLX staker key pair");

let url = "http://127.0.0.1::8899".to_string(); // Solana RPC node address
let timeout = Duration::from_secs(1);
let solana_client = Arc::new(RpcClient::new_with_timeout(url, timeout));

let rollup_client = RollupClient::new(solana_client);

// Account for a merkle tree data we are going to create
let tree_data_account = Keypair::new();
// Account for additional config data required by bubblegum solana program
let tree_config_account = Keypair::new();

// Creating Solana account for storing tree and initializing it.
// Will prepare merkle tree with depth 10 (not counting root),
// 32 cells changelog buffer (max 32 concurrent changes),
// and canopy tree with depth 4 (not counting root).
rollup_client.prepare_tree(
    &owner,
    &tree_data_account.pubkey(),
    &tree_config_account.pubkey(),
    10, // tree depth
    32, // maximum concurrent changes
    4   // canopy tree depth
).awailt()?;

let rollup_builder = rollup_client.create_rollup_builder(&tree_data_account.pubkey())
    .await()?;

// Adding NTF asset
let assets_to_add: &[(MetadataArgs, Pubkey, Pubkey)] = todo!("load/prepare");
for (asset, asset_owner, asset_delegate) in assets_to_add {
    rollup_builder.add_asset(&asset_owner, &asset_delegate, &asset);
}

// Creating rollup object to be persisted in Arweave/IPFS/etc.
let rollup = rollup_builder.build_rollup();

// Persisting rollup to Arweave, where it will
// be picked up from by a DAS operator node.
let mut buffer = BufWriter::new(Vec::new());
rollup.write_as_json(&mut buffer)?;
let rollup_json_bytes = buffer.buffer();
let metadata_url: String = todo("save rollup to arweave");
let metadata_hash: String = todo("hash of persisted rollup");

// Finalize rollup in solana:
// "move" offchain merkle tree along with the canopy tree to the account.
rollup_client.finalize_tree(
    &metadata_url,
    &metadata_hash,
    &rollup_builder,
    &tree_config_account,
    &tree_creator,
    &staker
)?;
```

First we need to have an account with a stack in MPLX (TODO add stack details).
Tree creator can be same account as staker, or can be different.

Then we need two accounts for storing merkle tree data itself,
and additional configs required by [Metaplex Bubblegum](https://developers.metaplex.com/bubblegum).

After that we create a rollup builder object.
It is a convenient wrapper that helps to easily:
* add assets to the tree
* generate rollup object that should be persisted to an immutable storage
* finalize rollup on chain (by sending tree root with proofs)
* write canopy (top level part of the tree that is saved on chain)

Using rollup builder object we add an asset to the rollup we build.

When all the assets are added (only one in our example),
we generate a rollup metadata JSON docuument and save it to Arweave.
As the result of this saving we need to get a URL of the persisted metadata,
and the metadata hash.

At this point we are ready to finalize the rollup by calling `finalize_tree`.
This operation "pushes" the merkle tree that had been created off-chain into solana.
If the canopy has been specified, it is also pushed in scope `finalize_tree`.

When DAO operator nodes detect `finalize_tree` transaction,
they download rollup JSON from the immutable storage it had been saved to,
and become ready to validate changes to the tree.

Compressed NFTs (merkle trees) that are created using the rollup flow,
are fully compatible with all [Metaplex Bubblegum](https://developers.metaplex.com/bubblegum)
instructions.
