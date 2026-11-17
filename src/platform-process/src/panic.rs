/// Replaces the default panic hook, whose output includes the panic payload. A payload can carry
/// provider responses, persisted values or credentials, so only the component and the location
/// are written. Callers that contain panics still observe them as failures.
pub fn install_bounded_panic_hook(component: &'static str) {
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_default();
        eprintln!("{component} panicked at {location}");
    }));
}
