use std::{
    fs::File,
    io::Write,
    path::Path,
    process::{Child, Command},
};

use base64::Engine;
use solana_sdk::pubkey::Pubkey;

const PROGRAM_NAME: &'static str = "solana-test-validator";
const ENV_SOLANA_HOME: &'static str = "SOLANA_HOME";

/// Wrapper for `solana-test-validator` that comes with the distribution of solana.
/// We use `solana-test-validator` for the rollup testing, because it
/// it is much more stable than the BankClient.
///
/// This wrapper allow to launch `solana-test-validator` as separate process,
/// plus specify list of contracts and account to deploy on startup.
pub struct TestValidatorRunner {
    contracts: Vec<ContractToDeploy>,
    accounts: Vec<AccountInit>,
    search_paths: Vec<String>,
}

impl TestValidatorRunner {
    pub fn new() -> TestValidatorRunner {
        TestValidatorRunner {
            contracts: Vec::new(),
            accounts: Vec::new(),
            search_paths: Vec::new(),
        }
    }

    pub fn add_account(&mut self, account: &AccountInit) {
        self.accounts.push(account.clone());
    }

    pub fn add_program(&mut self, program: &ContractToDeploy) {
        self.contracts.push(program.clone());
    }

    pub fn run(&self) -> std::io::Result<Child> {
        // If program is not an absolute path, the PATH will be searched in an OS-defined way.
        let cmd_name = if std::env::var(ENV_SOLANA_HOME).is_ok() {
            Path::new(&std::env::var(ENV_SOLANA_HOME).unwrap())
                .join(PROGRAM_NAME)
                .to_str()
                .unwrap()
                .to_string()
        } else {
            PROGRAM_NAME.to_string()
        };
        let mut cmd = Command::new(cmd_name);
        cmd.arg("--reset");

        for contract in &self.contracts {
            let path_to_so = self
                .find_in_paths(&contract.path)
                .expect(&format!("Cannot find: {}", &contract.path));
            cmd.args(["--bpf-program", &contract.addr.to_string(), &path_to_so]);
        }

        for account in &self.accounts {
            let file_path = write_to_temp_file(&account.name, account.to_json().as_bytes());
            cmd.args(["--account", &account.pubkey.to_string(), &file_path]);
        }

        let child = cmd.spawn()?;

        Ok(child)
    }

    fn find_in_paths(&self, file: &str) -> Option<String> {
        // if the given path to the contract is absolute,
        // or relative to the current dir, i.e. already accessible
        if Path::new(file).exists() {
            return Some(file.to_string());
        }

        for search_path in &self.search_paths {
            let try_path = Path::new(search_path).join(file);
            if try_path.exists() {
                return try_path.to_str().map(|s| s.to_owned());
            }
        }
        None
    }
}

#[derive(Clone, Debug)]
pub struct ContractToDeploy {
    pub addr: Pubkey,
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct AccountInit {
    pub name: String,
    pub pubkey: Pubkey,
    pub data: Vec<u8>,
    pub owner: Pubkey,
}

impl AccountInit {
    pub fn to_json(&self) -> String {
        let pubkey = self.pubkey;
        let data = base64::prelude::BASE64_STANDARD.encode(&self.data);
        let owner = self.owner;
        let space = self.data.len();
        format!(
            r#"
        {{
            "pubkey": "{pubkey}",
            "account": {{
              "lamports": 10000000000000000,
              "data": [
                "{data}",
                "base64"
              ],
              "owner": "{owner}",
              "executable": false,
              "rentEpoch": 18446744073709551615,
              "space": {space}
            }}
        }}
        "#
        )
    }
}

fn write_to_temp_file(name: &str, payload: &[u8]) -> String {
    let dir = std::env::temp_dir();
    let accounts_temp_dir = dir.join("test_sol_programs");
    if !accounts_temp_dir.exists() {
        std::fs::create_dir(&accounts_temp_dir).unwrap();
    }
    let file_path = accounts_temp_dir.join(name);
    let mut file = File::create(&file_path).unwrap();
    file.write_all(payload).unwrap();
    file_path.to_str().unwrap().to_string()
}

#[cfg(test)]
mod test {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_to_json() {
        let acc = AccountInit {
            name: "registrar.json".to_string(),
            pubkey: Pubkey::from_str("7KXf5wqxoDE9QTDdVysHULruroRCemWU9WQEyDcRkUFC").unwrap(),
            data: vec![1, 2, 3],
            owner: Pubkey::from_str("3GepGwMp6WgPqgNa5NuSpnw3rQjYnqHCcVWhVmpGnw6s").unwrap(),
        };
        println!("{}", acc.to_json());
    }
}
