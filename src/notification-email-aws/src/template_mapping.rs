use localization::Language;
use money::Price;
use notification_core::{
    notification::{
        LocalizedNotificationContent, LocalizedNotificationWatchlistChange, NotificationContent,
        NotificationWatchlistChange, PartnerApplicationDecision,
    },
    presentation::present_image,
};
use notification_service::ports::notification_delivery_repository::NotificationDeliverySource;
use product_listing_core::listing_availability::ListingAvailability;
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
    WatchlistUpdateState,
    SearchFilterMatch,
    PartnerApplicationApproval,
    PartnerApplicationRejection,
}

pub(crate) const fn template_type(content: &NotificationContent) -> EmailTemplateType {
    match content {
        NotificationContent::Watchlist {
            change: NotificationWatchlistChange::PriceChange { .. },
            ..
        } => EmailTemplateType::WatchlistUpdatePrice,
        NotificationContent::Watchlist { .. } => EmailTemplateType::WatchlistUpdateState,
        NotificationContent::SearchFilter { .. } => EmailTemplateType::SearchFilterMatch,
        NotificationContent::PartnerApplication {
            decision: PartnerApplicationDecision::Approved,
            ..
        } => EmailTemplateType::PartnerApplicationApproval,
        NotificationContent::PartnerApplication { .. } => {
            EmailTemplateType::PartnerApplicationRejection
        }
    }
}

pub(crate) const fn template_directory(template_type: EmailTemplateType) -> &'static str {
    match template_type {
        EmailTemplateType::WatchlistUpdatePrice => "mjml/watchlist/product-update/price",
        EmailTemplateType::WatchlistUpdateState => "mjml/watchlist/product-update/state",
        EmailTemplateType::SearchFilterMatch => "mjml/search-filter/match",
        EmailTemplateType::PartnerApplicationApproval => "mjml/partner-application/approval",
        EmailTemplateType::PartnerApplicationRejection => "mjml/partner-application/rejection",
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
        EmailTemplateType::WatchlistUpdateState => "WATCHLIST_UPDATE_STATE",
        EmailTemplateType::SearchFilterMatch => "SEARCH_FILTER_MATCH",
        EmailTemplateType::PartnerApplicationApproval => "PARTNER_APPLICATION_APPROVAL",
        EmailTemplateType::PartnerApplicationRejection => "PARTNER_APPLICATION_REJECTION",
    }
}

pub(crate) const fn subject(
    template_type: EmailTemplateType,
    language: EmailLanguage,
) -> &'static str {
    match language {
        EmailLanguage::De => match template_type {
            EmailTemplateType::WatchlistUpdatePrice => {
                "Der Preis auf deiner Merkliste hat sich geändert"
            }
            EmailTemplateType::WatchlistUpdateState => {
                "Ein Artikel auf deiner Merkliste hat sich geändert"
            }
            EmailTemplateType::SearchFilterMatch => "Neuer Treffer für deinen Suchfilter",
            EmailTemplateType::PartnerApplicationApproval => "Partnerantrag genehmigt",
            EmailTemplateType::PartnerApplicationRejection => "Update zu deinem Partnerantrag",
        },
        EmailLanguage::Fr => match template_type {
            EmailTemplateType::WatchlistUpdatePrice => {
                "Le prix de votre liste de souhaits a changé"
            }
            EmailTemplateType::WatchlistUpdateState => {
                "Un article de votre liste de souhaits a changé"
            }
            EmailTemplateType::SearchFilterMatch => {
                "Nouveau résultat pour votre filtre de recherche"
            }
            EmailTemplateType::PartnerApplicationApproval => "Demande de partenariat approuvée",
            EmailTemplateType::PartnerApplicationRejection => {
                "Mise à jour de votre demande de partenariat"
            }
        },
        EmailLanguage::Es => match template_type {
            EmailTemplateType::WatchlistUpdatePrice => {
                "El precio de tu lista de deseos ha cambiado"
            }
            EmailTemplateType::WatchlistUpdateState => {
                "Un artículo de tu lista de deseos ha cambiado"
            }
            EmailTemplateType::SearchFilterMatch => "Nuevo resultado para tu filtro de búsqueda",
            EmailTemplateType::PartnerApplicationApproval => "Solicitud de asociación aprobada",
            EmailTemplateType::PartnerApplicationRejection => {
                "Actualización de tu solicitud de asociación"
            }
        },
        EmailLanguage::It => match template_type {
            EmailTemplateType::WatchlistUpdatePrice => {
                "Il prezzo della tua lista dei desideri è cambiato"
            }
            EmailTemplateType::WatchlistUpdateState => {
                "Un articolo nella tua lista dei desideri è cambiato"
            }
            EmailTemplateType::SearchFilterMatch => "Nuovo risultato per il tuo filtro di ricerca",
            EmailTemplateType::PartnerApplicationApproval => "Richiesta di partnership approvata",
            EmailTemplateType::PartnerApplicationRejection => {
                "Aggiornamento della tua richiesta di partnership"
            }
        },
        EmailLanguage::En => match template_type {
            EmailTemplateType::WatchlistUpdatePrice => "Your watchlist price changed",
            EmailTemplateType::WatchlistUpdateState => "Your watchlist item changed",
            EmailTemplateType::SearchFilterMatch => "New search filter match",
            EmailTemplateType::PartnerApplicationApproval => "Partner application approved",
            EmailTemplateType::PartnerApplicationRejection => "Partner application update",
        },
    }
}

pub(crate) fn template_data(
    source: &NotificationDeliverySource,
    email_language: EmailLanguage,
    first_name: Option<&str>,
) -> Value {
    let localized = source
        .content
        .clone()
        .localized(&[email_language.as_language()]);
    let present_image_url = |image| {
        present_image(
            image,
            source.presentation_preferences.prohibited_content_consent,
        )
        .and_then(|image| image.url)
        .map(|url| url.to_string())
    };
    match localized {
        LocalizedNotificationContent::Watchlist {
            snapshot, change, ..
        } => {
            let mut data = product_template_data(
                snapshot.shop_name.to_string(),
                snapshot.shop_slug_id.to_string(),
                snapshot.product_listing_slug_id.to_string(),
                snapshot.title.map(|title| title.payload.to_string()),
                present_image_url(snapshot.image),
                snapshot.view_url.to_string(),
            );
            match change {
                LocalizedNotificationWatchlistChange::PriceChange {
                    old_price,
                    new_price,
                } => {
                    data["old_price"] = json!(price_text(old_price));
                    data["new_price"] = json!(price_text(new_price));
                    data["notification_type"] = json!("price_change");
                }
                LocalizedNotificationWatchlistChange::AvailabilityChange {
                    old_availability,
                    new_availability,
                } => {
                    data["old_state"] = json!(availability_text(
                        old_availability,
                        email_language.as_language()
                    ));
                    data["new_state"] = json!(availability_text(
                        new_availability,
                        email_language.as_language()
                    ));
                    data["notification_type"] = json!("state_change");
                }
            }
            add_recipient_data(data, first_name)
        }
        LocalizedNotificationContent::SearchFilter {
            snapshot,
            user_search_filter_id,
            user_search_filter_name,
            ..
        } => {
            let mut data = product_template_data(
                snapshot.shop_name.to_string(),
                snapshot.shop_slug_id.to_string(),
                snapshot.product_listing_slug_id.to_string(),
                snapshot.title.map(|title| title.payload.to_string()),
                present_image_url(snapshot.image),
                snapshot.view_url.to_string(),
            );
            data["search_filter_id"] = json!(user_search_filter_id.to_string());
            data["search_filter_name"] = json!(user_search_filter_name.to_string());
            data["notification_type"] = json!("search_filter_match");
            add_recipient_data(data, first_name)
        }
        LocalizedNotificationContent::PartnerApplication {
            snapshot, decision, ..
        } => add_recipient_data(
            json!({
                "shop_name": snapshot.shop_name.to_string(),
                "image_url": snapshot.image.map(|image| image.to_string()),
                "notification_type": match decision {
                    PartnerApplicationDecision::Approved => "partner_application_approval",
                    PartnerApplicationDecision::Rejected => "partner_application_rejection",
                },
            }),
            first_name,
        ),
    }
}

fn add_recipient_data(mut data: Value, first_name: Option<&str>) -> Value {
    data["first_name"] = json!(first_name);
    data
}
fn product_template_data(
    shop_name: String,
    shop_slug_id: String,
    product_listing_slug_id: String,
    title: Option<String>,
    image_url: Option<String>,
    view_url: String,
) -> Value {
    json!({ "shop_name": shop_name, "shop_slug_id": shop_slug_id, "product_listing_slug_id": product_listing_slug_id, "title": title, "image_url": image_url, "view_url": view_url })
}
fn price_text(price: Option<Price>) -> Option<String> {
    price.map(|price| price.format_human_readable())
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
        listing_availability::ListingAvailability, product_listing_id::ProductListingId,
        product_listing_slug_id::ProductListingSlugId, shop_listing_id::ShopListingId,
    };
    use rstest::rstest;
    use shop_core::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
    use std::collections::HashMap;
    use url::Url;
    use user_core::user_id::UserId;

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
            EmailTemplateType::WatchlistUpdateState,
            EmailLanguage::resolve(Language::Zh),
        );

        assert_eq!(
            "test/commit/mjml/watchlist/product-update/state/en.html",
            key
        );
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
        EmailTemplateType::WatchlistUpdateState,
        "Ein Artikel auf deiner Merkliste hat sich geändert",
        "Your watchlist item changed",
        "Un article de votre liste de souhaits a changé",
        "Un artículo de tu lista de deseos ha cambiado",
        "Un articolo nella tua lista dei desideri è cambiato"
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
        EmailTemplateType::PartnerApplicationApproval,
        "Partnerantrag genehmigt",
        "Partner application approved",
        "Demande de partenariat approuvée",
        "Solicitud de asociación aprobada",
        "Richiesta di partnership approvata"
    )]
    #[case(
        EmailTemplateType::PartnerApplicationRejection,
        "Update zu deinem Partnerantrag",
        "Partner application update",
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
    #[case(Some("None"), false, Some("https://shop.example/image.jpg"))]
    #[case(Some("NaziGermany"), false, None)]
    #[case(Some("NaziGermany"), true, Some("https://shop.example/image.jpg"))]
    #[case(None, false, None)]
    fn should_filter_image_url_in_email_template_data(
        #[case] prohibited_content: Option<&str>,
        #[case] consent: bool,
        #[case] expected_image_url: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let source = source(consent, prohibited_content)?;
        let data = template_data(&source, EmailLanguage::En, None);

        assert_eq!(expected_image_url, data["image_url"].as_str());
        Ok(())
    }

    #[test]
    fn should_localize_template_data_with_resolved_email_language()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        source.presentation_preferences.language = Language::Zh;
        let email_language = EmailLanguage::resolve(source.presentation_preferences.language);
        let data = template_data(&source, email_language, None);

        assert_eq!(Some("Available"), data["old_state"].as_str());
        assert_eq!(Some("In stock"), data["new_state"].as_str());
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
        let data = template_data(&source, email_language, None);

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
            old_price: Some(Price::new(MonetaryAmount::from(1000_u64), currency)),
            new_price: Some(Price::new(MonetaryAmount::from(900_u64), currency)),
        };

        let data = template_data(&source, EmailLanguage::En, None);

        assert_eq!(Some(expected_old_price), data["old_price"].as_str());
        assert_eq!(Some(expected_new_price), data["new_price"].as_str());
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
            new_price: Some(Price::new(MonetaryAmount::from(0_u64), Currency::Eur)),
        };

        let data = template_data(&source, EmailLanguage::En, None);

        assert!(data["old_price"].is_null());
        assert_eq!(Some("0,00 €"), data["new_price"].as_str());
        Ok(())
    }

    fn source(
        prohibited_content_consent: bool,
        prohibited_content: Option<&str>,
    ) -> Result<NotificationDeliverySource, Box<dyn std::error::Error>> {
        Ok(NotificationDeliverySource {
            notification_delivery_id: NotificationDeliveryId::new(),
            notification_id: NotificationId::new(),
            user_id: UserId::new(),
            channel: NotificationDeliveryChannel::Email,
            target_key: NotificationDeliveryTargetKey::primary(),
            content: NotificationContent::Watchlist {
                origin_event_id: EventId::new(),
                product_listing_id: ProductListingId::new(),
                snapshot: ProductListingNotificationSnapshot {
                    shop_id: ShopId::new(),
                    shop_listing_id: ShopListingId::from("shop-product"),
                    shop_slug_id: ShopSlugId::from("shop"),
                    product_listing_slug_id: ProductListingSlugId::from("product"),
                    shop_name: ShopName::from("Test Shop"),
                    title: None,
                    image: prohibited_content
                        .map(|prohibited_content| {
                            serde_json::from_value(serde_json::json!({
                                "url": "https://shop.example/image.jpg",
                                "prohibited_content": prohibited_content,
                            }))
                        })
                        .transpose()?,
                    url: Url::parse("https://shop.example/product")?,
                    view_url: Url::parse("https://aura-historia.example/product")?,
                },
                change: NotificationWatchlistChange::AvailabilityChange {
                    old_availability: ListingAvailability::Available,
                    new_availability: ListingAvailability::InStock,
                },
            },
            presentation_preferences: NotificationPresentationPreferences {
                language: Language::En,
                prohibited_content_consent,
            },
        })
    }
}
