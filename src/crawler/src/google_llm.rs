use llm::backends::google::GoogleServiceTier;
use llm::builder::{LLMBackend, LLMBuilder};

pub fn google_llm_builder(
    api_key: &str,
    model: &str,
    service_tier: Option<GoogleServiceTier>,
) -> LLMBuilder {
    let builder = LLMBuilder::new()
        .backend(LLMBackend::Google)
        .api_key(api_key)
        .model(model);

    match service_tier {
        Some(service_tier) => builder.google_service_tier(service_tier),
        None => builder,
    }
}

pub fn google_service_tier_from_env() -> Option<GoogleServiceTier> {
    google_service_tier_for_env(std::env::var("GEMINI_FLEX").ok().as_deref())
}

pub fn google_service_tier_for_env(raw: Option<&str>) -> Option<GoogleServiceTier> {
    match parse_bool_env(raw) {
        Some(true) => Some(GoogleServiceTier::Flex),
        Some(false) | None => None,
    }
}

pub fn google_service_tier_label(service_tier: Option<&GoogleServiceTier>) -> &'static str {
    match service_tier {
        Some(GoogleServiceTier::Standard) => "standard",
        Some(GoogleServiceTier::Flex) => "flex",
        Some(GoogleServiceTier::Priority) => "priority",
        None => "default",
    }
}

fn parse_bool_env(raw: Option<&str>) -> Option<bool> {
    let normalized = raw?.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_enable_flex_for_truthy_env() {
        assert_eq!(
            google_service_tier_for_env(Some("true")),
            Some(GoogleServiceTier::Flex)
        );
        assert_eq!(
            google_service_tier_for_env(Some("1")),
            Some(GoogleServiceTier::Flex)
        );
        assert_eq!(
            google_service_tier_for_env(Some("yes")),
            Some(GoogleServiceTier::Flex)
        );
    }

    #[test]
    fn should_disable_flex_for_falsey_or_missing_env() {
        assert_eq!(google_service_tier_for_env(Some("false")), None);
        assert_eq!(google_service_tier_for_env(Some("0")), None);
        assert_eq!(google_service_tier_for_env(None), None);
    }

    #[test]
    fn should_ignore_invalid_env_values() {
        assert_eq!(google_service_tier_for_env(Some("sometimes")), None);
        assert_eq!(google_service_tier_for_env(Some("")), None);
    }

    #[test]
    fn should_format_google_service_tier_labels() {
        assert_eq!(
            google_service_tier_label(Some(&GoogleServiceTier::Flex)),
            "flex"
        );
        assert_eq!(google_service_tier_label(None), "default");
    }
}
