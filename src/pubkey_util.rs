use solana_sdk::pubkey::Pubkey;

pub fn get_registrar_key() -> Pubkey{
    let (registrar_key, _) = Pubkey::find_program_address(
        &[
            bubblegum::state::REALM.to_bytes().as_ref(),
            b"registrar".as_ref(),
            bubblegum::state::REALM_GOVERNING_MINT.to_bytes().as_ref(),
        ],
        &mplx_staking_states::ID,
    );
    registrar_key
}

pub fn get_voter_key(staker: &Pubkey, voter_authority: &Pubkey) -> Pubkey {
    let (voter_key, _voter_bump) = Pubkey::find_program_address(
        &[
            staker.to_bytes().as_ref(), // staker or tree_creator?
            b"voter".as_ref(),
            voter_authority.to_bytes().as_ref(),
        ],
        &mplx_staking_states::ID,
    );
    voter_key
}

/// Account that hold additional merkle tree config,
/// aka tree_authority PDA account previously initialized by `prepare_tree`.
pub fn derive_tree_config_account(tree_data_account: &Pubkey) -> Pubkey {
    let (tree_authority, _) =
        Pubkey::find_program_address(&[tree_data_account.as_ref()], &bubblegum::id());
    tree_authority
}