use llm::backends::google::GoogleServiceTier;
use llm::builder::{LLMBackend, LLMBuilder};

pub fn google_llm_builder(api_key: &str, model: &str, gemini_flex: bool) -> LLMBuilder {
    let builder = LLMBuilder::new()
        .backend(LLMBackend::Google)
        .api_key(api_key)
        .model(model);

    if gemini_flex {
        builder.google_service_tier(GoogleServiceTier::Flex)
    } else {
        builder
    }
}

pub fn gemini_flex_enabled() -> bool {
    std::env::var("GEMINI_FLEX")
        .ok()
        .is_some_and(|raw| parse_gemini_flex(&raw))
}

fn parse_gemini_flex(raw: &str) -> bool {
    let raw = raw.trim();
    raw == "1" || raw.eq_ignore_ascii_case("true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_enable_flex_for_truthy_env() {
        assert!(parse_gemini_flex("true"));
        assert!(parse_gemini_flex("1"));
    }

    #[test]
    fn should_disable_flex_for_falsey_or_missing_env() {
        assert!(!parse_gemini_flex("false"));
        assert!(!parse_gemini_flex("0"));
    }

    #[test]
    fn should_ignore_invalid_env_values() {
        assert!(!parse_gemini_flex("sometimes"));
        assert!(!parse_gemini_flex(""));
    }

    #[test]
    fn should_enable_flex_case_insensitively_for_true() {
        assert!(parse_gemini_flex("TRUE"));
    }

    #[test]
    fn should_ignore_surrounding_whitespace() {
        assert!(parse_gemini_flex(" true "));
    }
}
