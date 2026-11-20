use aura_historia_ops::{Command, run};
use platform_observability::{LoggingConfig, init};
use platform_process::{config::load_file_backed_environment, install_bounded_panic_hook};
use std::process::ExitCode;

const COMPONENT: &str = "aura-historia-ops";

fn main() -> ExitCode {
    install_bounded_panic_hook(COMPONENT);
    let command = match Command::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(usage) => {
            eprintln!("{usage}\n\n{}", Command::USAGE);
            return ExitCode::from(2);
        }
    };
    if let Err(error) = load_file_backed_environment() {
        eprintln!("{COMPONENT}: {error}");
        return ExitCode::FAILURE;
    }
    init(LoggingConfig::from_env());
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("{COMPONENT}: failed to start the async runtime");
            return ExitCode::FAILURE;
        }
    };
    let name = command.name();
    match runtime.block_on(run(command)) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            // The error chain can carry configuration or provider text; record only the command,
            // the top-level message and the reason, which are static descriptions.
            tracing::error!(
                event = "ops.command.failed",
                command = name,
                error = %error,
                reason = error.reason(),
            );
            ExitCode::FAILURE
        }
    }
}
