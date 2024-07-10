mod utils;

use bubblegum::state::{REALM, REALM_GOVERNING_MINT};
use mpl_bubblegum::types::MetadataArgs;
use mplx_staking_states::state::{
    DepositEntry, Lockup, LockupKind, LockupPeriod, Registrar, Voter, VotingMintConfig, REGISTRAR_DISCRIMINATOR,
    VOTER_DISCRIMINATOR,
};
use rollup_sdk::merkle_tree_wrapper::{calc_canopy_size, calc_merkle_tree_size};
use rollup_sdk::rollup_client::RollupClient;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{account::AccountSharedData, pubkey::Pubkey, signature::Keypair, signer::Signer};
use spl_account_compression::ConcurrentMerkleTree;
use std::{
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::sleep;
use utils::test_validator_runner::{AccountInit, ChildProcess, ContractToDeploy, TestValidatorRunner};

const TREE_CREATOR: [u8; 64] = [
    71, 169, 21, 15, 207, 98, 125, 163, 177, 187, 118, 170, 54, 221, 34, 196, 99, 60, 80, 127, 202, 61, 72, 174, 135,
    151, 214, 203, 102, 106, 206, 18, 237, 231, 72, 189, 103, 136, 149, 222, 87, 237, 87, 30, 54, 80, 103, 206, 213,
    64, 193, 64, 100, 222, 54, 143, 251, 178, 188, 50, 54, 56, 87, 36,
];

pub const TREE_KEY: [u8; 64] = [
    48, 111, 197, 10, 137, 43, 207, 116, 57, 156, 24, 173, 58, 78, 235, 43, 129, 29, 81, 185, 140, 40, 63, 174, 159,
    208, 160, 246, 232, 151, 60, 201, 67, 162, 242, 249, 66, 65, 247, 140, 222, 107, 100, 127, 252, 98, 10, 242, 239,
    118, 198, 161, 87, 129, 14, 235, 76, 50, 9, 153, 52, 233, 11, 108,
];

// Just a predefined payer to be consistent betwwen test runs
const TEST_PAYER: &[u8] = &[
    180, 198, 251, 142, 71, 181, 136, 26, 203, 166, 231, 34, 223, 177, 9, 146, 247, 218, 42, 147, 89, 80, 190, 93, 82,
    213, 244, 111, 208, 225, 229, 30, 245, 66, 48, 225, 173, 117, 132, 129, 214, 176, 176, 39, 241, 9, 144, 79, 223,
    161, 99, 89, 97, 163, 63, 51, 106, 80, 233, 168, 246, 140, 97, 17,
];

#[tokio::test]
#[cfg(not(any(skip_integration_tests)))]
#[serial_test::serial]
async fn test_complete_rollup_flow() {
    // Prepare env
    let (
        _validator,
        solana_client,
        payer,
        tree_creator,
        tree_data_account
    ) = prepare_bubblegum_test_env(8899).await;

    // Starting testing
    let rollup_client = RollupClient::new(solana_client.clone());

    const DEPTH: usize = 10;
    const BUFFER: usize = 32;
    const CANOPY: u32 = 3;

    let _sig_1 = rollup_client
        .prepare_tree(
            &payer,
            &tree_creator,
            &tree_data_account,
            DEPTH as u32,
            BUFFER as u32,
            CANOPY,
        )
        .await
        .unwrap();

    let mut rollup_builder = rollup_client
        .create_rollup_builder(&tree_data_account.pubkey())
        .await
        .unwrap();
    println!(
        "Rollup builder size: {}, {}, {}",
        rollup_builder.max_depth, rollup_builder.max_buffer_size, rollup_builder.canopy_depth
    );

    rollup_builder.add_asset(&payer.pubkey(), &payer.pubkey(), &make_test_metadata(1u8)).unwrap();

    let _sig_2 = rollup_client
        .finalize_tree(
            &payer,
            "http://mymetadata.ololo/",
            "mymetadatahash",
            &rollup_builder,
            &tree_creator,
            &payer,
        )
        .await
        .unwrap();

    // Verification:
    // After FinilizeTreeWithRoot is executed, the offline ConcurrentMerkleTree
    // which is encapsulated by the rollup, should be reflected in solana tree data account.
    // And the canopy bytes (if canopy had been set), should be cleared,
    // because the canopy is only needed to reflect the merkle tree from rollup.
    let account_raw_bytes = solana_client
        .get_account_data(&tree_data_account.pubkey())
        .await
        .unwrap();

    let header_size = spl_account_compression::state::CONCURRENT_MERKLE_TREE_HEADER_SIZE_V1;
    let tree_size = calc_merkle_tree_size(DEPTH as u32, BUFFER as u32, CANOPY).unwrap();
    let canopy_size = calc_canopy_size(CANOPY);

    // Comparing offchain merkle tree with the one created by finilize_tree
    unsafe {
        let (orig_tree_ptr, _vtable_ptr): (*const u8, *const u8) =
            std::mem::transmute(Box::into_raw(rollup_builder.merkle));
        let original: *const ConcurrentMerkleTree<DEPTH, BUFFER> = std::mem::transmute(orig_tree_ptr);

        let acc_tree_ptr = account_raw_bytes.as_ptr().add(header_size);
        let created: *const ConcurrentMerkleTree<DEPTH, BUFFER> = std::mem::transmute(acc_tree_ptr);

        assert_eq!((*original).sequence_number, (*created).sequence_number);
        assert_eq!((*original).rightmost_proof, (*created).rightmost_proof);
    }

    // Canopy is cleared after finilize_tree
    unsafe {
        let canopy_segment_offset = account_raw_bytes.as_ptr().add(header_size + tree_size);
        let canopy_ptr = canopy_segment_offset as *const [u8; 32];
        for canopy_ind in 0..canopy_size / 32 {
            assert_eq!(*canopy_ptr.add(canopy_ind), [0u8; 32]);
        }
    }
}

#[tokio::test]
#[cfg(not(any(skip_integration_tests)))]
#[serial_test::serial]
async fn test_half_filled_assets() {
    // Prepare env
    let (
        _validator,
        solana_client,
        payer,
        tree_creator,
        tree_data_account
    ) = prepare_bubblegum_test_env(8909).await;

    // Starting testing
    let rollup_client = RollupClient::new(solana_client.clone());

    const DEPTH: usize = 5;
    const BUFFER: usize = 8;
    const CANOPY: u32 = 3;

    let _sig_1 = rollup_client
        .prepare_tree(
            &payer,
            &tree_creator,
            &tree_data_account,
            DEPTH as u32,
            BUFFER as u32,
            CANOPY,
        )
        .await
        .unwrap();

    let mut rollup_builder = rollup_client
        .create_rollup_builder(&tree_data_account.pubkey())
        .await
        .unwrap();

    for i in 1u8 .. (((1<<DEPTH) / 2) + 2) {
        rollup_builder.add_asset(&payer.pubkey(), &payer.pubkey(), &make_test_metadata(i));
    }

    let _sig_2 = rollup_client
        .finalize_tree(
            &payer,
            "http://mymetadata.ololo/",
            "mymetadatahash",
            &rollup_builder,
            &tree_creator,
            &payer,
        )
        .await
        .unwrap();

    // Verification:
    let account_raw_bytes = solana_client
        .get_account_data(&tree_data_account.pubkey())
        .await
        .unwrap();

    let header_size = spl_account_compression::state::CONCURRENT_MERKLE_TREE_HEADER_SIZE_V1;
    let tree_size = calc_merkle_tree_size(DEPTH as u32, BUFFER as u32, CANOPY).unwrap();
    let canopy_size = calc_canopy_size(CANOPY);

    unsafe {
        let (orig_tree_ptr, _vtable_ptr): (*const u8, *const u8) =
            std::mem::transmute(Box::into_raw(rollup_builder.merkle));
        let original: *const ConcurrentMerkleTree<DEPTH, BUFFER> = std::mem::transmute(orig_tree_ptr);

        let acc_tree_ptr = account_raw_bytes.as_ptr().add(header_size);
        let created: *const ConcurrentMerkleTree<DEPTH, BUFFER> = std::mem::transmute(acc_tree_ptr);

        // Thought the rollup contains multiple assets, from the perspective of bubblegum merkle tree,
        // it is only one node added
        assert_eq!(1, (*created).sequence_number);
        assert_eq!((*original).rightmost_proof, (*created).rightmost_proof);
    }

    unsafe {
        let canopy_segment_offset = account_raw_bytes.as_ptr().add(header_size + tree_size);
        let canopy_ptr = canopy_segment_offset as *const [u8; 32];
        for canopy_ind in 0..canopy_size / 32 {
            assert_eq!(*canopy_ptr.add(canopy_ind), [0u8; 32]);
        }
    }
}

/// Helps to wait for an async functionality to startup.
async fn await_for<T, E, F, Fut>(attempts: u32, interval: Duration, f: F) -> std::result::Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, E>>,
{
    for _attempts in 1..attempts {
        let r = f().await;
        if r.is_ok() {
            return r;
        }
        sleep(interval).await;
    }
    f().await
}

async fn prepare_bubblegum_test_env(port: u32) -> (ChildProcess, Arc<RpcClient>, Keypair, Keypair, Keypair,) {
        // Preparing account for test
        let (payer, tree_creator, tree_data_account, registrar, voter) = prepare_test_accounts();

        // Launching solana-test-validator with registrar and voter test accounts
        let mut tvr = TestValidatorRunner::new(port);
        tvr.add_account(&registrar);
        tvr.add_account(&voter);
        tvr.add_program(&ContractToDeploy {
            addr: bubblegum::ID,
            path: "../mpl-bubblegum/programs/.bin/bubblegum.so".to_string(),
        });
        tvr.add_program(&ContractToDeploy {
            addr: spl_account_compression::ID,
            path: "../mpl-bubblegum/programs/.bin/spl_account_compression.so".to_string(),
        });
        tvr.add_program(&ContractToDeploy {
            addr: spl_noop::ID,
            path: "../mpl-bubblegum/programs/.bin/spl_noop.so".to_string(),
        });
    
        let mut tvp_process = tvr.run().unwrap();
    
        let url = format!("http://127.0.0.1:{port}"); // Solana RPC node address
        let solana_client = Arc::new(RpcClient::new_with_timeout(url, Duration::from_secs(1)));
    
        // Waiting for server to start
        await_for(10, Duration::from_secs(1), || solana_client.get_health())
            .await
            .unwrap();
    
        {
            // Fund test accounts and wait for transaction to be commited.
            let airdrop_sig_1 = solana_client
                .request_airdrop(&payer.pubkey(), 20000000 * 10000)
                .await
                .unwrap();
            let airdrop_sig_2 = solana_client
                .request_airdrop(&tree_creator.pubkey(), 20000000 * 10000)
                .await
                .unwrap();
            while !(solana_client.confirm_transaction(&airdrop_sig_1).await.unwrap()
                && solana_client.confirm_transaction(&airdrop_sig_2).await.unwrap())
            {
                sleep(Duration::from_secs(1)).await;
            }
        }

        (ChildProcess(tvp_process), solana_client, payer, tree_creator, tree_data_account)
}

/// FinalizeTreeWithRoot instruction, which is the final step for creating a rollup
/// requires registrar and voter accounts that are not easy to create.
/// That's why for the testing purposes we manually create these accounts,
/// by pushing them directly to solana-test-validator.
///
/// The code of accounts initialization is taken from bubblegum program tests.
fn prepare_test_accounts() -> (Keypair, Keypair, Keypair, AccountInit, AccountInit) {
    let tree_creator = Keypair::from_bytes(TREE_CREATOR.as_ref()).unwrap();

    let tree_key = Keypair::from_bytes(TREE_KEY.as_ref()).unwrap();

    let payer: Keypair = Keypair::from_bytes(TEST_PAYER).unwrap();

    let governance_program_id = Pubkey::from_str("CuyWCRdHT8pZLG793UR5R9z31AC49d47ZW9ggN6P7qZ4").unwrap();
    let realm_authority = Pubkey::from_str("Euec5oQGN3Y9kqVrz6PQRfTpYSn6jK3k1JonDiMTzAtA").unwrap();
    let voter_authority = payer.pubkey();

    let mplx_mint_key = Pubkey::new_unique();
    let grant_authority = Pubkey::new_unique();

    let registrar_key = Pubkey::find_program_address(
        &[
            REALM.to_bytes().as_ref(),
            b"registrar".as_ref(),
            REALM_GOVERNING_MINT.to_bytes().as_ref(),
        ],
        &mplx_staking_states::ID,
    )
    .0;

    let (voter_key, voter_bump) = Pubkey::find_program_address(
        &[
            registrar_key.to_bytes().as_ref(),
            b"voter".as_ref(),
            voter_authority.to_bytes().as_ref(),
        ],
        &mplx_staking_states::ID,
    );

    // // init structs for Registrar and Voter and fill it in with data
    let voting_mint_config = VotingMintConfig {
        mint: mplx_mint_key,
        grant_authority,
        baseline_vote_weight_scaled_factor: 0,
        max_extra_lockup_vote_weight_scaled_factor: 0,
        lockup_saturation_secs: 0,
        digit_shift: 0,
        padding: [0, 0, 0, 0, 0, 0, 0],
    };

    let registrar = Registrar {
        governance_program_id,
        realm: REALM,
        realm_governing_token_mint: REALM_GOVERNING_MINT,
        realm_authority,
        voting_mints: [
            voting_mint_config,
            voting_mint_config,
            voting_mint_config,
            voting_mint_config,
        ],
        time_offset: 0,
        bump: 0,
    };

    let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;

    let lockup = Lockup {
        start_ts: 0,
        end_ts: current_time + 100,
        cooldown_ends_at: 0,
        cooldown_requested: false,
        kind: LockupKind::Constant,
        period: LockupPeriod::ThreeMonths,
        _reserved1: [0; 5],
    };

    let deposit_entry = DepositEntry {
        lockup: lockup.clone(),
        amount_deposited_native: 100000000,
        voting_mint_config_idx: 0,
        is_used: true,
        _reserved1: [0; 6],
    };

    let deposit_entries = [deposit_entry; 32];

    let voter = Voter {
        deposits: deposit_entries,
        voter_authority,
        registrar: registrar_key,
        voter_bump,
        voter_weight_record_bump: 0,
        _reserved1: [0; 14],
    };

    let registrar_acc_data = [REGISTRAR_DISCRIMINATOR.as_ref(), bytemuck::bytes_of(&registrar)].concat();
    let voter_acc_data = [VOTER_DISCRIMINATOR.as_ref(), bytemuck::bytes_of(&voter)].concat();

    // for next two accounts set arbitrary balance because it doesn't meter for test
    let mut registrar_account =
        AccountSharedData::new(10000000000000000, registrar_acc_data.len(), &mplx_staking_states::ID);
    registrar_account.set_data_from_slice(registrar_acc_data.as_ref());

    let mut voter_account = AccountSharedData::new(10000000000000000, voter_acc_data.len(), &mplx_staking_states::ID);
    voter_account.set_data_from_slice(voter_acc_data.as_ref());

    (
        payer,
        tree_creator,
        tree_key,
        AccountInit {
            name: "registrar.json".to_string(),
            pubkey: registrar_key,
            data: registrar_acc_data,
            owner: mplx_staking_states::ID,
        },
        AccountInit {
            name: "voter.json".to_string(),
            pubkey: voter_key,
            data: voter_acc_data,
            owner: mplx_staking_states::ID,
        },
    )
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
