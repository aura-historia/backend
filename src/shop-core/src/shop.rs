use crate::{
    address::{GeoAddress, StructuredAddress},
    affiliate_configuration::AffiliateConfiguration,
    lifecycle::ShopLifecycle,
    partner_status::ShopPartnerStatus,
    shop_type::ShopType,
    woocommerce_webhook_secret::WoocommerceWebhookSecret,
};
use crate::{domain::Domain, shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};
use domain_primitives::change_outcome::ChangeOutcome;
use localization::Language;
use money::Currency;
use serde_email::Email;
use std::collections::HashSet;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Shop {
    id: ShopId,
    slug_id: ShopSlugId,
    name: ShopName,
    shop_type: ShopType,
    domains: HashSet<Domain>,
    shopify: Option<ShopifyIntegration>,
    woocommerce: Option<WoocommerceIntegration>,
    presentation: ShopPresentation,
    address: Option<ShopAddress>,
    contact: ShopContact,
    partner_status: ShopPartnerStatus,
    lifecycle: ShopLifecycle,
    affiliate_configuration: Option<AffiliateConfiguration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewShop {
    pub id: ShopId,
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub shopify: Option<ShopifyIntegration>,
    pub woocommerce: Option<WoocommerceIntegration>,
    pub presentation: ShopPresentation,
    pub address: Option<ShopAddress>,
    pub contact: ShopContact,
    pub partner_status: ShopPartnerStatus,
    pub affiliate_configuration: Option<AffiliateConfiguration>,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedShopState {
    pub id: ShopId,
    pub slug_id: ShopSlugId,
    pub name: ShopName,
    pub shop_type: ShopType,
    pub domains: HashSet<Domain>,
    pub shopify: Option<ShopifyIntegration>,
    pub woocommerce: Option<WoocommerceIntegration>,
    pub presentation: ShopPresentation,
    pub address: Option<ShopAddress>,
    pub contact: ShopContact,
    pub partner_status: ShopPartnerStatus,
    pub lifecycle: ShopLifecycle,
    pub affiliate_configuration: Option<AffiliateConfiguration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShopifyIntegration {
    pub domain: Domain,
    pub currency: Option<Currency>,
    pub language: Option<Language>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WoocommerceIntegration {
    pub webhook_secret: Option<WoocommerceWebhookSecret>,
    pub currency: Option<Currency>,
    pub language: Option<Language>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShopPresentation {
    pub url: Option<Url>,
    pub image: Option<Url>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShopAddress {
    pub structured: StructuredAddress,
    pub geo: Option<GeoAddress>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShopContact {
    pub phone: Option<String>,
    pub email: Option<Email>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RehydrateShopError {
    #[error("shop slug does not match shop name")]
    SlugMismatch {
        expected: ShopSlugId,
        actual: ShopSlugId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ShopDiscardError {
    #[error("only drafted shops can be discarded")]
    NotDrafted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ShopPublishError {
    #[error("discarded shops cannot be published")]
    Discarded,
}

impl Shop {
    pub fn create(input: NewShop) -> Self {
        Self::create_with_lifecycle(input, ShopLifecycle::Drafted)
    }

    pub fn create_with_lifecycle(input: NewShop, lifecycle: ShopLifecycle) -> Self {
        Self {
            slug_id: ShopSlugId::from(input.name.as_ref()),
            id: input.id,
            name: input.name,
            shop_type: input.shop_type,
            domains: input.domains,
            shopify: input.shopify,
            woocommerce: input.woocommerce,
            presentation: input.presentation,
            address: input.address,
            contact: input.contact,
            partner_status: input.partner_status,
            lifecycle,
            affiliate_configuration: input.affiliate_configuration,
        }
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn rehydrate(state: RehydratedShopState) -> Result<Self, RehydrateShopError> {
        let expected_slug_id = ShopSlugId::from(state.name.as_ref());
        if expected_slug_id != state.slug_id {
            return Err(RehydrateShopError::SlugMismatch {
                expected: expected_slug_id,
                actual: state.slug_id,
            });
        }

        Ok(Self {
            id: state.id,
            slug_id: state.slug_id,
            name: state.name,
            shop_type: state.shop_type,
            domains: state.domains,
            shopify: state.shopify,
            woocommerce: state.woocommerce,
            presentation: state.presentation,
            address: state.address,
            contact: state.contact,
            partner_status: state.partner_status,
            lifecycle: state.lifecycle,
            affiliate_configuration: state.affiliate_configuration,
        })
    }

    pub fn change_shop_type(&mut self, shop_type: ShopType) -> ChangeOutcome {
        replace_if_changed(&mut self.shop_type, shop_type)
    }

    pub fn change_partner_status(&mut self, partner_status: ShopPartnerStatus) -> ChangeOutcome {
        replace_if_changed(&mut self.partner_status, partner_status)
    }

    pub fn replace_domains(&mut self, domains: HashSet<Domain>) -> ChangeOutcome {
        replace_if_changed(&mut self.domains, domains)
    }

    pub fn replace_shopify_integration(
        &mut self,
        shopify: Option<ShopifyIntegration>,
    ) -> ChangeOutcome {
        replace_if_changed(&mut self.shopify, shopify)
    }

    pub fn replace_woocommerce_integration(
        &mut self,
        woocommerce: Option<WoocommerceIntegration>,
    ) -> ChangeOutcome {
        replace_if_changed(&mut self.woocommerce, woocommerce)
    }

    pub fn replace_presentation(&mut self, presentation: ShopPresentation) -> ChangeOutcome {
        replace_if_changed(&mut self.presentation, presentation)
    }

    pub fn replace_address(&mut self, address: Option<ShopAddress>) -> ChangeOutcome {
        replace_if_changed(&mut self.address, address)
    }

    pub fn replace_contact(&mut self, contact: ShopContact) -> ChangeOutcome {
        replace_if_changed(&mut self.contact, contact)
    }

    pub fn replace_affiliate_configuration(
        &mut self,
        affiliate_configuration: Option<AffiliateConfiguration>,
    ) -> ChangeOutcome {
        replace_if_changed(&mut self.affiliate_configuration, affiliate_configuration)
    }

    pub fn id(&self) -> ShopId {
        self.id
    }

    pub fn slug_id(&self) -> &ShopSlugId {
        &self.slug_id
    }

    pub fn name(&self) -> &ShopName {
        &self.name
    }

    pub fn shop_type(&self) -> ShopType {
        self.shop_type
    }

    pub fn domains(&self) -> &HashSet<Domain> {
        &self.domains
    }

    pub fn shopify(&self) -> Option<&ShopifyIntegration> {
        self.shopify.as_ref()
    }

    pub fn woocommerce(&self) -> Option<&WoocommerceIntegration> {
        self.woocommerce.as_ref()
    }

    pub fn presentation(&self) -> &ShopPresentation {
        &self.presentation
    }

    pub fn view_url(&self) -> Option<Url> {
        self.presentation.url.as_ref().map(|url| {
            self.affiliate_configuration
                .as_ref()
                .map(|configuration| configuration.build_url(url))
                .unwrap_or_else(|| append_utm_params(url.clone()))
        })
    }

    pub fn address(&self) -> Option<&ShopAddress> {
        self.address.as_ref()
    }

    pub fn contact(&self) -> &ShopContact {
        &self.contact
    }

    pub fn partner_status(&self) -> ShopPartnerStatus {
        self.partner_status
    }

    pub fn lifecycle(&self) -> ShopLifecycle {
        self.lifecycle
    }

    pub fn publish(&mut self) -> Result<ChangeOutcome, ShopPublishError> {
        match self.lifecycle {
            ShopLifecycle::Drafted => Ok(replace_if_changed(
                &mut self.lifecycle,
                ShopLifecycle::Published,
            )),
            ShopLifecycle::Published => Ok(ChangeOutcome::Unchanged),
            ShopLifecycle::Discarded => Err(ShopPublishError::Discarded),
        }
    }

    pub fn discard(&mut self) -> Result<ChangeOutcome, ShopDiscardError> {
        if self.lifecycle != ShopLifecycle::Drafted {
            return Err(ShopDiscardError::NotDrafted);
        }

        Ok(replace_if_changed(
            &mut self.lifecycle,
            ShopLifecycle::Discarded,
        ))
    }

    pub fn affiliate_configuration(&self) -> Option<&AffiliateConfiguration> {
        self.affiliate_configuration.as_ref()
    }
}

fn append_utm_params(mut url: Url) -> Url {
    if url.query_pairs().any(|(key, _)| key == "utm_source") {
        return url;
    }
    url.query_pairs_mut()
        .append_pair("utm_source", "aura_historia")
        .append_pair("utm_medium", "referral");
    url
}

fn replace_if_changed<T: PartialEq>(target: &mut T, value: T) -> ChangeOutcome {
    if *target == value {
        ChangeOutcome::Unchanged
    } else {
        *target = value;
        ChangeOutcome::Changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain(value: &str) -> Domain {
        Domain::try_from(value).unwrap()
    }

    fn new_shop(name: &str) -> NewShop {
        NewShop {
            id: ShopId::new(),
            name: ShopName::from(name),
            shop_type: ShopType::CommercialDealer,
            domains: HashSet::from([domain("https://example.com")]),
            shopify: None,
            woocommerce: None,
            presentation: ShopPresentation::default(),
            address: None,
            contact: ShopContact::default(),
            partner_status: ShopPartnerStatus::Scraped,
            affiliate_configuration: None,
        }
    }

    fn shop_state(name: &str, slug_id: &str) -> RehydratedShopState {
        RehydratedShopState {
            id: ShopId::new(),
            slug_id: ShopSlugId::from(slug_id),
            name: ShopName::from(name),
            shop_type: ShopType::CommercialDealer,
            domains: HashSet::from([domain("https://example.com")]),
            shopify: None,
            woocommerce: None,
            presentation: ShopPresentation::default(),
            address: None,
            contact: ShopContact::default(),
            partner_status: ShopPartnerStatus::Scraped,
            lifecycle: ShopLifecycle::Drafted,
            affiliate_configuration: None,
        }
    }

    #[test]
    fn should_create_shop_with_private_state_and_slug() {
        let shop = Shop::create(new_shop("Antik und Stil"));

        assert_eq!("antik-und-stil", shop.slug_id().to_string());
        assert_eq!(ShopPartnerStatus::Scraped, shop.partner_status());
    }

    #[test]
    fn should_create_shop_with_all_input_state() {
        let id = ShopId::new();
        let shopify = ShopifyIntegration {
            domain: domain("shopify.example.com"),
            currency: Some(Currency::Eur),
            language: Some(Language::De),
        };
        let woocommerce = WoocommerceIntegration {
            webhook_secret: Some(WoocommerceWebhookSecret::from("secret")),
            currency: Some(Currency::Usd),
            language: Some(Language::En),
        };
        let presentation = ShopPresentation {
            url: Some(Url::parse("https://example.com").unwrap()),
            image: Some(Url::parse("https://example.com/image.jpg").unwrap()),
        };
        let address = ShopAddress {
            structured: StructuredAddress {
                addressline: Some("Main 1".to_string()),
                addressline_extra: Some("Back house".to_string()),
                locality: Some("Berlin".to_string()),
                region: Some("Berlin".to_string()),
                postal_code: Some("10115".to_string()),
                country: Some(isocountry::CountryCode::DEU),
                continent: Some(crate::continent::Continent::Europe),
            },
            geo: Some(GeoAddress {
                lat: 52.5,
                lon: 13.4,
            }),
        };
        let contact = ShopContact {
            phone: Some("+49 123".to_string()),
            email: Some("shop@example.com".parse().unwrap()),
        };
        let affiliate_configuration = AffiliateConfiguration::Partnerize {
            camref: "1110lF73C".to_string(),
        };
        let input = NewShop {
            id,
            name: ShopName::from("Antik und Stil"),
            shop_type: ShopType::AuctionHouse,
            domains: HashSet::from([domain("https://example.com")]),
            shopify: Some(shopify.clone()),
            woocommerce: Some(woocommerce.clone()),
            presentation: presentation.clone(),
            address: Some(address.clone()),
            contact: contact.clone(),
            partner_status: ShopPartnerStatus::Partnered,
            affiliate_configuration: Some(affiliate_configuration.clone()),
        };

        let shop = Shop::create(input);

        assert_eq!(id, shop.id());
        assert_eq!("antik-und-stil", shop.slug_id().to_string());
        assert_eq!("Antik und Stil", shop.name().as_ref());
        assert_eq!(ShopType::AuctionHouse, shop.shop_type());
        assert!(shop.domains().contains(&domain("https://example.com")));
        assert_eq!(Some(&shopify), shop.shopify());
        assert_eq!(Some(&woocommerce), shop.woocommerce());
        assert_eq!(&presentation, shop.presentation());
        assert_eq!(Some(&address), shop.address());
        assert_eq!(&contact, shop.contact());
        assert_eq!(ShopPartnerStatus::Partnered, shop.partner_status());
        assert_eq!(
            Some(&affiliate_configuration),
            shop.affiliate_configuration()
        );
    }

    #[test]
    fn should_rehydrate_shop_when_slug_matches_name() {
        let shop = Shop::rehydrate(shop_state("Antik und Stil", "antik-und-stil")).unwrap();

        assert_eq!("antik-und-stil", shop.slug_id().to_string());
    }

    #[test]
    fn should_reject_rehydrate_when_slug_does_not_match_name() {
        let result = Shop::rehydrate(shop_state("Antik und Stil", "wrong"));

        assert_eq!(
            Err(RehydrateShopError::SlugMismatch {
                expected: ShopSlugId::from("antik-und-stil"),
                actual: ShopSlugId::from("wrong"),
            }),
            result
        );
    }

    #[test]
    fn should_publish_drafted_shop_and_ignore_replay() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));

        assert_eq!(Ok(ChangeOutcome::Changed), shop.publish());
        assert_eq!(ShopLifecycle::Published, shop.lifecycle());
        assert_eq!(Ok(ChangeOutcome::Unchanged), shop.publish());
    }

    #[test]
    fn should_reject_publish_of_discarded_shop() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));
        assert_eq!(Ok(ChangeOutcome::Changed), shop.discard());

        assert_eq!(Err(ShopPublishError::Discarded), shop.publish());
        assert_eq!(ShopLifecycle::Discarded, shop.lifecycle());
    }

    #[test]
    fn should_discard_only_drafted_shop() {
        let mut published = Shop::create(new_shop("Antik und Stil"));
        assert_eq!(Ok(ChangeOutcome::Changed), published.publish());

        assert_eq!(Err(ShopDiscardError::NotDrafted), published.discard());
    }

    #[test]
    fn should_change_partner_status_when_status_differs() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));

        let outcome = shop.change_partner_status(ShopPartnerStatus::Partnered);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(ShopPartnerStatus::Partnered, shop.partner_status());
    }

    #[test]
    fn should_report_unchanged_when_partner_status_unchanged() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));

        let outcome = shop.change_partner_status(ShopPartnerStatus::Scraped);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_replace_domains_when_domains_differ() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));
        let domains = HashSet::from([domain("https://auction.example.com")]);

        let outcome = shop.replace_domains(domains);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert!(
            shop.domains()
                .contains(&domain("https://auction.example.com"))
        );
    }

    #[test]
    fn should_report_unchanged_when_shop_type_same() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));

        let outcome = shop.change_shop_type(ShopType::CommercialDealer);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_replace_contact_when_contact_differs() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));
        let contact = ShopContact {
            phone: Some("+49 123".to_string()),
            email: None,
        };

        let outcome = shop.replace_contact(contact);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(Some("+49 123"), shop.contact().phone.as_deref());
    }

    #[test]
    fn should_build_affiliate_view_url_from_canonical_url() {
        let mut input = new_shop("Antik und Stil");
        input.presentation = ShopPresentation {
            url: Some(Url::parse("https://example.com/l/123").unwrap()),
            image: None,
        };
        input.affiliate_configuration = Some(AffiliateConfiguration::Partnerize {
            camref: "1110lF73C".to_string(),
        });
        let shop = Shop::create(input);

        assert_eq!(
            Some(
                "https://prf.hn/click/camref:1110lF73C/pubref:aurahistoria/destination:https%3A%2F%2Fexample.com%2Fl%2F123"
            ),
            shop.view_url().as_ref().map(Url::as_str)
        );
    }

    #[test]
    fn should_change_shop_type_when_type_differs() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));

        let outcome = shop.change_shop_type(ShopType::Marketplace);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(ShopType::Marketplace, shop.shop_type());
    }

    #[test]
    fn should_report_unchanged_when_domains_same() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));
        let domains = shop.domains().clone();

        let outcome = shop.replace_domains(domains);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_replace_shopify_integration_when_changed() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));
        let shopify = ShopifyIntegration {
            domain: domain("shopify.example.com"),
            currency: Some(Currency::Eur),
            language: Some(Language::De),
        };

        let outcome = shop.replace_shopify_integration(Some(shopify));

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(
            Some(Currency::Eur),
            shop.shopify().and_then(|value| value.currency)
        );
    }

    #[test]
    fn should_clear_shopify_integration_when_changed() {
        let mut input = new_shop("Antik und Stil");
        input.shopify = Some(ShopifyIntegration {
            domain: domain("shopify.example.com"),
            currency: None,
            language: None,
        });
        let mut shop = Shop::create(input);

        let outcome = shop.replace_shopify_integration(None);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(None, shop.shopify());
    }

    #[test]
    fn should_report_unchanged_when_shopify_integration_same() {
        let shopify = ShopifyIntegration {
            domain: domain("shopify.example.com"),
            currency: Some(Currency::Eur),
            language: Some(Language::En),
        };
        let mut input = new_shop("Antik und Stil");
        input.shopify = Some(shopify.clone());
        let mut shop = Shop::create(input);

        let outcome = shop.replace_shopify_integration(Some(shopify));

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_replace_woocommerce_integration_when_changed() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));
        let woocommerce = WoocommerceIntegration {
            webhook_secret: Some(WoocommerceWebhookSecret::from("secret")),
            currency: Some(Currency::Usd),
            language: Some(Language::En),
        };

        let outcome = shop.replace_woocommerce_integration(Some(woocommerce));

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert!(shop.woocommerce().is_some());
    }

    #[test]
    fn should_clear_woocommerce_integration_when_changed() {
        let mut input = new_shop("Antik und Stil");
        input.woocommerce = Some(WoocommerceIntegration {
            webhook_secret: Some(WoocommerceWebhookSecret::from("secret")),
            currency: Some(Currency::Usd),
            language: Some(Language::En),
        });
        let mut shop = Shop::create(input);

        let outcome = shop.replace_woocommerce_integration(None);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(None, shop.woocommerce());
    }

    #[test]
    fn should_report_unchanged_when_woocommerce_integration_same() {
        let woocommerce = WoocommerceIntegration {
            webhook_secret: Some(WoocommerceWebhookSecret::from("secret")),
            currency: Some(Currency::Usd),
            language: Some(Language::En),
        };
        let mut input = new_shop("Antik und Stil");
        input.woocommerce = Some(woocommerce.clone());
        let mut shop = Shop::create(input);

        let outcome = shop.replace_woocommerce_integration(Some(woocommerce));

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_replace_presentation_when_changed() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));
        let presentation = ShopPresentation {
            url: Some(Url::parse("https://example.com").unwrap()),
            image: Some(Url::parse("https://example.com/image.jpg").unwrap()),
        };

        let outcome = shop.replace_presentation(presentation);

        assert_eq!(ChangeOutcome::Changed, outcome);
        assert_eq!(
            Some("https://example.com/"),
            shop.presentation().url.as_ref().map(Url::as_str)
        );
    }

    #[test]
    fn should_report_unchanged_when_presentation_same() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));
        let presentation = shop.presentation().clone();

        let outcome = shop.replace_presentation(presentation);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_replace_and_clear_address_when_changed() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));
        let address = ShopAddress {
            structured: StructuredAddress {
                addressline: Some("Main 1".to_string()),
                addressline_extra: None,
                locality: Some("Berlin".to_string()),
                region: None,
                postal_code: Some("10115".to_string()),
                country: Some(isocountry::CountryCode::DEU),
                continent: Some(crate::continent::Continent::Europe),
            },
            geo: Some(GeoAddress {
                lat: 52.5,
                lon: 13.4,
            }),
        };

        let set = shop.replace_address(Some(address));
        let clear = shop.replace_address(None);

        assert_eq!(ChangeOutcome::Changed, set);
        assert_eq!(ChangeOutcome::Changed, clear);
        assert_eq!(None, shop.address());
    }

    #[test]
    fn should_report_unchanged_when_address_same() {
        let address = ShopAddress {
            structured: StructuredAddress {
                addressline: Some("Main 1".to_string()),
                addressline_extra: None,
                locality: Some("Berlin".to_string()),
                region: None,
                postal_code: Some("10115".to_string()),
                country: Some(isocountry::CountryCode::DEU),
                continent: Some(crate::continent::Continent::Europe),
            },
            geo: Some(GeoAddress {
                lat: 52.5,
                lon: 13.4,
            }),
        };
        let mut input = new_shop("Antik und Stil");
        input.address = Some(address.clone());
        let mut shop = Shop::create(input);

        let outcome = shop.replace_address(Some(address));

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_report_unchanged_when_clearing_missing_address() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));

        let outcome = shop.replace_address(None);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_report_unchanged_when_contact_same() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));
        let contact = shop.contact().clone();

        let outcome = shop.replace_contact(contact);

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_replace_and_clear_affiliate_configuration_when_changed() {
        let mut shop = Shop::create(new_shop("Antik und Stil"));
        let affiliate = AffiliateConfiguration::Partnerize {
            camref: "1110lF73C".to_string(),
        };

        let set = shop.replace_affiliate_configuration(Some(affiliate));
        let clear = shop.replace_affiliate_configuration(None);

        assert_eq!(ChangeOutcome::Changed, set);
        assert_eq!(ChangeOutcome::Changed, clear);
        assert_eq!(None, shop.affiliate_configuration());
    }

    #[test]
    fn should_report_unchanged_when_affiliate_configuration_same() {
        let affiliate = AffiliateConfiguration::Partnerize {
            camref: "1110lF73C".to_string(),
        };
        let mut input = new_shop("Antik und Stil");
        input.affiliate_configuration = Some(affiliate.clone());
        let mut shop = Shop::create(input);

        let outcome = shop.replace_affiliate_configuration(Some(affiliate));

        assert_eq!(ChangeOutcome::Unchanged, outcome);
    }

    #[test]
    fn should_build_utm_view_url_when_no_affiliate_config() {
        let mut input = new_shop("Antik und Stil");
        input.presentation = ShopPresentation {
            url: Some(Url::parse("https://example.com/l/123").unwrap()),
            image: None,
        };
        let shop = Shop::create(input);

        assert_eq!(
            Some("https://example.com/l/123?utm_source=aura_historia&utm_medium=referral"),
            shop.view_url().as_ref().map(Url::as_str)
        );
    }

    #[test]
    fn should_not_build_view_url_when_canonical_url_missing() {
        let shop = Shop::create(new_shop("Antik und Stil"));

        assert_eq!(None, shop.view_url());
    }
}
