use std::sync::{Mutex, Once};

/// Global registry of cleanup functions to call when the process is interrupted.
///
/// Each test-infrastructure module (LocalStack, Postgres, …) registers its own `cleanup`
/// function here via [`register_signal_cleanup`]. The shared `ctrlc` handler installed
/// once for the whole process iterates over every registered function so that all related
/// Docker containers are removed on `CTRL+C` or CI-cancellation (SIGTERM).
static CLEANUP_REGISTRY: Mutex<Vec<fn()>> = Mutex::new(Vec::new());

/// Ensures the `ctrlc` signal handler is installed exactly once.
static CTRLC_INIT: Once = Once::new();

/// Registers `f` to be called when the process receives `SIGINT` or `SIGTERM`.
///
/// The first caller also installs the `ctrlc` handler that drives all registered
/// functions. Subsequent callers only append to the registry — the handler is already
/// in place and will pick up the new entry automatically.
///
/// After running all cleanup functions the handler calls [`std::process::exit`] with
/// exit-code `1`, which in turn triggers any `atexit` handlers already installed
/// (e.g. the `libc::atexit(cleanup)` registrations in each module).
pub(crate) fn register_signal_cleanup(f: fn()) {
    CLEANUP_REGISTRY
        .lock()
        .expect("shouldn't fail locking signal cleanup registry")
        .push(f);

    CTRLC_INIT.call_once(|| {
        ctrlc::set_handler(|| {
            let registry = CLEANUP_REGISTRY
                .lock()
                .expect("shouldn't fail locking signal cleanup registry in handler");
            for cleanup in registry.iter() {
                cleanup();
            }
            std::process::exit(1);
        })
        .expect("shouldn't fail installing ctrlc handler");
    });
}
