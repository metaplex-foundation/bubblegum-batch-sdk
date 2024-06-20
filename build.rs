const SKIP_INTEGRATION_TESTS: &str = "SKIP_INTEGRATION_TESTS";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed={}", SKIP_INTEGRATION_TESTS);

    if get_bool_env(SKIP_INTEGRATION_TESTS) {
        println!("cargo:rustc-cfg=skip_integration_tests");
    }

    Ok(())
}

fn get_bool_env(var_name: &str) -> bool {
    let Ok(var_value) = std::env::var(var_name) else {
        return false;
    };
    var_value.parse::<bool>().unwrap_or(false)
}
