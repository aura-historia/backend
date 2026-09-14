//! Explicit dev-only fresh initialization or read-only verification; no legacy setup.
mod bootstrap_runtime;

fn main() -> std::process::ExitCode {
    bootstrap_runtime::main_for(bootstrap_runtime::Entrypoint::Dev)
}
