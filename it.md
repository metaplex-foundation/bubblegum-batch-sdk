# Integration tests

For the integration tests we use `solana-test-validator` instead of `BankClient`,
because the `BankClient` is less stable.

1) Make sure you have [Solana CLI](https://docs.solanalabs.com/cli/install) installed on your machine.
It should be available in the PATH.
If you don't want to have it the PATH, or you have multiple SOlana CLI installations and want to use specific one,
set `SOLANA_HOME` env. variable (it will be picked up by tests), e.g.

```shell
export SOLANA_HOME=/home/stas/dev/sdk/solana-1.18.15/bin
```

2) Prepare dependency.

Note: At the moment, while it is still a development phase, we use fork of bubblegum + account compression from a private repository.

```shell
git clone git@github.com:adm-metaex/mpl-bubblegum.git
cd mpl-bubblegum
git switch -c feature/cleanup origin/feature/cleanup
git submodule update --init --recursive
pnpm install
pnpm programs:build
./configs/scripts/program/temp_prep_commands.sh
```

3) Run `cargo test`

If during tests compilation you have errors like these:

```
error[E0308]: mismatched types
  --> /Users/user/.cargo/git/checkouts/solana-program-library-f541f372004088f4/623df91/account-compression/programs/account-compression/src/noop/mod.rs:18:9
   |
17 |     fn id() -> Pubkey {
   |                ------ expected `anchor_lang::prelude::Pubkey` because of return type
18 |         spl_noop::id()
   |         ^^^^^^^^^^^^^^ expected `anchor_lang::prelude::Pubkey`, found `solana_program::pubkey::Pubkey`
```

or

```
error[E0308]: mismatched types
   --> /Users/user/.cargo/git/checkouts/solana-program-library-f541f372004088f4/623df91/account-compression/programs/account-compression/src/noop/mod.rs:27:9
    |
26  |     invoke(
    |     ------ arguments to this function are incorrect
27  |         &spl_noop::instruction(event.try_to_vec()?),
    |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected `Instruction`, found `solana_program::instruction::Instruction`
```

make sure that your `Cargo.lock` has only one solana-program version and it's **1.18.xx**. In other words drop manually from `.lock` file solana-program with versions different to 1.18.xx.

This problem is happening when cargo loads different solana program versions, for example 1.18.21 and 2.0.4, we need to stick to 1.18.xx.