use localization::Language;

use notification_core::{
    notification::{
        LocalizedNotificationContent, LocalizedNotificationWatchlistChange, NotificationContent,
        NotificationWatchlistChange, PartnershipApplicationDecision,
    },
    presentation::present_image,
};
use notification_service::ports::notification_delivery_repository::NotificationDeliverySource;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing_price::ProductListingPrice,
    product_listing_slug_id::ProductListingSlugId,
};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmailLanguage {
    De,
    En,
    Fr,
    Es,
    It,
}

impl EmailLanguage {
    pub(crate) const fn resolve(language: Language) -> Self {
        match language {
            Language::De => Self::De,
            Language::Fr => Self::Fr,
            Language::Es => Self::Es,
            Language::It => Self::It,
            Language::En
            | Language::Zh
            | Language::Pt
            | Language::Pl
            | Language::Tr
            | Language::Nl
            | Language::Cs
            | Language::Ja
            | Language::Ru
            | Language::Ar => Self::En,
        }
    }

    pub(crate) const fn as_language(self) -> Language {
        match self {
            Self::De => Language::De,
            Self::En => Language::En,
            Self::Fr => Language::Fr,
            Self::Es => Language::Es,
            Self::It => Language::It,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::De => "de",
            Self::En => "en",
            Self::Fr => "fr",
            Self::Es => "es",
            Self::It => "it",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmailTemplateType {
    WatchlistUpdatePrice,
    WatchlistUpdateAvailability,
    SearchFilterMatch,
    SavedSearchDigest,
    PartnershipApplicationApproval,
    PartnershipApplicationRejection,
}

pub(crate) const fn template_type(content: &NotificationContent) -> EmailTemplateType {
    match content {
        NotificationContent::Watchlist {
            change: NotificationWatchlistChange::PriceChange { .. },
            ..
        } => EmailTemplateType::WatchlistUpdatePrice,
        NotificationContent::Watchlist { .. } => EmailTemplateType::WatchlistUpdateAvailability,
        NotificationContent::SearchFilter { .. } => EmailTemplateType::SearchFilterMatch,
        NotificationContent::PartnershipApplication {
            decision: PartnershipApplicationDecision::Approved,
            ..
        } => EmailTemplateType::PartnershipApplicationApproval,
        NotificationContent::PartnershipApplication { .. } => {
            EmailTemplateType::PartnershipApplicationRejection
        }
    }
}

pub(crate) const fn template_directory(template_type: EmailTemplateType) -> &'static str {
    match template_type {
        EmailTemplateType::WatchlistUpdatePrice => "mjml/watchlist/product-update/price",
        EmailTemplateType::WatchlistUpdateAvailability => {
            "mjml/watchlist/product-update/availability"
        }
        EmailTemplateType::SearchFilterMatch => "mjml/search-filter/match",
        EmailTemplateType::SavedSearchDigest => "mjml/search-filter/digest",
        EmailTemplateType::PartnershipApplicationApproval => {
            "mjml/partnership-application/approval"
        }
        EmailTemplateType::PartnershipApplicationRejection => {
            "mjml/partnership-application/rejection"
        }
    }
}

pub(crate) fn s3_template_key(
    stage: &str,
    commit_sha: &str,
    template_type: EmailTemplateType,
    language: EmailLanguage,
) -> String {
    format!(
        "{stage}/{commit_sha}/{}/{}.html",
        template_directory(template_type),
        language.as_str()
    )
}

pub(crate) const fn ses_template_tag_value(template_type: EmailTemplateType) -> &'static str {
    match template_type {
        EmailTemplateType::WatchlistUpdatePrice => "WATCHLIST_UPDATE_PRICE",
        EmailTemplateType::WatchlistUpdateAvailability => "WATCHLIST_UPDATE_AVAILABILITY",
        EmailTemplateType::SearchFilterMatch => "SEARCH_FILTER_MATCH",
        EmailTemplateType::SavedSearchDigest => "SAVED_SEARCH_DIGEST",
        EmailTemplateType::PartnershipApplicationApproval => "PARTNERSHIP_APPLICATION_APPROVAL",
        EmailTemplateType::PartnershipApplicationRejection => "PARTNERSHIP_APPLICATION_REJECTION",
    }
}

pub(crate) const fn subject(
    template_type: EmailTemplateType,
    language: EmailLanguage,
) -> &'static str {
    match language {
        EmailLanguage::De => match template_type {
            EmailTemplateType::SavedSearchDigest => "Neue Treffer für deine gespeicherten Suchen",
            EmailTemplateType::WatchlistUpdatePrice => {
                "Der Preis auf deiner Merkliste hat sich geändert"
            }
            EmailTemplateType::WatchlistUpdateAvailability => {
                "Die Verfügbarkeit eines Artikels auf deiner Merkliste hat sich geändert"
            }
            EmailTemplateType::SearchFilterMatch => "Neuer Treffer für deinen Suchfilter",
            EmailTemplateType::PartnershipApplicationApproval => "Partnerschaftsantrag genehmigt",
            EmailTemplateType::PartnershipApplicationRejection => {
                "Update zu deinem Partnerschaftsantrag"
            }
        },
        EmailLanguage::Fr => match template_type {
            EmailTemplateType::SavedSearchDigest => {
                "Nouveaux résultats pour vos recherches enregistrées"
            }
            EmailTemplateType::WatchlistUpdatePrice => {
                "Le prix de votre liste de souhaits a changé"
            }
            EmailTemplateType::WatchlistUpdateAvailability => {
                "La disponibilité d’un article de votre liste de souhaits a changé"
            }
            EmailTemplateType::SearchFilterMatch => {
                "Nouveau résultat pour votre filtre de recherche"
            }
            EmailTemplateType::PartnershipApplicationApproval => "Demande de partenariat approuvée",
            EmailTemplateType::PartnershipApplicationRejection => {
                "Mise à jour de votre demande de partenariat"
            }
        },
        EmailLanguage::Es => match template_type {
            EmailTemplateType::SavedSearchDigest => {
                "Nuevos resultados para tus búsquedas guardadas"
            }
            EmailTemplateType::WatchlistUpdatePrice => {
                "El precio de tu lista de deseos ha cambiado"
            }
            EmailTemplateType::WatchlistUpdateAvailability => {
                "La disponibilidad de un artículo de tu lista de deseos ha cambiado"
            }
            EmailTemplateType::SearchFilterMatch => "Nuevo resultado para tu filtro de búsqueda",
            EmailTemplateType::PartnershipApplicationApproval => "Solicitud de asociación aprobada",
            EmailTemplateType::PartnershipApplicationRejection => {
                "Actualización de tu solicitud de asociación"
            }
        },
        EmailLanguage::It => match template_type {
            EmailTemplateType::SavedSearchDigest => "Nuovi risultati per le tue ricerche salvate",
            EmailTemplateType::WatchlistUpdatePrice => {
                "Il prezzo della tua lista dei desideri è cambiato"
            }
            EmailTemplateType::WatchlistUpdateAvailability => {
                "La disponibilità di un articolo nella tua lista dei desideri è cambiata"
            }
            EmailTemplateType::SearchFilterMatch => "Nuovo risultato per il tuo filtro di ricerca",
            EmailTemplateType::PartnershipApplicationApproval => {
                "Richiesta di partnership approvata"
            }
            EmailTemplateType::PartnershipApplicationRejection => {
                "Aggiornamento della tua richiesta di partnership"
            }
        },
        EmailLanguage::En => match template_type {
            EmailTemplateType::SavedSearchDigest => "New matches for your saved searches",
            EmailTemplateType::WatchlistUpdatePrice => "Your watchlist price changed",
            EmailTemplateType::WatchlistUpdateAvailability => {
                "Your watchlist item's availability changed"
            }
            EmailTemplateType::SearchFilterMatch => "New search filter match",
            EmailTemplateType::PartnershipApplicationApproval => "Partnership application approved",
            EmailTemplateType::PartnershipApplicationRejection => "Partnership application update",
        },
    }
}

/// Links into the web app, localized with the email language. Paths mirror the
/// web app's routes (`/$lng/...`), which are the source of truth.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrontendLinks<'a> {
    origin: &'a str,
    language: EmailLanguage,
}

impl<'a> FrontendLinks<'a> {
    /// `origin` is a validated bare origin without a trailing slash.
    pub(crate) const fn new(origin: &'a str, language: EmailLanguage) -> Self {
        Self { origin, language }
    }

    fn page(self, path: &str) -> String {
        format!("{}/{}{path}", self.origin, self.language.as_str())
    }

    fn home(self) -> String {
        self.page("")
    }

    fn product_listing(self, slug_id: &ProductListingSlugId) -> String {
        self.page(&format!("/products/{slug_id}"))
    }

    fn watchlist(self) -> String {
        self.page("/me/watchlist")
    }

    fn search_filters(self) -> String {
        self.page("/me/search-filters")
    }

    fn search_filter(self, search_filter_id: &str) -> String {
        self.page(&format!("/me/search-filter/{search_filter_id}"))
    }

    fn partner_applications(self) -> String {
        self.page("/partners/applications")
    }
}

pub(crate) fn template_data(
    source: &NotificationDeliverySource,
    email_language: EmailLanguage,
    first_name: Option<&str>,
    links: FrontendLinks<'_>,
) -> Value {
    if let Some(digest) = &source.saved_search_digest {
        return digest_template_data(source, digest, email_language, first_name, links);
    }
    let localized = source
        .content
        .clone()
        .localized(&[email_language.as_language()]);
    let present_image_url = |image, content_policy| {
        present_image(
            image,
            content_policy,
            source
                .presentation_preferences
                .show_unassessed_or_sensitive_content,
        )
        .and_then(|image| image.url)
        .map(|url| url.to_string())
    };
    match localized {
        LocalizedNotificationContent::Watchlist {
            snapshot, change, ..
        } => {
            let mut data = product_template_data(
                links,
                snapshot.listing_source_name.to_string(),
                &snapshot.product_listing_title_slug_id,
                snapshot.title.map(|title| title.payload.to_string()),
                present_image_url(snapshot.image, snapshot.content_policy),
                snapshot.view_url.to_string(),
            );
            match change {
                LocalizedNotificationWatchlistChange::PriceChange {
                    old_price,
                    new_price,
                } => {
                    data["old_price"] = json!(price_text(old_price, email_language));
                    data["new_price"] = json!(price_text(new_price, email_language));
                    data["notification_type"] = json!("price_change");
                }
                LocalizedNotificationWatchlistChange::AvailabilityChange {
                    old_availability,
                    new_availability,
                } => {
                    data["old_availability"] = json!(old_availability.map(|availability| {
                        availability_text(availability, email_language.as_language())
                    }));
                    data["new_availability"] = json!(new_availability.map(|availability| {
                        availability_text(availability, email_language.as_language())
                    }));
                    data["notification_type"] = json!("availability_change");
                }
            }
            data["watchlist_url"] = json!(links.watchlist());
            add_recipient_data(data, first_name, links)
        }
        LocalizedNotificationContent::SearchFilter {
            snapshot,
            user_search_filter_id,
            user_search_filter_name,
            ..
        } => {
            let mut data = product_template_data(
                links,
                snapshot.listing_source_name.to_string(),
                &snapshot.product_listing_title_slug_id,
                snapshot.title.map(|title| title.payload.to_string()),
                present_image_url(snapshot.image, snapshot.content_policy),
                snapshot.view_url.to_string(),
            );
            let search_filter_id = user_search_filter_id.to_string();
            data["search_filter_url"] = json!(links.search_filter(&search_filter_id));
            data["search_filter_id"] = json!(search_filter_id);
            data["search_filter_name"] = json!(user_search_filter_name.to_string());
            data["notification_type"] = json!("search_filter_match");
            add_recipient_data(data, first_name, links)
        }
        LocalizedNotificationContent::PartnershipApplication {
            snapshot, decision, ..
        } => add_recipient_data(
            json!({
                "party_name": snapshot.party_name.to_string(),
                "listing_source_name": snapshot.listing_source_name.to_string(),
                "image_url": snapshot.image.map(|image| image.to_string()),
                "partner_dashboard_url": links.partner_applications(),
                "notification_type": match decision {
                    PartnershipApplicationDecision::Approved => "partnership_application_approval",
                    PartnershipApplicationDecision::Rejected => "partnership_application_rejection",
                },
            }),
            first_name,
            links,
        ),
    }
}

fn add_recipient_data(
    mut data: Value,
    first_name: Option<&str>,
    links: FrontendLinks<'_>,
) -> Value {
    data["first_name"] = json!(first_name);
    data["home_url"] = json!(links.home());
    data
}

const DIGEST_PREVIEW_LIMIT: usize = 20;

fn digest_template_data(
    source: &NotificationDeliverySource,
    digest: &notification_service::saved_search_digest::SavedSearchEmailDigest,
    language: EmailLanguage,
    first_name: Option<&str>,
    links: FrontendLinks<'_>,
) -> Value {
    use std::collections::BTreeMap;
    let mut listings: BTreeMap<String, Value> = BTreeMap::new();
    let mut groups: BTreeMap<String, Value> = BTreeMap::new();
    for content in &digest.members {
        if let LocalizedNotificationContent::SearchFilter {
            product_listing_id,
            snapshot,
            user_search_filter_id,
            user_search_filter_name,
        } = content.clone().localized(&[language.as_language()])
        {
            let filter_id = user_search_filter_id.to_string();
            let label = json!({ "id": filter_id, "name": user_search_filter_name.to_string(), "url": links.search_filter(&filter_id) });
            let group = groups.entry(filter_id).or_insert_with(|| {
                let mut group = label.clone();
                group["match_count"] = json!(0);
                group
            });
            group["match_count"] = json!(group["match_count"].as_u64().unwrap_or(0) + 1);
            let card = listings
                .entry(product_listing_id.to_string())
                .or_insert_with(|| {
                    let image = present_image(
                        snapshot.image,
                        snapshot.content_policy,
                        source
                            .presentation_preferences
                            .show_unassessed_or_sensitive_content,
                    )
                    .and_then(|image| image.url)
                    .map(|url| url.to_string());
                    let mut card = product_template_data(
                        links,
                        snapshot.listing_source_name.to_string(),
                        &snapshot.product_listing_title_slug_id,
                        snapshot.title.map(|title| title.payload.to_string()),
                        image,
                        snapshot.view_url.to_string(),
                    );
                    card["searches"] = json!([]);
                    card
                });
            if let Some(searches) = card["searches"].as_array_mut() {
                searches.push(label);
            }
        }
    }
    for card in listings.values_mut() {
        if let Some(labels) = card["searches"].as_array_mut() {
            labels.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
            labels.dedup_by(|a, b| a["id"] == b["id"]);
        }
    }
    let listing_count = listings.len();
    let cards = listings
        .into_values()
        .take(DIGEST_PREVIEW_LIMIT)
        .collect::<Vec<_>>();
    tracing::info!(digest_id = %digest.digest_id, run_end = %digest.window_end, eligible_matches = digest.members.len(), deduplicated_listings = listing_count, rendered_preview_items = cards.len(), "saved_search.digest.rendered");
    add_recipient_data(
        json!({
            "match_count": digest.members.len(), "listing_count": listing_count,
            "preview_count": cards.len(), "preview_limit": DIGEST_PREVIEW_LIMIT,
            "has_more": listing_count > DIGEST_PREVIEW_LIMIT, "groups": groups.into_values().collect::<Vec<_>>(),
            "listings": cards, "results_url": links.search_filters(),
            "settings_url": links.search_filters(), "notification_type": "saved_search_digest",
        }),
        first_name,
        links,
    )
}
fn product_template_data(
    links: FrontendLinks<'_>,
    listing_source_name: String,
    product_listing_title_slug_id: &ProductListingSlugId,
    title: Option<String>,
    image_url: Option<String>,
    view_url: String,
) -> Value {
    json!({
        "listing_source_name": listing_source_name,
        "product_listing_url": links.product_listing(product_listing_title_slug_id),
        "title": title,
        "image_url": image_url,
        "view_url": view_url,
    })
}

fn price_text(price: Option<ProductListingPrice>, language: EmailLanguage) -> Option<String> {
    price.map(|price| match price {
        ProductListingPrice::Monetary(price) => price.format_human_readable(),
        ProductListingPrice::OnRequest => match language {
            EmailLanguage::De => "Preis auf Anfrage",
            EmailLanguage::En => "Price on request",
            EmailLanguage::Fr => "Prix sur demande",
            EmailLanguage::Es => "Precio a consultar",
            EmailLanguage::It => "Prezzo su richiesta",
        }
        .to_owned(),
    })
}

fn availability_text(availability: ListingAvailability, language: Language) -> &'static str {
    match availability {
        ListingAvailability::Available => match language {
            Language::De => "Verfügbar",
            Language::Fr => "Disponible",
            Language::Es => "Disponible",
            Language::It => "Disponibile",
            _ => "Available",
        },
        ListingAvailability::InStock => match language {
            Language::De => "Auf Lager",
            Language::Fr => "En stock",
            Language::Es => "En stock",
            Language::It => "Disponibile",
            _ => "In stock",
        },
        ListingAvailability::LimitedAvailability => match language {
            Language::De => "Begrenzt verfügbar",
            Language::Fr => "Disponibilité limitée",
            Language::Es => "Disponibilidad limitada",
            Language::It => "Disponibilità limitata",
            _ => "Limited availability",
        },
        ListingAvailability::BackOrder => match language {
            Language::De => "Nachbestellung",
            Language::Fr => "Sur commande",
            Language::Es => "Bajo pedido",
            Language::It => "Su ordinazione",
            _ => "Backorder",
        },
        ListingAvailability::MadeToOrder => match language {
            Language::De => "Auf Bestellung gefertigt",
            Language::Fr => "Fabriqué sur commande",
            Language::Es => "Fabricado por encargo",
            Language::It => "Realizzato su ordinazione",
            _ => "Made to order",
        },
        ListingAvailability::PreOrder => match language {
            Language::De => "Vorbestellung",
            Language::Fr => "Précommande",
            Language::Es => "Preventa",
            Language::It => "Preordine",
            _ => "Pre-order",
        },
        ListingAvailability::PreSale => match language {
            Language::De => "Vorverkauf",
            Language::Fr => "Vente anticipée",
            Language::Es => "Venta anticipada",
            Language::It => "Vendita anticipata",
            _ => "Presale",
        },
        ListingAvailability::Unavailable => match language {
            Language::De => "Nicht verfügbar",
            Language::Fr => "Indisponible",
            Language::Es => "No disponible",
            Language::It => "Non disponibile",
            _ => "Unavailable",
        },
        ListingAvailability::Reserved => match language {
            Language::De => "Reserviert",
            Language::Fr => "Réservé",
            Language::Es => "Reservado",
            Language::It => "Riservato",
            _ => "Reserved",
        },
        ListingAvailability::OutOfStock => match language {
            Language::De => "Nicht auf Lager",
            Language::Fr => "Rupture de stock",
            Language::Es => "Agotado",
            Language::It => "Esaurito",
            _ => "Out of stock",
        },
        ListingAvailability::SoldOut => match language {
            Language::De => "Ausverkauft",
            Language::Fr => "Épuisé",
            Language::Es => "Agotado",
            Language::It => "Esaurito",
            _ => "Sold out",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::event_id::EventId;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::Language;
    use money::{Currency, MonetaryAmount, Price};
    use notification_core::notification_id::NotificationId;
    use notification_core::{
        notification::{
            NotificationContent, NotificationWatchlistChange, ProductListingNotificationSnapshot,
        },
        notification_delivery::{NotificationDeliveryChannel, NotificationDeliveryTargetKey},
        notification_delivery_id::NotificationDeliveryId,
    };
    use notification_service::{
        ports::notification_delivery_repository::NotificationDeliverySource,
        presentation::NotificationPresentationPreferences,
    };
    use product_listing_core::{
        content_policy::{ContentPolicyDecision, SensitiveContentCategory},
        listing_availability::ListingAvailability,
        product_listing_id::ProductListingId,
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
    };
    use rstest::rstest;
    use std::{
        collections::{BTreeSet, HashMap},
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };
    use url::Url;
    use user_core::user_id::UserId;

    #[test]
    fn digest_deduplicates_labels_bounds_preview_and_keeps_full_accounting()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, Some(ContentPolicyDecision::Allowed))?;
        let NotificationContent::Watchlist { mut snapshot, .. } = source.content.clone() else {
            panic!("fixture")
        };
        snapshot.title = Some(HashMap::from([
            (
                Language::En,
                serde_json::from_value(json!("<script>English & title</script>"))?,
            ),
            (
                Language::De,
                serde_json::from_value(json!("Deutscher Titel"))?,
            ),
        ]));
        let first_filter = search_filter_core::user_search_filter_id::UserSearchFilterId::new();
        let second_filter = search_filter_core::user_search_filter_id::UserSearchFilterId::new();
        let mut members = (0..25)
            .map(|_| NotificationContent::SearchFilter {
                origin_event_id: EventId::new(),
                product_listing_id: ProductListingId::new(),
                user_search_filter_id: first_filter,
                snapshot: snapshot.clone(),
                user_search_filter_name: "<b>Saved & search</b>".into(),
            })
            .collect::<Vec<_>>();
        let mut shared = members[0].clone();
        if let NotificationContent::SearchFilter {
            user_search_filter_id,
            user_search_filter_name,
            ..
        } = &mut shared
        {
            *user_search_filter_id = second_filter;
            *user_search_filter_name = "Other search".into();
        }
        members.push(shared);
        source.saved_search_digest = Some(
            notification_service::saved_search_digest::SavedSearchEmailDigest {
                digest_id: notification_core::saved_search_digest_id::SavedSearchDigestId::new(),
                window_end: time::OffsetDateTime::UNIX_EPOCH,
                members,
            },
        );
        let data = template_data(
            &source,
            EmailLanguage::En,
            Some("<img src=x>"),
            links(EmailLanguage::En),
        );
        assert_eq!(26, data["match_count"]);
        assert_eq!(25, data["listing_count"]);
        assert_eq!(20, data["preview_count"]);
        assert_eq!(true, data["has_more"]);
        assert_eq!(2, data["groups"].as_array().unwrap().len());
        let lists = data["listings"].as_array().unwrap();
        assert_eq!(20, lists.len());
        assert_eq!(2, lists[0]["searches"].as_array().unwrap().len());
        source
            .saved_search_digest
            .as_mut()
            .unwrap()
            .members
            .reverse();
        assert_eq!(
            data,
            template_data(
                &source,
                EmailLanguage::En,
                Some("<img src=x>"),
                links(EmailLanguage::En)
            )
        );
        let german = template_data(&source, EmailLanguage::De, None, links(EmailLanguage::De));
        assert_eq!("Deutscher Titel", german["listings"][0]["title"]);
        let sensitive = source.saved_search_digest.as_mut().unwrap();
        for member in &mut sensitive.members {
            if let NotificationContent::SearchFilter { snapshot, .. } = member {
                snapshot.content_policy = Some(ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany,
                ));
            }
        }
        assert!(
            template_data(&source, EmailLanguage::En, None, links(EmailLanguage::En))["listings"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["image_url"].is_null())
        );
        let templates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../mjml/search-filter/digest");
        for language in [
            EmailLanguage::En,
            EmailLanguage::De,
            EmailLanguage::Fr,
            EmailLanguage::Es,
            EmailLanguage::It,
        ] {
            let template =
                fs::read_to_string(templates.join(format!("{}.mjml", language.as_str())))?;
            let rendered = handlebars::Handlebars::new().render_template(&template, &data)?;
            assert!(!rendered.contains("<script>"));
            assert!(!rendered.contains("<img src=x>"));
            assert!(!rendered.contains("<b>Saved & search</b>"));
            assert!(rendered.contains("&lt;script&gt;English &amp; title&lt;/script&gt;"));
            assert!(rendered.contains("https://stage.aura-historia.com/en/me/search-filter/"));
            assert!(rendered.contains("https://stage.aura-historia.com/en/me/search-filters\""));
            assert!(rendered.contains("https://stage.aura-historia.com/en/products/"));
            assert!(!subject(EmailTemplateType::SavedSearchDigest, language).is_empty());
        }
        Ok(())
    }

    #[rstest]
    #[case(Language::De, EmailLanguage::De)]
    #[case(Language::En, EmailLanguage::En)]
    #[case(Language::Fr, EmailLanguage::Fr)]
    #[case(Language::Es, EmailLanguage::Es)]
    #[case(Language::It, EmailLanguage::It)]
    #[case(Language::Zh, EmailLanguage::En)]
    #[case(Language::Pt, EmailLanguage::En)]
    #[case(Language::Pl, EmailLanguage::En)]
    #[case(Language::Tr, EmailLanguage::En)]
    #[case(Language::Nl, EmailLanguage::En)]
    #[case(Language::Cs, EmailLanguage::En)]
    #[case(Language::Ja, EmailLanguage::En)]
    #[case(Language::Ru, EmailLanguage::En)]
    #[case(Language::Ar, EmailLanguage::En)]
    fn should_resolve_profile_language_to_deployed_email_language(
        #[case] profile_language: Language,
        #[case] expected_email_language: EmailLanguage,
    ) {
        assert_eq!(
            expected_email_language,
            EmailLanguage::resolve(profile_language)
        );
    }

    #[test]
    fn should_use_english_template_key_for_ingestion_only_language() {
        let key = s3_template_key(
            "test",
            "commit",
            EmailTemplateType::WatchlistUpdateAvailability,
            EmailLanguage::resolve(Language::Zh),
        );

        assert_eq!(
            "test/commit/mjml/watchlist/product-update/availability/en.html",
            key
        );
    }

    #[test]
    fn should_publish_workflow_templates_at_runtime_s3_keys()
    -> Result<(), Box<dyn std::error::Error>> {
        const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
        let workflow = include_str!("../../../.github/workflows/deploy.yml");
        let publisher = workflow
            .split_once("\n  aws-push-mail-templates:\n")
            .expect("mail publisher job exists")
            .1
            .split_once("\n  aws-cdk-deploy:\n")
            .expect("mail publisher job ends before CDK deploy")
            .0;
        let script = publisher
            .split_once("      - name: Compile and upload missing templates\n")
            .expect("mail publisher step exists")
            .1
            .split_once("        run: |\n")
            .expect("mail publisher bash body exists")
            .1
            .lines()
            .take_while(|line| line.is_empty() || line.starts_with("          "))
            .map(|line| line.strip_prefix("          ").unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!script.is_empty());

        for case in ["missing", "existing", "mixed", "probe_error"] {
            struct Fixture(PathBuf);
            impl Drop for Fixture {
                fn drop(&mut self) {
                    let _ = fs::remove_dir_all(&self.0);
                }
            }
            let fixture = Fixture(std::env::temp_dir().join(format!(
                "notification-email-publisher-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
            )));
            fs::create_dir(&fixture.0)?;
            let bin = fixture.0.join("bin");
            fs::create_dir(&bin)?;
            let ci = fixture.0.join("ci");
            fs::create_dir(&ci)?;
            fs::write(
                ci.join("s3-artifact-exists.sh"),
                include_str!("../../../ci/s3-artifact-exists.sh"),
            )?;
            let mjml = fixture.0.join("mjml/node_modules/.bin/mjml");
            fs::create_dir_all(mjml.parent().expect("MJML bin has parent"))?;
            fs::write(
                &mjml,
                "#!/usr/bin/env bash\n[[ $# == 3 && $2 == -o ]] || exit 1\nprintf '%s\\n' \"$1\" >> \"$MOCK_COMPILES\"\nprintf '<html/>' > \"$3\"\n",
            )?;
            fs::set_permissions(&mjml, fs::Permissions::from_mode(0o755))?;
            let aws = bin.join("aws");
            fs::write(
                &aws,
                r#"#!/usr/bin/env bash
set -euo pipefail
[[ $# -ge 1 && $1 == s3api ]] || exit 2
shift
operation="$1"
shift
case "$operation" in
  head-object)
    [[ $# == 4 && $1 == --bucket && $2 == mail && $3 == --key ]] || exit 2
    key="$4"
    grep -Fxq -- "$key" "$MOCK_KEYS" || exit 2
    printf '%s\n' "$key" >> "$MOCK_PROBES"
    if [[ $MOCK_PROBE_ERROR == true ]]; then
      echo 'An error occurred (AccessDenied) when calling the HeadObject operation: Forbidden' >&2
      exit 1
    fi
    if grep -Fxq -- "$key" "$MOCK_EXISTING"; then
      exit 0
    fi
    echo 'An error occurred (404) when calling the HeadObject operation: Not Found' >&2
    exit 1
    ;;
  put-object)
    [[ $# == 10 && $1 == --bucket && $2 == mail && $3 == --key && $5 == --body && $7 == --if-none-match && $8 == '*' && $9 == --tagging && ${10} == stage=dev ]] || exit 2
    key="$4"
    body="$6"
    grep -Fxq -- "$key" "$MOCK_KEYS" || exit 2
    [[ $body == "dist/emails/${key#dev/$DEPLOY_COMMIT_SHA/}" && -s $body ]] || exit 2
    printf 's3api put-object --bucket mail --key %s --body %s --if-none-match * --tagging stage=dev\n' "$key" "$body" >> "$MOCK_UPLOADS"
    ;;
  *) exit 2 ;;
esac
"#,
            )?;
            fs::set_permissions(&aws, fs::Permissions::from_mode(0o755))?;

            let families = [
                (
                    EmailTemplateType::WatchlistUpdatePrice,
                    "watchlist/product-update/price",
                ),
                (
                    EmailTemplateType::WatchlistUpdateAvailability,
                    "watchlist/product-update/availability",
                ),
                (EmailTemplateType::SearchFilterMatch, "search-filter/match"),
                (EmailTemplateType::SavedSearchDigest, "search-filter/digest"),
                (
                    EmailTemplateType::PartnershipApplicationApproval,
                    "partnership-application/approval",
                ),
                (
                    EmailTemplateType::PartnershipApplicationRejection,
                    "partnership-application/rejection",
                ),
            ];
            let languages = [
                (EmailLanguage::De, "de"),
                (EmailLanguage::En, "en"),
                (EmailLanguage::Fr, "fr"),
                (EmailLanguage::Es, "es"),
                (EmailLanguage::It, "it"),
            ];
            let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../mjml");
            let mut expected = BTreeSet::new();
            let mut missing = BTreeSet::new();
            let mut expected_compiles = BTreeSet::new();
            let mut all_keys = String::new();
            let mut existing_keys = String::new();
            for (family_index, (template_type, family)) in families.into_iter().enumerate() {
                for (language_index, (language, code)) in languages.into_iter().enumerate() {
                    let source = format!("mjml/{family}/{code}.mjml");
                    let output = format!("dist/emails/mjml/{family}/{code}.html");
                    let key = format!("dev/{SHA}/mjml/{family}/{code}.html");
                    assert_eq!(key, s3_template_key("dev", SHA, template_type, language));
                    let target = fixture.0.join(&source);
                    fs::create_dir_all(target.parent().expect("template has parent"))?;
                    fs::copy(
                        source_root.join(family).join(format!("{code}.mjml")),
                        target,
                    )?;
                    all_keys.push_str(&format!("{key}\n"));
                    let is_existing = case == "existing"
                        || (case == "mixed"
                            && (family_index * languages.len() + language_index).is_multiple_of(2));
                    if is_existing {
                        existing_keys.push_str(&format!("{key}\n"));
                    } else if case != "probe_error" {
                        missing.insert(format!(
                            "s3api put-object --bucket mail --key {key} --body {output} --if-none-match * --tagging stage=dev"
                        ));
                        expected_compiles.insert(source);
                    }
                    expected.insert(key);
                }
            }
            assert_eq!(expected.len(), 30);
            fs::write(fixture.0.join("keys"), all_keys)?;
            fs::write(fixture.0.join("existing"), existing_keys)?;
            let git_init = Command::new("git")
                .args(["init", "-q"])
                .current_dir(&fixture.0)
                .output()?;
            assert!(
                git_init.status.success(),
                "git init: {}",
                String::from_utf8_lossy(&git_init.stderr)
            );
            let git_add = Command::new("git")
                .args(["add", "--", "mjml"])
                .current_dir(&fixture.0)
                .output()?;
            assert!(
                git_add.status.success(),
                "git add: {}",
                String::from_utf8_lossy(&git_add.stderr)
            );

            let uploads = fixture.0.join("uploads");
            let probes = fixture.0.join("probes");
            let compiles = fixture.0.join("compiles");
            let result = Command::new("bash")
                .args(["-c", &script])
                .current_dir(&fixture.0)
                .env("STAGE", "dev")
                .env("DEPLOY_COMMIT_SHA", SHA)
                .env("BUCKET", "mail")
                .env("MOCK_UPLOADS", &uploads)
                .env("MOCK_PROBES", &probes)
                .env("MOCK_COMPILES", &compiles)
                .env("MOCK_KEYS", fixture.0.join("keys"))
                .env("MOCK_EXISTING", fixture.0.join("existing"))
                .env("MOCK_PROBE_ERROR", (case == "probe_error").to_string())
                .env(
                    "PATH",
                    format!("{}:{}", bin.display(), std::env::var("PATH")?),
                )
                .output()?;
            assert_eq!(
                result.status.success(),
                case != "probe_error",
                "{case} publisher: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            if case == "probe_error" {
                assert!(String::from_utf8_lossy(&result.stderr).contains("AccessDenied"));
            }
            let read_log = |path: &PathBuf| -> Result<String, std::io::Error> {
                if path.exists() {
                    fs::read_to_string(path)
                } else {
                    Ok(String::new())
                }
            };
            let actual = read_log(&uploads)?;
            let published: BTreeSet<_> = actual.lines().collect();
            assert_eq!(
                actual.lines().count(),
                missing.len(),
                "{case} uploads: {actual}"
            );
            assert_eq!(
                published,
                missing.iter().map(String::as_str).collect(),
                "{case}"
            );
            let probed = read_log(&probes)?;
            assert_eq!(
                probed.lines().count(),
                if case == "probe_error" { 1 } else { 30 },
                "{case} probes: {probed}"
            );
            if case != "probe_error" {
                assert_eq!(
                    probed.lines().collect::<BTreeSet<_>>(),
                    expected.iter().map(String::as_str).collect(),
                    "{case} probe keys"
                );
            }
            let compiled = read_log(&compiles)?;
            assert_eq!(
                compiled.lines().count(),
                expected_compiles.len(),
                "{case} compiles: {compiled}"
            );
            assert_eq!(
                compiled.lines().collect::<BTreeSet<_>>(),
                expected_compiles.iter().map(String::as_str).collect(),
                "{case} compile sources"
            );
            assert!(
                fixture
                    .0
                    .join("mjml/watchlist/product-update/price/en.mjml")
                    .is_file()
            );
            for key in &expected {
                let relative = key
                    .strip_prefix(&format!("dev/{SHA}/"))
                    .expect("stage/SHA prefix");
                let source = format!("{}.mjml", relative.strip_suffix(".html").expect("HTML key"));
                assert_eq!(
                    fixture.0.join(format!("dist/emails/{relative}")).is_file(),
                    expected_compiles.contains(&source),
                    "{case} generated HTML for {key}"
                );
            }
        }
        Ok(())
    }

    #[rstest]
    #[case(
        EmailTemplateType::WatchlistUpdatePrice,
        "Der Preis auf deiner Merkliste hat sich geändert",
        "Your watchlist price changed",
        "Le prix de votre liste de souhaits a changé",
        "El precio de tu lista de deseos ha cambiado",
        "Il prezzo della tua lista dei desideri è cambiato"
    )]
    #[case(
        EmailTemplateType::WatchlistUpdateAvailability,
        "Die Verfügbarkeit eines Artikels auf deiner Merkliste hat sich geändert",
        "Your watchlist item's availability changed",
        "La disponibilité d’un article de votre liste de souhaits a changé",
        "La disponibilidad de un artículo de tu lista de deseos ha cambiado",
        "La disponibilità di un articolo nella tua lista dei desideri è cambiata"
    )]
    #[case(
        EmailTemplateType::SearchFilterMatch,
        "Neuer Treffer für deinen Suchfilter",
        "New search filter match",
        "Nouveau résultat pour votre filtre de recherche",
        "Nuevo resultado para tu filtro de búsqueda",
        "Nuovo risultato per il tuo filtro di ricerca"
    )]
    #[case(
        EmailTemplateType::PartnershipApplicationApproval,
        "Partnerschaftsantrag genehmigt",
        "Partnership application approved",
        "Demande de partenariat approuvée",
        "Solicitud de asociación aprobada",
        "Richiesta di partnership approvata"
    )]
    #[case(
        EmailTemplateType::PartnershipApplicationRejection,
        "Update zu deinem Partnerschaftsantrag",
        "Partnership application update",
        "Mise à jour de votre demande de partenariat",
        "Actualización de tu solicitud de asociación",
        "Aggiornamento della tua richiesta di partnership"
    )]
    fn should_localize_subject_for_every_email_template_and_email_language(
        #[case] template: EmailTemplateType,
        #[case] expected_de: &str,
        #[case] expected_en: &str,
        #[case] expected_fr: &str,
        #[case] expected_es: &str,
        #[case] expected_it: &str,
    ) {
        assert_eq!(expected_de, subject(template, EmailLanguage::De));
        assert_eq!(expected_en, subject(template, EmailLanguage::En));
        assert_eq!(expected_fr, subject(template, EmailLanguage::Fr));
        assert_eq!(expected_es, subject(template, EmailLanguage::Es));
        assert_eq!(expected_it, subject(template, EmailLanguage::It));
    }

    #[rstest]
    #[case(Language::En, "Available", "In stock")]
    #[case(Language::De, "Verfügbar", "Auf Lager")]
    fn should_localize_watchlist_availability_change_for_recipient_language(
        #[case] language: Language,
        #[case] expected_old_availability: &str,
        #[case] expected_new_availability: &str,
    ) {
        assert_eq!(
            expected_old_availability,
            availability_text(ListingAvailability::Available, language)
        );
        assert_eq!(
            expected_new_availability,
            availability_text(ListingAvailability::InStock, language)
        );
    }

    #[rstest]
    #[case(
        Some(ContentPolicyDecision::Allowed),
        false,
        Some("https://shop.example/image.jpg")
    )]
    #[case(
        Some(ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany)),
        false,
        None
    )]
    #[case(
        Some(ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany)),
        true,
        Some("https://shop.example/image.jpg")
    )]
    #[case(None, false, None)]
    fn should_filter_image_url_in_email_template_data(
        #[case] content_policy: Option<ContentPolicyDecision>,
        #[case] consent: bool,
        #[case] expected_image_url: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let source = source(consent, content_policy)?;
        let data = template_data(&source, EmailLanguage::En, None, links(EmailLanguage::En));

        assert_eq!(expected_image_url, data["image_url"].as_str());
        Ok(())
    }

    #[test]
    fn should_include_frontend_product_listing_url_and_first_name_in_template_data()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = source(false, None)?;
        let data = template_data(
            &source,
            EmailLanguage::En,
            Some("Ada"),
            links(EmailLanguage::En),
        );

        assert_eq!(
            Some("https://stage.aura-historia.com/en/products/antique-vase-a1b2c3"),
            data["product_listing_url"].as_str()
        );
        assert_eq!(
            Some("https://stage.aura-historia.com/en/me/watchlist"),
            data["watchlist_url"].as_str()
        );
        assert_eq!(
            Some("https://stage.aura-historia.com/en"),
            data["home_url"].as_str()
        );
        assert_eq!(Some("Ada"), data["first_name"].as_str());
        assert!(data.get("listing_source_slug_id").is_none());
        assert!(data.get("product_listing_title_slug_id").is_none());
        Ok(())
    }

    #[test]
    fn should_localize_template_data_with_resolved_email_language()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        source.presentation_preferences.language = Language::Zh;
        let email_language = EmailLanguage::resolve(source.presentation_preferences.language);
        let data = template_data(&source, email_language, None, links(email_language));

        assert_eq!(Some("Available"), data["old_availability"].as_str());
        assert_eq!(Some("In stock"), data["new_availability"].as_str());
        Ok(())
    }

    #[test]
    fn should_render_null_for_absent_watchlist_availability()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        let NotificationContent::Watchlist { change, .. } = &mut source.content else {
            return Err(
                std::io::Error::other("test source is not a watchlist notification").into(),
            );
        };
        *change = NotificationWatchlistChange::AvailabilityChange {
            old_availability: None,
            new_availability: Some(ListingAvailability::InStock),
        };

        let data = template_data(&source, EmailLanguage::En, None, links(EmailLanguage::En));

        assert!(data["old_availability"].is_null());
        assert_eq!(Some("In stock"), data["new_availability"].as_str());
        assert_eq!(
            Some("availability_change"),
            data["notification_type"].as_str()
        );
        Ok(())
    }

    #[test]
    fn should_localize_notification_title_with_resolved_email_language()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        source.presentation_preferences.language = Language::Zh;
        let NotificationContent::Watchlist { snapshot, .. } = &mut source.content else {
            return Err(
                std::io::Error::other("test source is not a watchlist notification").into(),
            );
        };
        snapshot.title = Some(HashMap::from([
            (
                Language::En,
                serde_json::from_value(json!("English title"))?,
            ),
            (
                Language::Zh,
                serde_json::from_value(json!("Chinese title"))?,
            ),
        ]));

        let email_language = EmailLanguage::resolve(source.presentation_preferences.language);
        let data = template_data(&source, email_language, None, links(email_language));

        assert_eq!(Some("English title"), data["title"].as_str());
        Ok(())
    }

    #[rstest]
    #[case(Currency::Eur, "10,00 €", "9,00 €")]
    #[case(Currency::Usd, "$10.00", "$9.00")]
    fn should_format_watchlist_prices_in_their_source_currency(
        #[case] currency: Currency,
        #[case] expected_old_price: &str,
        #[case] expected_new_price: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        let NotificationContent::Watchlist { change, .. } = &mut source.content else {
            return Err(
                std::io::Error::other("test source is not a watchlist notification").into(),
            );
        };
        *change = NotificationWatchlistChange::PriceChange {
            old_price: Some(Price::new(MonetaryAmount::from(1000_u64), currency).into()),
            new_price: Some(Price::new(MonetaryAmount::from(900_u64), currency).into()),
        };

        let data = template_data(&source, EmailLanguage::En, None, links(EmailLanguage::En));

        assert_eq!(Some(expected_old_price), data["old_price"].as_str());
        assert_eq!(Some(expected_new_price), data["new_price"].as_str());
        Ok(())
    }

    #[rstest]
    #[case(EmailLanguage::De, "Preis auf Anfrage")]
    #[case(EmailLanguage::En, "Price on request")]
    #[case(EmailLanguage::Fr, "Prix sur demande")]
    #[case(EmailLanguage::Es, "Precio a consultar")]
    #[case(EmailLanguage::It, "Prezzo su richiesta")]
    fn should_localize_on_request_watchlist_prices(
        #[case] language: EmailLanguage,
        #[case] expected: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        let NotificationContent::Watchlist { change, .. } = &mut source.content else {
            return Err(
                std::io::Error::other("test source is not a watchlist notification").into(),
            );
        };
        *change = NotificationWatchlistChange::PriceChange {
            old_price: None,
            new_price: Some(ProductListingPrice::OnRequest),
        };

        let data = template_data(&source, language, None, links(language));

        assert!(data["old_price"].is_null());
        assert_eq!(Some(expected), data["new_price"].as_str());
        Ok(())
    }

    #[test]
    fn should_distinguish_zero_and_absent_watchlist_prices()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        let NotificationContent::Watchlist { change, .. } = &mut source.content else {
            return Err(
                std::io::Error::other("test source is not a watchlist notification").into(),
            );
        };
        *change = NotificationWatchlistChange::PriceChange {
            old_price: None,
            new_price: Some(Price::new(MonetaryAmount::from(0_u64), Currency::Eur).into()),
        };

        let data = template_data(&source, EmailLanguage::En, None, links(EmailLanguage::En));

        assert!(data["old_price"].is_null());
        assert_eq!(Some("0,00 €"), data["new_price"].as_str());
        Ok(())
    }

    fn links(language: EmailLanguage) -> FrontendLinks<'static> {
        FrontendLinks::new("https://stage.aura-historia.com", language)
    }

    #[rstest]
    #[case(
        EmailLanguage::De,
        "https://stage.aura-historia.com/de/partners/applications"
    )]
    #[case(
        EmailLanguage::En,
        "https://stage.aura-historia.com/en/partners/applications"
    )]
    #[case(
        EmailLanguage::Fr,
        "https://stage.aura-historia.com/fr/partners/applications"
    )]
    #[case(
        EmailLanguage::Es,
        "https://stage.aura-historia.com/es/partners/applications"
    )]
    #[case(
        EmailLanguage::It,
        "https://stage.aura-historia.com/it/partners/applications"
    )]
    fn should_link_partnership_decisions_to_localized_partner_applications(
        #[case] language: EmailLanguage,
        #[case] expected: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        source.content = NotificationContent::PartnershipApplication {
            partnership_application_id:
                partnership_core::partnership_application_id::PartnershipApplicationId::new(),
            snapshot: notification_core::notification::PartnershipApplicationNotificationSnapshot {
                party_name: party_core::party_name::PartyName::try_from("Antik und Stil")?,
                listing_source_name: ListingSourceName::try_from("antik-und-stil.de")?,
                image: None,
            },
            decision: PartnershipApplicationDecision::Approved,
        };

        let data = template_data(&source, language, None, links(language));

        assert_eq!(Some(expected), data["partner_dashboard_url"].as_str());
        Ok(())
    }

    #[test]
    fn should_link_search_filter_matches_to_the_localized_search_filter()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        let NotificationContent::Watchlist { snapshot, .. } = source.content.clone() else {
            panic!("fixture")
        };
        let filter_id = search_filter_core::user_search_filter_id::UserSearchFilterId::new();
        source.content = NotificationContent::SearchFilter {
            origin_event_id: EventId::new(),
            product_listing_id: ProductListingId::new(),
            user_search_filter_id: filter_id,
            snapshot,
            user_search_filter_name: "Art Nouveau".into(),
        };

        let data = template_data(&source, EmailLanguage::De, None, links(EmailLanguage::De));

        assert_eq!(
            Some(
                format!("https://stage.aura-historia.com/de/me/search-filter/{filter_id}").as_str()
            ),
            data["search_filter_url"].as_str()
        );
        assert_eq!(
            Some("https://stage.aura-historia.com/de/products/antique-vase-a1b2c3"),
            data["product_listing_url"].as_str()
        );
        Ok(())
    }

    fn source(
        show_unassessed_or_sensitive_content: bool,
        content_policy: Option<ContentPolicyDecision>,
    ) -> Result<NotificationDeliverySource, Box<dyn std::error::Error>> {
        Ok(NotificationDeliverySource {
            saved_search_digest: None,
            notification_delivery_id: NotificationDeliveryId::new(),
            source_id: notification_service::ports::notification_delivery_repository::NotificationDeliverySourceId::Notification(NotificationId::new()),
            user_id: UserId::new(),
            channel: NotificationDeliveryChannel::Email,
            target_key: NotificationDeliveryTargetKey::primary(),
            content: NotificationContent::Watchlist {
                origin_event_id: EventId::new(),
                product_listing_id: ProductListingId::new(),
                snapshot: ProductListingNotificationSnapshot {
                    listing_source_id: ListingSourceId::new(),
                    source_listing_id: SourceListingId::try_from("source-product")
                        .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
                    listing_source_slug_id: ListingSourceSlugId::raw("test-source")
                        .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
                    product_listing_title_slug_id: ProductListingSlugId::raw("antique-vase-a1b2c3")
                        .unwrap_or_else(|error| {
                            panic!("valid product listing title slug: {error}")
                        }),
                    listing_source_name: ListingSourceName::try_from("Test Listing Source")
                        .unwrap_or_else(|error| {
                            panic!("invalid test listing source name: {error}")
                        }),
                    title: None,
                    image: Some(Url::parse("https://shop.example/image.jpg")?),
                    content_policy,
                    url: Url::parse("https://shop.example/product")?,
                    view_url: Url::parse("https://aura-historia.example/product")?,
                },
                change: NotificationWatchlistChange::AvailabilityChange {
                    old_availability: Some(ListingAvailability::Available),
                    new_availability: Some(ListingAvailability::InStock),
                },
            },
            presentation_preferences: NotificationPresentationPreferences {
                language: Language::En,
                show_unassessed_or_sensitive_content,
            },
        })
    }
}
