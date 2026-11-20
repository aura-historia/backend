//! Test-only helpers shared by the adapter's unit tests.
use crate::template_mapping::{EmailLanguage, EmailTemplateType, template_path};

static NEXT_ROOT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A fresh template root holding `content` for every template, with `overrides` replacing
/// or (`None`) removing single templates.
pub(crate) fn template_root(
    content: &str,
    overrides: &[(&str, Option<&str>)],
) -> std::io::Result<std::path::PathBuf> {
    let root = std::env::temp_dir().join(format!(
        "notification-email-templates-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    for template_type in EmailTemplateType::ALL {
        for language in EmailLanguage::ALL {
            let path = root.join(template_path(template_type, language));
            std::fs::create_dir_all(path.parent().unwrap_or(&root))?;
            std::fs::write(path, content)?;
        }
    }
    for (template, replacement) in overrides {
        match replacement {
            Some(replacement) => std::fs::write(root.join(template), replacement)?,
            None => std::fs::remove_file(root.join(template))?,
        }
    }
    Ok(root)
}
