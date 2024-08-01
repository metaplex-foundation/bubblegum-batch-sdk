# Rollup SDK

This library allows to easily create a rollup (a compressed NFT tree that is initialized off-chain).

The [merkle tree](https://developers.metaplex.com/bubblegum/concurrent-merkle-trees)
is fully compatible with [Metaplex Bubblegum](https://developers.metaplex.com/bubblegum/mint-cnfts).

## Motivation

In case if you are minting a lot of NFTs,
[Metaplex Bubblegum](https://developers.metaplex.com/bubblegum) allows to significantly reduce
the cost on storage.
Yet you still have to make a separate transaction for each minted NFT,
that's why the initial creation of a big package of NFTs (e.g. for a game you are launching)
can be not cheap.

The Rollup solves this problem by moving the creation of the whole initial set of NFTs to off-chain.

1) you create the whole rollup, which is an off-chain representation of
the [merkle tree](https://developers.metaplex.com/bubblegum/concurrent-merkle-trees)
populate it with all the assets you want to be included in your initial set
2) you persist the rollup into an immutable storage, to make it available for validators
3) you push the whole tree of NFTs to Solana in a single operation (can consist of a couple of transactions)

And as the result you have a whole tree of NFTs, with expense of one account and a couple of solana transaction.

## Usage

This section demonstrates the complete flow of rollup creation.

⚠️ To be able to create a rollup, you need to have a stake in MPLX tokens.
TODO: add link to staking page.

Example of batch minting:
```rust
use rollup_sdk::rollup_client::RollupClient;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signer::keypair::Keypair;
use mpl_bubblegum::types::MetadataArgs;
use std::sync::Arc;
use rollup_sdk::model::CollectionConfig;
use std::time::Duration;

let payer: Keypair = todo!("the one who pays for the rollup");
let staker: Keypair = todo!("can be same as payer");

let url = "https://api.devnet.solana.com".to_string(); // Solana RPC node address
let timeout = Duration::from_secs(1);
let solana_client = Arc::new(RpcClient::new_with_timeout(url, timeout));

let rollup_client = RollupClient::new(solana_client);

// Account for a merkle tree data we are going to create
let tree_data_account = Keypair::new();

// Creating Solana account for storing tree and initializing it.
// Will prepare merkle tree with depth 10 (not counting root),
// 32 cells changelog buffer (max 32 concurrent changes),
// and canopy tree with depth 4 (not counting root).
let sign = rollup_client.prepare_tree(
    &payer,
    &tree_creator,
    &tree_data_account,
    20, // tree depth
    256,// maximum concurrent changes
    4   // canopy tree depth
).awailt()?;

let rollup_builder = rollup_client.create_rollup_builder(&tree_data_account.pubkey())
    .await?;

// Adding NTF asset
let assets_to_add: &[(MetadataArgs, Pubkey, Pubkey)] = todo!("load/prepare");
for (asset, asset_owner, asset_delegate) in assets_to_add {
    rollup_builder.add_asset(&asset_owner, &asset_delegate, &asset);
}

// Creating rollup object to be persisted in Arweave/IPFS/etc.
let rollup = rollup_builder.build_rollup();

// Persisting rollup to Arweave, where it will
// be picked up from by a DAS operator node.
let mut rollup_json_bytes = Vec::<u8>::new();
rollup.write_as_json(&mut rollup_json_bytes)?;
let metadata_url: String = todo("save rollup to arweave");
let metadata_hash: String = todo("hash of persisted rollup");

// Finalize rollup in solana:
// "move" offchain merkle tree along with the canopy tree to the account.
let sign = rollup_client.finalize_tree(
    &payer,
    &metadata_url,
    &metadata_hash,
    &rollup_builder,
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

Also if you want some asset have collection verified, tou can add collection config
```rust
let mut rollup_builder = rollup_client.create_rollup_builder(&tree_data_account.pubkey())
    .await?;

// Setup collection config if you want to add assets with verified collection
let collection_authority = todo!("keypair for collection authority");
rollup_builder.setup_collection_config(CollectionConfig {
    collection_authority,
    collection_authority_record_pda: None,
    collection_mint: todo!("add collection pubkey"),
    collection_metadata: todo!("add collection metadata pubkey"),
    edition_account: todo!("add collection edition account pubkey"),
});
```
All other steps are the same as regular batch mint

## Splitting the rollup creation in time

You may want to not fill all the assets and create the merkle tree at once,
but prepare a part of rollup, and then later (after hours, days, etc.)
fill the rest of asserts, and push the tree to Solana.

For that you need to create a `RollupBuilder`, populate it with a portion on assets,
generate generate the `Rollup` object and save it as JSON somewhere
(immutable storage, object store, local file, etc.)

```rust
let tree_data_account = Keypair::new();
let rollup_client: RollupClient = ...;

rollup_client.prepare_tree(&payer, &tree_creator, &tree_data_account.pubkey(), 10, 32, 4)
    .awailt()?;

let rollup_builder = rollup_client.create_rollup_builder(&tree_data_account.pubkey())
    .await()?;

let assets_to_add: &[(MetadataArgs, Pubkey, Pubkey)] = ...;
for (asset, asset_owner, asset_delegate) in assets_to_add {
    rollup_builder.add_asset(&asset_owner, &asset_delegate, &asset);
}

{
    let mut file = std::fs::File::create("rollup.json")?;
    let rollup = rollup_builder.build_rollup();
    rollup.write_as_json(&mut file)?;
}
```

Later you can recover the `RollupBuilder` from this persisted `Rollup` JSON representation
and continue the flow.

```rust
let mut file = std::fs::File::create("rollup.json")?;
let rollup = Rollup::read_as_json(&file).unwrap();
let rollup_builder = rollup_client.restore_rollup_builder(&rollup).await?;
```


## Running tests

See [Integration tests](it.md)
