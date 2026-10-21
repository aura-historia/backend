#[tokio::main(flavor = "current_thread")]
async fn main() {
    match test_api::environment::run_cli(std::env::args_os().skip(1).collect()).await {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("aura-test-env: {error}");
            let mut cause = error.source();
            while let Some(error) = cause {
                eprintln!("  caused by: {error}");
                cause = error.source();
            }
            std::process::exit(1);
        }
    }
}
