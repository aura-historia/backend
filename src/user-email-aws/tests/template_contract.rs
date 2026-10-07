use handlebars::Handlebars;
use serde_json::json;

const TEMPLATES: [(&str, &str, &str, &str); 5] = [
    (
        "de",
        include_str!("../../../mjml/newsletter/confirmation/de.mjml"),
        "Hallo ",
        "Guten Tag,",
    ),
    (
        "en",
        include_str!("../../../mjml/newsletter/confirmation/en.mjml"),
        "Hello ",
        "Hello,",
    ),
    (
        "es",
        include_str!("../../../mjml/newsletter/confirmation/es.mjml"),
        "Hola ",
        "Hola,",
    ),
    (
        "fr",
        include_str!("../../../mjml/newsletter/confirmation/fr.mjml"),
        "Bonjour ",
        "Bonjour,",
    ),
    (
        "it",
        include_str!("../../../mjml/newsletter/confirmation/it.mjml"),
        "Ciao ",
        "Salve,",
    ),
];

#[test]
fn newsletter_confirmation_templates_render_with_delivery_contract() {
    let mut handlebars = Handlebars::new();
    handlebars.set_strict_mode(true);

    for (language, template, greeting, fallback) in TEMPLATES {
        assert!(
            template.contains("{{confirmation_url}}") && !template.contains("{{{"),
            "{language} must use an escaped confirmation_url expression"
        );
        let root = mrml::parse(template)
            .unwrap_or_else(|error| panic!("{language} failed MJML parsing: {error}"));
        let html = root
            .element
            .render(&mrml::prelude::render::RenderOptions::default())
            .unwrap_or_else(|error| panic!("{language} failed MJML rendering: {error}"));
        assert!(
            html.contains("{{confirmation_url}}") && !html.contains("{{{"),
            "{language} compiled HTML must keep an escaped confirmation_url expression"
        );
        let confirmation_url =
            format!("https://aura-historia.com/{language}/newsletter/confirm#token=abc123");

        for first_name in [Some("<Ada & Co>"), None] {
            let rendered = handlebars
                .render_template(
                    &html,
                    &json!({
                        "confirmation_url": confirmation_url,
                        "first_name": first_name,
                    }),
                )
                .unwrap_or_else(|error| panic!("{language} failed to render: {error}"));

            assert!(
                !rendered.contains("{{") && !rendered.contains("}}"),
                "{language} left an unresolved expression"
            );
            let href = rendered
                .split("href=\"")
                .skip(1)
                .filter_map(|rest| rest.split('"').next())
                .find(|href| {
                    href.starts_with(&format!(
                        "https://aura-historia.com/{language}/newsletter/confirm#token"
                    ))
                })
                .unwrap_or_else(|| panic!("{language} is missing the confirmation link"));
            assert_ne!(href, confirmation_url, "{language} did not escape the URL");
            assert_eq!(
                html_escape::decode_html_entities(href),
                confirmation_url,
                "{language} confirmation link changed during escaping"
            );

            match first_name {
                Some(_) => {
                    assert!(
                        rendered.contains(&format!("{greeting}&lt;Ada &amp; Co&gt;,")),
                        "{language} did not render the escaped personal greeting"
                    );
                    assert!(
                        !rendered.contains("<Ada & Co>"),
                        "{language} exposed the raw name"
                    );
                    assert!(
                        !rendered.contains(fallback),
                        "{language} rendered the fallback with a name"
                    );
                }
                None => {
                    assert!(
                        rendered.contains(fallback),
                        "{language} did not render the anonymous greeting"
                    );
                    assert!(
                        !rendered.contains("Ada"),
                        "{language} rendered a missing name"
                    );
                }
            }
        }
    }
}
