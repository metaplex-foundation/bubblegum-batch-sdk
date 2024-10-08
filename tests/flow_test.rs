mod utils;

use bubblegum_batch_sdk::batch_mint_client::BatchMintClient;
use bubblegum_batch_sdk::errors::BatchMintError;
use bubblegum_batch_sdk::merkle_tree_wrapper::{calc_canopy_size, calc_merkle_tree_size};
use bubblegum_batch_sdk::pubkey_util;
use bubblegum_batch_sdk::pubkey_util::{get_mining_key, REWARD_POOL_ADDRESS};
use mpl_bubblegum::types::MetadataArgs;
use mpl_common_constants::constants::{DAO_GOVERNING_MINT, DAO_PUBKEY};
use mplx_staking_states::state::{
    DepositEntry, Lockup, LockupKind, LockupPeriod, Registrar, Voter, VotingMintConfig, REGISTRAR_DISCRIMINATOR,
};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_program::instruction::{AccountMeta, InstructionError};
use solana_rpc_client_api::client_error::ErrorKind;
use solana_rpc_client_api::request::{RpcError, RpcResponseErrorData};
use solana_sdk::compute_budget::ComputeBudgetInstruction;
use solana_sdk::transaction::{Transaction, TransactionError};
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

pub const VOTER_DISCRIMINATOR: [u8; 8] = [241, 93, 35, 191, 254, 147, 17, 202];
const MINIMUM_WEIGHTED_STAKE: u64 = 30_000_000_000_000; // 30 weighted MPLX

#[tokio::test]
#[cfg(not(any(skip_integration_tests)))]
#[serial_test::serial]
async fn test_complete_batch_mint_flow() {
    // Prepare env
    let (_validator, solana_client, payer, tree_creator, tree_data_account) =
        prepare_bubblegum_test_env(8899, MINIMUM_WEIGHTED_STAKE / LockupPeriod::OneYear.multiplier()).await;

    // Starting testing
    let batch_mint_client = BatchMintClient::new(solana_client.clone());

    const DEPTH: usize = 10;
    const BUFFER: usize = 32;
    const CANOPY: u32 = 3;

    let _sig_1 = batch_mint_client
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

    let mut batch_mint_builder = batch_mint_client
        .create_batch_mint_builder(&tree_data_account.pubkey())
        .await
        .unwrap();
    println!(
        "BatchMint builder size: {}, {}, {}",
        batch_mint_builder.max_depth, batch_mint_builder.max_buffer_size, batch_mint_builder.canopy_depth
    );

    batch_mint_builder
        .add_asset(&payer.pubkey(), &payer.pubkey(), &make_test_metadata(1u8))
        .unwrap();

    let _sig_2 = batch_mint_client
        .finalize_tree(
            &payer,
            "http://mymetadata.ololo/",
            "mymetadatahash",
            &batch_mint_builder,
            &tree_creator,
            &payer,
        )
        .await
        .unwrap();

    // Verification:
    // After FinilizeTreeWithRoot is executed, the offline ConcurrentMerkleTree
    // which is encapsulated by the batch mint, should be reflected in solana tree data account.
    // And the canopy bytes (if canopy had been set), should be cleared,
    // because the canopy is only needed to reflect the merkle tree from batch mint.
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
            std::mem::transmute(Box::into_raw(batch_mint_builder.merkle));
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
async fn prepare_tree_without_enough_stake() {
    // Prepare env
    let (_validator, solana_client, payer, tree_creator, tree_data_account) =
        prepare_bubblegum_test_env(8899, (MINIMUM_WEIGHTED_STAKE / LockupPeriod::OneYear.multiplier()) - 1).await;

    // Starting testing
    let batch_mint_client = BatchMintClient::new(solana_client.clone());

    const DEPTH: usize = 10;
    const BUFFER: usize = 32;
    const CANOPY: u32 = 3;

    let _sig_1 = batch_mint_client
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
}

#[tokio::test]
#[cfg(not(any(skip_integration_tests)))]
#[serial_test::serial]
async fn finalize_tree_without_enough_stake_fails() {
    // Prepare env
    let (_validator, solana_client, payer, tree_creator, tree_data_account) =
        prepare_bubblegum_test_env(8899, (MINIMUM_WEIGHTED_STAKE / LockupPeriod::OneYear.multiplier()) - 1).await;

    // Starting testing
    let batch_mint_client = BatchMintClient::new(solana_client.clone());

    const DEPTH: usize = 10;
    const BUFFER: usize = 32;
    const CANOPY: u32 = 3;

    let _sig_1 = batch_mint_client
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

    let mut batch_mint_builder = batch_mint_client
        .create_batch_mint_builder(&tree_data_account.pubkey())
        .await
        .unwrap();
    println!(
        "BatchMint builder size: {}, {}, {}",
        batch_mint_builder.max_depth, batch_mint_builder.max_buffer_size, batch_mint_builder.canopy_depth
    );

    batch_mint_builder
        .add_asset(&payer.pubkey(), &payer.pubkey(), &make_test_metadata(1u8))
        .unwrap();

    let err = batch_mint_client
        .finalize_tree(
            &payer,
            "http://mymetadata.ololo/",
            "mymetadatahash",
            &batch_mint_builder,
            &tree_creator,
            &payer,
        )
        .await
        .err()
        .unwrap();

    match err {
        BatchMintError::SolanaClientErr(e) => match e.kind {
            ErrorKind::RpcError(rpc_error) => match rpc_error {
                RpcError::RpcResponseError {
                    code: _code,
                    message: _message,
                    data,
                } => match data {
                    RpcResponseErrorData::SendTransactionPreflightFailure(simulate_tx_err) => {
                        match simulate_tx_err.err.unwrap() {
                            TransactionError::InstructionError(1, InstructionError::Custom(custom_error_idx)) => {
                                assert_eq!(custom_error_idx, 6042)
                            }
                            e => panic!("Unexpected TransactionError error: {}", e),
                        }
                        let mut canopy_root_mismatch = false;
                        simulate_tx_err.logs.unwrap().iter().for_each(|log| {
                            if log.contains("NotEnoughStakeForOperation") {
                                canopy_root_mismatch = true
                            }
                        });
                        assert!(canopy_root_mismatch)
                    }
                    e => panic!("Unexpected RpcResponseErrorData error: {}", e),
                },
                e => panic!("Unexpected RPC error: {}", e),
            },
            e => panic!("Unexpected solana error: {}", e),
        },
        e => panic!("Unexpected BatchMintError error: {}", e),
    }
}

#[tokio::test]
#[cfg(not(any(skip_integration_tests)))]
#[serial_test::serial]
async fn test_half_filled_assets() {
    // Prepare env
    let (_validator, solana_client, payer, tree_creator, tree_data_account) =
        prepare_bubblegum_test_env(8909, MINIMUM_WEIGHTED_STAKE / LockupPeriod::OneYear.multiplier()).await;

    // Starting testing
    let batch_mint_client = BatchMintClient::new(solana_client.clone());

    const DEPTH: usize = 5;
    const BUFFER: usize = 8;
    const CANOPY: u32 = 3;

    let _sig_1 = batch_mint_client
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

    let mut batch_mint_builder = batch_mint_client
        .create_batch_mint_builder(&tree_data_account.pubkey())
        .await
        .unwrap();

    for i in 1u8..(((1 << DEPTH) / 2) + 2) {
        batch_mint_builder
            .add_asset(&payer.pubkey(), &payer.pubkey(), &make_test_metadata(i))
            .unwrap();
    }

    let _sig_2 = batch_mint_client
        .finalize_tree(
            &payer,
            "http://mymetadata.ololo/",
            "mymetadatahash",
            &batch_mint_builder,
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
            std::mem::transmute(Box::into_raw(batch_mint_builder.merkle));
        let original: *const ConcurrentMerkleTree<DEPTH, BUFFER> = std::mem::transmute(orig_tree_ptr);

        let acc_tree_ptr = account_raw_bytes.as_ptr().add(header_size);
        let created: *const ConcurrentMerkleTree<DEPTH, BUFFER> = std::mem::transmute(acc_tree_ptr);

        // Thought the batch mint contains multiple assets, from the perspective of bubblegum merkle tree,
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

// Canopy leaf nodes are added in portions of maximum 24 nodes.
// This means that if we have more than 24 canopy leaf nodes, theoretically
// we can fall into a situation when after adding of a first portion of nodes,
// the applocation goes down (e.g. electricity issue).
// In this case, the functionality should be able to detect canopy leaf nodes
// added in the previous session, and add only mission nodes.
#[tokio::test]
#[cfg(not(any(skip_integration_tests)))]
#[serial_test::serial]
async fn test_canopy_resume() {
    // Prepare env

    use bubblegum_batch_sdk::pubkey_util;
    use mpl_bubblegum::instructions::AddCanopyBuilder;
    use solana_sdk::{system_program, transaction::Transaction};
    let (_validator, solana_client, payer, tree_creator, tree_data_account) =
        prepare_bubblegum_test_env(8919, MINIMUM_WEIGHTED_STAKE / LockupPeriod::OneYear.multiplier()).await;

    // Starting testing
    let batch_mint_client = BatchMintClient::new(solana_client.clone());

    const DEPTH: usize = 5;
    const BUFFER: usize = 8;
    const CANOPY: u32 = 3;

    let _sig_1 = batch_mint_client
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

    let mut batch_mint_builder = batch_mint_client
        .create_batch_mint_builder(&tree_data_account.pubkey())
        .await
        .unwrap();

    for i in 1u8..(((1 << DEPTH) / 2) + 2) {
        batch_mint_builder
            .add_asset(&payer.pubkey(), &payer.pubkey(), &make_test_metadata(i))
            .unwrap();
    }

    {
        let tree_config_account = pubkey_util::derive_tree_config_account(&batch_mint_builder.tree_account);
        // simulating adding of canopy
        let add_canopy_inst = AddCanopyBuilder::new()
            .tree_config(tree_config_account)
            .merkle_tree(batch_mint_builder.tree_account)
            .tree_creator_or_delegate(tree_creator.pubkey()) // Correct?
            .canopy_nodes(
                batch_mint_builder
                    .canopy_leaves
                    .iter()
                    .take(1)
                    .map(|a| a.clone())
                    .collect::<Vec<_>>(),
            )
            .start_index(0)
            .log_wrapper(spl_noop::id())
            .compression_program(spl_account_compression::id())
            .system_program(system_program::id())
            .instruction();
        let tx = Transaction::new_signed_with_payer(
            &[add_canopy_inst],
            Some(&tree_creator.pubkey()),
            &[&tree_creator],
            solana_client.get_latest_blockhash().await.unwrap(),
        );
        solana_client.send_and_confirm_transaction(&tx).await.unwrap();
    }

    let _sig_2 = batch_mint_client
        .finalize_tree(
            &payer,
            "http://mymetadata.ololo/",
            "mymetadatahash",
            &batch_mint_builder,
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
            std::mem::transmute(Box::into_raw(batch_mint_builder.merkle));
        let original: *const ConcurrentMerkleTree<DEPTH, BUFFER> = std::mem::transmute(orig_tree_ptr);

        let acc_tree_ptr = account_raw_bytes.as_ptr().add(header_size);
        let created: *const ConcurrentMerkleTree<DEPTH, BUFFER> = std::mem::transmute(acc_tree_ptr);

        // Thought the batch mint contains multiple assets, from the perspective of bubblegum merkle tree,
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

#[tokio::test]
#[cfg(not(any(skip_integration_tests)))]
#[serial_test::serial]
async fn test_finalize_canopy_tree_without_canopy_setup() {
    // Prepare env
    let (_validator, solana_client, payer, tree_creator, tree_data_account) =
        prepare_bubblegum_test_env(8919, MINIMUM_WEIGHTED_STAKE / LockupPeriod::OneYear.multiplier()).await;

    // Starting testing
    let batch_mint_client = BatchMintClient::new(solana_client.clone());

    const DEPTH: usize = 5;
    const BUFFER: usize = 8;
    const CANOPY: u32 = 3;

    let _sig_1 = batch_mint_client
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

    let mut batch_mint_builder = batch_mint_client
        .create_batch_mint_builder(&tree_data_account.pubkey())
        .await
        .unwrap();

    for i in 1u8..(((1 << DEPTH) / 2) + 2) {
        batch_mint_builder
            .add_asset(&payer.pubkey(), &payer.pubkey(), &make_test_metadata(i))
            .unwrap();
    }

    // finalize_tree without canopy setup
    let err = {
        let tree_config_account = pubkey_util::derive_tree_config_account(&batch_mint_builder.tree_account);

        // We're just using remaining_accounts to send proofs because they are of the same type
        let remaining_accounts = batch_mint_builder
            .merkle
            .get_rightmost_proof()
            .iter()
            .map(|proof| AccountMeta {
                pubkey: Pubkey::new_from_array(*proof),
                is_signer: false,
                is_writable: false,
            })
            .collect::<Vec<_>>();
        let finalize_instruction = batch_mint_client
            .finalize_tree_instruction(
                &payer,
                &batch_mint_builder,
                "http://mymetadata.ololo/",
                "mymetadatahash",
                remaining_accounts.as_slice(),
                tree_config_account,
                payer.pubkey(),
                tree_creator.pubkey(),
            )
            .unwrap();
        let mut signing_keypairs = [&payer, &tree_creator, &payer].to_vec();
        if let Some(ref collection_config) = batch_mint_builder.collection_config {
            signing_keypairs.push(&collection_config.collection_authority);
        }

        let compute_budget = ComputeBudgetInstruction::set_compute_unit_limit(1000000);

        let tx = Transaction::new_signed_with_payer(
            &[compute_budget, finalize_instruction],
            Some(&tree_creator.pubkey()),
            signing_keypairs.as_slice(),
            solana_client.get_latest_blockhash().await.unwrap(),
        );

        solana_client.send_and_confirm_transaction(&tx).await.err().unwrap()
    };
    match err.kind {
        ErrorKind::RpcError(rpc_error) => match rpc_error {
            RpcError::RpcResponseError {
                code: _code,
                message: _message,
                data,
            } => match data {
                RpcResponseErrorData::SendTransactionPreflightFailure(simulate_tx_err) => {
                    match simulate_tx_err.err.unwrap() {
                        TransactionError::InstructionError(1, InstructionError::Custom(custom_error_idx)) => {
                            assert_eq!(custom_error_idx, 6012)
                        }
                        e => panic!("Unexpected TransactionError error: {}", e),
                    }
                    let mut canopy_root_mismatch = false;
                    simulate_tx_err.logs.unwrap().iter().for_each(|log| {
                        if log.contains("CanopyRootMismatch") {
                            canopy_root_mismatch = true
                        }
                    });
                    assert!(canopy_root_mismatch)
                }
                e => panic!("Unexpected RpcResponseErrorData error: {}", e),
            },
            e => panic!("Unexpected RPC error: {}", e),
        },
        e => panic!("Unexpected solana error: {}", e),
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

async fn prepare_bubblegum_test_env(
    port: u32,
    stake_amount: u64,
) -> (ChildProcess, Arc<RpcClient>, Keypair, Keypair, Keypair) {
    // Preparing account for test
    let test_accounts = prepare_test_accounts(stake_amount);

    // Launching solana-test-validator with registrar and voter test accounts
    let mut tvr = TestValidatorRunner::new(port);
    tvr.add_account(&test_accounts.registrar);
    tvr.add_account(&test_accounts.voter);
    tvr.add_account(&test_accounts.mining);
    tvr.add_program(&ContractToDeploy {
        addr: mpl_bubblegum::ID,
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

    let tvp_process = tvr.run().unwrap();

    let url = format!("http://127.0.0.1:{port}"); // Solana RPC node address
    let solana_client = Arc::new(RpcClient::new_with_timeout(url, Duration::from_secs(1)));

    // Waiting for server to start
    await_for(10, Duration::from_secs(1), || solana_client.get_health())
        .await
        .unwrap();

    {
        // Fund test accounts and wait for transaction to be commited.
        let airdrop_sig_1 = solana_client
            .request_airdrop(&test_accounts.payer.pubkey(), 20000000 * 10000)
            .await
            .unwrap();
        let airdrop_sig_2 = solana_client
            .request_airdrop(&test_accounts.tree_creator.pubkey(), 20000000 * 10000)
            .await
            .unwrap();
        while !(solana_client.confirm_transaction(&airdrop_sig_1).await.unwrap()
            && solana_client.confirm_transaction(&airdrop_sig_2).await.unwrap())
        {
            sleep(Duration::from_secs(1)).await;
        }
    }

    (
        ChildProcess(tvp_process),
        solana_client,
        test_accounts.payer,
        test_accounts.tree_creator,
        test_accounts.tree_data_account,
    )
}

struct TestAccounts {
    payer: Keypair,
    tree_creator: Keypair,
    tree_data_account: Keypair,
    registrar: AccountInit,
    voter: AccountInit,
    mining: AccountInit,
}

/// FinalizeTreeWithRoot instruction, which is the final step for creating a batch mint
/// requires registrar, voter and mining accounts that are not easy to create.
/// That's why for the testing purposes we manually create these accounts,
/// by pushing them directly to solana-test-validator.
///
/// The code of accounts initialization is taken from bubblegum program tests.
fn prepare_test_accounts(stake_amount: u64) -> TestAccounts {
    let tree_creator = Keypair::from_bytes(TREE_CREATOR.as_ref()).unwrap();

    let tree_key = Keypair::from_bytes(TREE_KEY.as_ref()).unwrap();

    let payer: Keypair = Keypair::from_bytes(TEST_PAYER).unwrap();

    let governance_program_id = Pubkey::from_str("CuyWCRdHT8pZLG793UR5R9z31AC49d47ZW9ggN6P7qZ4").unwrap();
    let realm_authority = Pubkey::from_str("Euec5oQGN3Y9kqVrz6PQRfTpYSn6jK3k1JonDiMTzAtA").unwrap();
    let voter_authority = payer.pubkey();

    let mplx_mint_key = Pubkey::new_unique();
    let grant_authority = Pubkey::new_unique();
    let mining_key = get_mining_key(&payer.pubkey());
    let reward_pool_key = REWARD_POOL_ADDRESS;

    let registrar_key = Pubkey::find_program_address(
        &[DAO_PUBKEY.as_ref(), b"registrar".as_ref(), DAO_GOVERNING_MINT.as_ref()],
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

    // init structs for Registrar and Voter and fill it in with data
    let voting_mint_config = VotingMintConfig {
        mint: mplx_mint_key,
        grant_authority,
    };

    let registrar = Registrar {
        governance_program_id,
        realm: Pubkey::new_from_array(DAO_PUBKEY),
        realm_governing_token_mint: Pubkey::new_from_array(DAO_GOVERNING_MINT),
        realm_authority,
        voting_mints: [voting_mint_config, voting_mint_config],
        padding: [0, 0, 0, 0, 0, 0, 0],
        bump: 0,
        reward_pool: reward_pool_key,
    };

    let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;

    let lockup = Lockup {
        start_ts: 0,
        end_ts: current_time + Duration::from_secs(1000).as_millis() as u64,
        cooldown_ends_at: 0,
        cooldown_requested: false,
        kind: LockupKind::Constant,
        period: LockupPeriod::OneYear,
        _reserved0: [0; 16],
        _reserved1: [0; 5],
    };

    let deposit_entry = DepositEntry {
        lockup: lockup.clone(),
        delegate: Pubkey::new_unique(),
        amount_deposited_native: 0,
        voting_mint_config_idx: 0,
        is_used: true,
        _reserved0: [0; 32],
        _reserved1: [0; 6],
        delegate_last_update_ts: 0,
    };

    let mut deposit_entries = [deposit_entry; 32];
    deposit_entries[0] = DepositEntry {
        lockup: lockup.clone(),
        delegate: Pubkey::new_unique(),
        amount_deposited_native: stake_amount,
        voting_mint_config_idx: 0,
        is_used: true,
        _reserved0: [0; 32],
        _reserved1: [0; 6],
        delegate_last_update_ts: 0,
    };

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
    let mut mining_acc_data = [0; mplx_rewards::state::WrappedMining::LEN];
    mining_acc_data[32..64].copy_from_slice(&voter_authority.to_bytes());

    let mut mining_account = AccountSharedData::new(10000000000000000, mining_acc_data.len(), &mplx_rewards::ID);
    mining_account.set_data_from_slice(mining_acc_data.as_ref());

    TestAccounts {
        payer,
        tree_creator,
        tree_data_account: tree_key,
        registrar: AccountInit {
            name: "registrar.json".to_string(),
            pubkey: registrar_key,
            data: registrar_acc_data,
            owner: mplx_staking_states::ID,
        },
        voter: AccountInit {
            name: "voter.json".to_string(),
            pubkey: voter_key,
            data: voter_acc_data,
            owner: mplx_staking_states::ID,
        },
        mining: AccountInit {
            name: "mining.json".to_string(),
            pubkey: mining_key,
            data: mining_acc_data.as_ref().to_vec(),
            owner: mplx_rewards::ID,
        },
    }
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
