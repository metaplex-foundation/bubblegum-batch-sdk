use mpl_common_constants::constants::{DAO_GOVERNING_MINT, DAO_PUBKEY};
use mplx_rewards::utils::find_mining_program_address;
use solana_sdk::pubkey;
use solana_sdk::pubkey::Pubkey;

// todo: import from package with staking/rewards constants
pub const REWARD_POOL_ADDRESS: Pubkey = pubkey!("J9iTArkeHKahfAiKcFYKK128EC3rBr8ZyVthCE7TE6F9");

pub fn get_registrar_key() -> Pubkey {
    let (registrar_key, _) = Pubkey::find_program_address(
        &[DAO_PUBKEY.as_ref(), b"registrar".as_ref(), DAO_GOVERNING_MINT.as_ref()],
        &mplx_staking_states::ID,
    );
    registrar_key
}

/// ## Arguments
/// `registrar_account` - registrar
/// `voter_authority` - payer
pub fn get_voter_key(registrar_account: &Pubkey, voter_authority: &Pubkey) -> Pubkey {
    let (voter_key, _voter_bump) = Pubkey::find_program_address(
        &[
            registrar_account.to_bytes().as_ref(), // staker or tree_creator?
            b"voter".as_ref(),
            voter_authority.to_bytes().as_ref(),
        ],
        &mplx_staking_states::ID,
    );
    voter_key
}

pub fn get_mining_key(staker: &Pubkey) -> Pubkey {
    find_mining_program_address(&mplx_rewards::id(), staker, &REWARD_POOL_ADDRESS).0
}

/// Account that hold additional merkle tree config,
/// aka tree_authority PDA account previously initialized by `prepare_tree`.
pub fn derive_tree_config_account(tree_data_account: &Pubkey) -> Pubkey {
    let (tree_authority, _bump) = Pubkey::find_program_address(&[tree_data_account.as_ref()], &mpl_bubblegum::ID);
    tree_authority
}

pub fn discriminator(namespace: &str, name: &str) -> [u8; 8] {
    let preimage = format!("{}:{}", namespace, name);

    let mut sighash = [0u8; 8];
    sighash.copy_from_slice(&anchor_lang::solana_program::hash::hash(preimage.as_bytes()).to_bytes()[..8]);
    sighash
}
