mod utils;

use std::{str::FromStr, sync::Arc, time::{Duration, SystemTime, UNIX_EPOCH}};

use async_trait::async_trait;
use bubblegum::state::{REALM, REALM_GOVERNING_MINT};
use mplx_staking_states::state::{DepositEntry, Lockup, LockupKind, LockupPeriod, Registrar, Voter, VotingMintConfig, REGISTRAR_DISCRIMINATOR, VOTER_DISCRIMINATOR};
use rollup_sdk::{errors::RollupError, rollup_client::{AbstractSolanaClient, RollupClient}};
use mpl_bubblegum::types::MetadataArgs;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{account::AccountSharedData, pubkey::Pubkey, signature::{Keypair, Signature}, signer::Signer, transaction::Transaction};
use tokio::sync::Mutex;
use utils::{context::BubblegumTestContext, DirtyClone};


struct SyscallStubs {}
impl solana_sdk::program_stubs::SyscallStubs for SyscallStubs {
    fn sol_log(&self, message: &str) {
        println!("{message}");
    }
}

fn local_solana_clinet() -> RpcClient {
    let url = "http://127.0.0.1:8899".to_string(); // Solana RPC node address
    let timeout = Duration::from_secs(1);
    RpcClient::new_with_timeout(url, timeout)
}

//#[tokio::test]
async fn test_complete_flow() {
    // TODO: create registrar account
    // TODO: create voter account

    let owner: Keypair = id_json_key();

    let tree_data_account = Keypair::new();

    let url = "http://127.0.0.1:8899".to_string(); // Solana RPC node address
    let timeout = Duration::from_secs(1);
    let solana_client = Arc::new(RpcClient::new_with_timeout(url, timeout));

    let rollup_client = RollupClient::new(solana_client.clone());

    let sig_1 = rollup_client
        .prepare_tree(&owner, &tree_data_account, 20, 64, 3)
        .await
        .unwrap();
    println!("Prepare tree signature: {sig_1:?}");

    let mut rollup_builder = rollup_client
        .create_rollup_builder(&tree_data_account.pubkey())
        .await
        .unwrap();
    println!(
        "Rollup builder size: {}, {}, {}",
        rollup_builder.max_depth, rollup_builder.max_buffer_size, rollup_builder.canopy_depth
    );

    rollup_builder.add_asset(&owner.pubkey(), &owner.pubkey(), &make_test_metadata(1u8));

    let sig_2 = rollup_client
        .finalize_tree(
            "http://mymetadata.ololo/",
            "mymetadatahash",
            &rollup_builder,
            &owner,
            &owner,
        )
        .await
        .unwrap();
    println!("Finalize tree signature: {sig_2:?}");
}

const TREE_CREATOR: [u8; 64] = [
    71, 169, 21, 15, 207, 98, 125, 163, 177, 187, 118, 170, 54, 221, 34, 196, 99, 60, 80, 127, 202,
    61, 72, 174, 135, 151, 214, 203, 102, 106, 206, 18, 237, 231, 72, 189, 103, 136, 149, 222, 87,
    237, 87, 30, 54, 80, 103, 206, 213, 64, 193, 64, 100, 222, 54, 143, 251, 178, 188, 50, 54, 56,
    87, 36,
];

pub const TREE_KEY: [u8; 64] = [
    48, 111, 197, 10, 137, 43, 207, 116, 57, 156, 24, 173, 58, 78, 235, 43, 129, 29, 81, 185, 140,
    40, 63, 174, 159, 208, 160, 246, 232, 151, 60, 201, 67, 162, 242, 249, 66, 65, 247, 140, 222,
    107, 100, 127, 252, 98, 10, 242, 239, 118, 198, 161, 87, 129, 14, 235, 76, 50, 9, 153, 52, 233,
    11, 108,
];

#[tokio::test]
async fn test_complete_flow_with_test_context() {
    solana_sdk::program_stubs::set_syscall_stubs(Box::new(SyscallStubs {}));
    // user
    let tree_creator = Keypair::from_bytes(TREE_CREATOR.as_ref()).unwrap();

    let tree_key = Keypair::from_bytes(TREE_KEY.as_ref()).unwrap();

    // get test context
    let mut program_context = BubblegumTestContext::new().await.unwrap();
    let payer = program_context.test_context().payer.dirty_clone();
    println!("Balance: {:?}", program_context.client().get_balance(payer.pubkey()).await);

    let governance_program_id =
        Pubkey::from_str("CuyWCRdHT8pZLG793UR5R9z31AC49d47ZW9ggN6P7qZ4").unwrap();
    let realm_authority = Pubkey::from_str("Euec5oQGN3Y9kqVrz6PQRfTpYSn6jK3k1JonDiMTzAtA").unwrap();
    let voter_authority = program_context.test_context().payer.pubkey();

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

    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

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

    let registrar_acc_data = [
        REGISTRAR_DISCRIMINATOR.as_ref(),
        bytemuck::bytes_of(&registrar),
    ]
    .concat();
    let voter_acc_data = [VOTER_DISCRIMINATOR.as_ref(), bytemuck::bytes_of(&voter)].concat();

    // for next two accounts set arbitrary balance because it doesn't meter for test
    let mut registrar_account = AccountSharedData::new(
        10000000000000000,
        registrar_acc_data.len(),
        &mplx_staking_states::ID,
    );
    registrar_account.set_data_from_slice(registrar_acc_data.as_ref());

    let mut voter_account = AccountSharedData::new(
        10000000000000000,
        voter_acc_data.len(),
        &mplx_staking_states::ID,
    );
    voter_account.set_data_from_slice(voter_acc_data.as_ref());

    program_context
        .mut_test_context()
        .set_account(&registrar_key, &registrar_account);
    program_context
        .mut_test_context()
        .set_account(&voter_key, &voter_account);

    let rollup_client = RollupClient::new_from_abstract_solana_client(
        Arc::new(TestContextWrapper(Mutex::new(program_context),
        Mutex::new(0)))
    );

    let sig_1 = rollup_client
        .prepare_tree(&payer, &tree_key, 10, 32, 3)
        .await
        .unwrap();
    println!("Prepare tree signature: {sig_1:?}");

    // println!("Balance: {:?}", rollup_client.client().get_balance(&payer.pubkey()).await);
    // {
    //     rollup_client.make_acc(&payer).await.unwrap();
    // }
    // println!("Balance: {:?}", rollup_client.client().get_balance(&payer.pubkey()).await);

    println!("Account: {:?}", rollup_client.client().get_account(&tree_key.pubkey()).await);

    let mut rollup_builder = rollup_client
        .create_rollup_builder(&tree_key.pubkey())
        .await
        .unwrap();

    println!(
        "Rollup builder size: {}, {}, {}",
        rollup_builder.max_depth, rollup_builder.max_buffer_size, rollup_builder.canopy_depth
    );

    rollup_builder.add_asset(
        &payer.pubkey(),
        &payer.pubkey(),
        &make_test_metadata(1u8)
    );

    let sig_2 = rollup_client
        .finalize_tree(
            "http://mymetadata.ololo/",
            "mymetadatahash",
            &rollup_builder,
            &tree_creator, // tree creator
            &payer, // staker
        )
        .await
        .unwrap();

    println!("Finalize tree signature: {sig_2:?}");
}


/// Loads keypair from "~/.config/solana/id.json",
/// i.e. the one by default used by solana-test-validato
fn id_json_key() -> Keypair {
    let id_json_path = format!(
        "{}/.config/solana/id.json",
        std::env::home_dir().unwrap().to_str().unwrap()
    );

    let id_json = std::fs::read_to_string(id_json_path).expect("Should have been able to read the file");

    let id_bytes_str: String = id_json
        .chars()
        .skip_while(|s| *s != '[')
        .skip(1)
        .take_while(|s| *s != ']')
        .collect();

    let bytes: Vec<u8> = id_bytes_str
        .split(",")
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


struct TestContextWrapper(Mutex<BubblegumTestContext>, Mutex<u64>);

#[async_trait]
impl AbstractSolanaClient for TestContextWrapper {

    async fn get_account(&self, pubkey: &Pubkey) -> std::result::Result<solana_sdk::account::Account, rollup_sdk::errors::RollupError> {

        let mut mutex = self.0.lock().await;
        let client = &mut mutex.program_context.banks_client;
        match client.get_account(*pubkey).await {
            Ok(Some(acc)) => Ok(acc),
            Ok(None) => Err(RollupError::GenricErr(format!("No account: {pubkey:?}"))),
            Err(e) => Err(RollupError::NestedErr(Box::new(e))),
        }
    }

    async fn get_balance(&self, pubkey: &Pubkey) -> std::result::Result<u64, RollupError> {
        let mut mutex = self.0.lock().await;
        let client = &mut mutex.program_context.banks_client;
        let result =  client.get_balance(*pubkey).await
            .map_err(|e|RollupError::NestedErr(Box::new(e)))?;
        Ok(result)
    }

    async fn get_minimum_balance_for_rent_exemption(
        &self,
        data_len: usize,
    ) -> std::result::Result<u64, rollup_sdk::errors::RollupError> {
        Ok(data_len as u64 * solana_program::fee_calculator::DEFAULT_TARGET_LAMPORTS_PER_SIGNATURE)
    }

    async fn get_latest_blockhash(&self) -> std::result::Result<solana_program::hash::Hash, rollup_sdk::errors::RollupError> {
        let mut mutex = self.0.lock().await;
        let client = &mut mutex.program_context.banks_client;
        client.get_latest_blockhash().await
            .map_err(|e| RollupError::NestedErr(Box::new(e)))
    }

    async fn send_and_confirm_transaction(
        &self,
        transaction: &Transaction,
    ) -> std::result::Result<solana_sdk::signature::Signature, rollup_sdk::errors::RollupError> {
        let mut mutex = self.0.lock().await;
        {
            let mut counter_guard = self.1.lock().await;
            if *counter_guard > 0 {
                mutex.program_context.warp_to_slot(*counter_guard * 100).unwrap();
            }
            *counter_guard += 1;
        }
        let client = &mut mutex.program_context.banks_client;
        let signature = client.process_transaction(transaction.clone()).await
            .map(|_| Signature::new_unique())
            .map_err(|e| {
                println!("Original error - {:?}: {}", e, e.to_string());
                RollupError::NestedErr(Box::new(e))
            })?;
        Ok(signature)
    }
}