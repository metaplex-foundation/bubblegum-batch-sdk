# Integration tests

1) Prepare dependency.

At the moment, we use fork of bubblegum + account compression from a private repository.

```
git clone git@github.com:n00m4d/mpl-bubblegum.git
cd mpl-bubblegum
git switch -c feature/cleanup origin/feature/cleanup
git submodule update --init --recursive
pnpm install
pnpm programs:build
./configs/scripts/program/temp_prep_commands.sh
```

2) Run solana-test-validator

```shell
solana-test-validator --reset \
  --bpf-program cmtDvXumGCrqC1Age74AVPhSRVXJMd8PJS91L8KbNCK mpl-bubblegum/account-compression/target/deploy/spl_account_compression.so \
  --bpf-program noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV mpl-bubblegum/programs/.bin/spl_noop.so \
  --bpf-program BGUMAp9Gq7iTEuizy4pqaxsTyUCBK68MDfK752saRPUY mpl-bubblegum/programs/.bin/bubblegum.so
```

3) Run test `test_prepare_tree`


solana-test-validator --reset   --bpf-program cmtDvXumGCrqC1Age74AVPhSRVXJMd8PJS91L8KbNCK solana-contracts/spl_account_compression.so   --bpf-program noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV solana-contracts/spl_noop.so   --bpf-program BGUMAp9Gq7iTEuizy4pqaxsTyUCBK68MDfK752saRPUY mpl-bubblegum/programs/.bin/bubblegum.so --account 7KXf5wqxoDE9QTDdVysHULruroRCemWU9WQEyDcRkUFC /home/stas/dev/proj/rollup-sdk/test_data/accounts/registrar.json --account GnbsGti24qZNiASqysqcwmb2nyk49U8Eaoc7REhF62dJ /home/stas/dev/proj/rollup-sdk/test_data/accounts/voter.json