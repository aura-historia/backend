//! Email templates for tests, laid out like the image's compiled template directory.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

/// A fresh template directory holding every `mjml/<area>/<kind>/<language>.mjml` source as
/// `<area>/<kind>/<language>.html`. MJML sources hold the same Handlebars expressions as their
/// compiled HTML, so they render like the templates the image ships.
pub fn mjml_template_root() -> std::io::Result<PathBuf> {
    let sources = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mjml");
    let root = std::env::temp_dir().join(format!(
        "aura-mail-templates-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    copy_sources(&sources, &sources, &root)?;
    Ok(root)
}

fn copy_sources(sources: &Path, directory: &Path, root: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "node_modules") {
                copy_sources(sources, &path, root)?;
            }
        } else if path
            .extension()
            .is_some_and(|extension| extension == "mjml")
        {
            let relative = path
                .strip_prefix(sources)
                .map_err(std::io::Error::other)?
                .with_extension("html");
            let target = root.join(relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&path, target)?;
        }
    }
    Ok(())
}
