use std::time::SystemTime;

use mplx_staking_states::state::{DepositEntry, Lockup, LockupKind, LockupPeriod, Registrar, Voter, VotingMintConfig};
use solana_program::pubkey;
use solana_sdk::{account::AccountSharedData, pubkey::Pubkey};

pub async fn create_voter_and_registrar(payer: &Pubkey) {}

pub fn make_accs(payer: &Pubkey) {
    let governance_program_id = pubkey!("CuyWCRdHT8pZLG793UR5R9z31AC49d47ZW9ggN6P7qZ4");
    let realm_authority = pubkey!("Euec5oQGN3Y9kqVrz6PQRfTpYSn6jK3k1JonDiMTzAtA");
    let voter_authority = payer.clone();

    let mplx_mint_key = Pubkey::new_unique();
    let grant_authority = Pubkey::new_unique();

    let registrar_key = Pubkey::find_program_address(
        &[
            bubblegum::state::REALM.to_bytes().as_ref(),
            b"registrar".as_ref(),
            bubblegum::state::REALM_GOVERNING_MINT.to_bytes().as_ref(),
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
        realm: bubblegum::state::REALM,
        realm_governing_token_mint: bubblegum::state::REALM_GOVERNING_MINT,
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
        .duration_since(std::time::UNIX_EPOCH)
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
        mplx_staking_states::state::REGISTRAR_DISCRIMINATOR.as_ref(),
        bytemuck::bytes_of(&registrar),
    ]
    .concat();
    let voter_acc_data = [
        mplx_staking_states::state::VOTER_DISCRIMINATOR.as_ref(),
        bytemuck::bytes_of(&voter),
    ]
    .concat();

    // for next two accounts set arbitrary balance because it doesn't meter for test
    let mut registrar_account =
        AccountSharedData::new(10000000000000000, registrar_acc_data.len(), &mplx_staking_states::ID);
    registrar_account.set_data_from_slice(registrar_acc_data.as_ref());

    let mut voter_account = AccountSharedData::new(10000000000000000, voter_acc_data.len(), &mplx_staking_states::ID);
    voter_account.set_data_from_slice(voter_acc_data.as_ref());

    // program_context
    //     .mut_test_context()
    //     .set_account(&registrar_key, &registrar_account);
    // program_context
    //     .mut_test_context()
    //     .set_account(&voter_key, &voter_account);
}

pub fn sighash(namespace: &str, name: &str) -> [u8; 8] {
    let preimage = format!("{}:{}", namespace, name);

    let mut sighash = [0u8; 8];
    sighash.copy_from_slice(&anchor_lang::solana_program::hash::hash(preimage.as_bytes()).to_bytes()[..8]);
    sighash
}

#[test]
fn calc_disc() {
    println!("{:?}", sighash("global", "prepare_tree"));
}
