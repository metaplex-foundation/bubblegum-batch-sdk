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
