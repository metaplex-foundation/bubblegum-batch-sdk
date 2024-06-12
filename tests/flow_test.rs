use std::{ sync::Arc, time::Duration};

use metagrid_sdk::rollup_client::RollupClient;
use mpl_bubblegum::types::MetadataArgs;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{signature::Keypair, signer::Signer};



#[tokio::test]
async fn test_complete_flow() {

    // TODO: create registrar account
    // TODO: create voter account

    let owner: Keypair = id_json_key();

    let tree_data_account = Keypair::new();

    let url = "http://127.0.0.1:8899".to_string(); // Solana RPC node address
    let timeout = Duration::from_secs(1);
    let solana_client = Arc::new(RpcClient::new_with_timeout(url, timeout));


    let rollup_client = RollupClient::new(solana_client.clone());

    let sig_1 = rollup_client.prepare_tree(&owner, &tree_data_account, 20, 64, 3).await.unwrap();
    println!("Prepare tree signature: {sig_1:?}");

    let mut rollup_builder = rollup_client.create_rollup_builder(&tree_data_account.pubkey()).await.unwrap();
    println!("Rollup builder size: {}, {}, {}", rollup_builder.max_depth, rollup_builder.max_buffer_size, rollup_builder.canopy_depth);

    rollup_builder.add_asset(&owner.pubkey(), &owner.pubkey(), &make_test_metadata(1u8));

    let sig_2 = rollup_client.finalize_tree("http://mymetadata.ololo/", "mymetadatahash", &rollup_builder, &owner, &owner).await.unwrap();
    println!("Finalize tree signature: {sig_2:?}");
}

/// Loads keypair from "~/.config/solana/id.json",
/// i.e. the one by default used by solana-test-validato
fn id_json_key() -> Keypair {
    let id_json_path = format!("{}/.config/solana/id.json", std::env::home_dir().unwrap().to_str().unwrap());

    let id_json = std::fs::read_to_string(id_json_path)
    .expect("Should have been able to read the file");

    let id_bytes_str: String = id_json.chars()
        .skip_while(|s| *s != '[')
        .skip(1)
        .take_while(|s| *s != ']')
        .collect();

    let bytes: Vec<u8> = id_bytes_str.split(",")
        .map(|s| s.trim())
        .map(|s| s.parse::<u8>().unwrap())
        .collect();

    Keypair::from_bytes(&bytes).unwrap()
}

fn make_test_metadata(index: u8) -> MetadataArgs {
    MetadataArgs {
        name: format!("{index}"),
        symbol: format!("symbol-{index}"),
        uri: format!("https://immutable-storage/asset/{index}"),
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