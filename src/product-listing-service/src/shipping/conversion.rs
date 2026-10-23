//! Request-local display conversion of advertised shipping rates.
use super::{ConvertedShippingRate, ShippingDisplay};
use fxrate_core::{FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use money::{Currency, Price};
use shipping_core::{
    assessment::EffectiveDeliveryTerms,
    pricing::{PriceRange, ShippingPrice},
    terms::ShippingPolicy,
};
use std::collections::HashMap;

/// A source rate at its zone/service position in the effective terms.
pub(super) struct AdvertisedRate {
    pub(super) zone_index: usize,
    pub(super) service_index: usize,
    pub(super) price: ShippingPrice,
}

/// Every advertised rate of the effective shipping coverage, in terms order.
pub(super) fn advertised_rates(
    effective: &EffectiveDeliveryTerms,
) -> impl Iterator<Item = AdvertisedRate> + '_ {
    let zones = match effective.terms().shipping() {
        Some(ShippingPolicy::Coverage(coverage)) => coverage.zones(),
        _ => &[],
    };
    zones.iter().enumerate().flat_map(|(zone_index, zone)| {
        zone.services()
            .iter()
            .enumerate()
            .filter_map(move |(service_index, service)| {
                service.rate().map(|rate| AdvertisedRate {
                    zone_index,
                    service_index,
                    price: rate.price(),
                })
            })
    })
}

/// Request-local memoization reuses conversions across profiles, inline terms and overrides.
pub(super) struct ShippingConversion<'a> {
    snapshot: &'a FxRateSnapshot,
    currency: Currency,
    amounts: HashMap<(Currency, u64), Price>,
}
impl<'a> ShippingConversion<'a> {
    pub(super) fn new(snapshot: &'a FxRateSnapshot, currency: Currency) -> Self {
        Self {
            snapshot,
            currency,
            amounts: HashMap::new(),
        }
    }
    fn amount(&mut self, price: Price) -> Result<Price, FxRateSnapshotError> {
        let key = (price.currency, price.monetary_amount.into());
        if let Some(converted) = self.amounts.get(&key) {
            return Ok(*converted);
        }
        let converted = self
            .snapshot
            .convert(price, self.currency, RoundingMode::HalfUp)?;
        self.amounts.insert(key, converted);
        Ok(converted)
    }
    fn price(
        &mut self,
        price: ShippingPrice,
    ) -> Result<Option<ShippingPrice>, FxRateSnapshotError> {
        Ok(Some(match price {
            ShippingPrice::Free(Some(currency)) => {
                self.amount(Price::new(0_u64.into(), currency))?;
                ShippingPrice::Free(Some(self.currency))
            }
            ShippingPrice::Fixed(price) => ShippingPrice::Fixed(self.amount(price)?),
            ShippingPrice::From(price) => ShippingPrice::From(self.amount(price)?),
            ShippingPrice::EstimatedRange(range) => ShippingPrice::EstimatedRange(
                PriceRange::new(self.amount(range.minimum())?, self.amount(range.maximum())?)
                    .expect("monotone conversion preserves range bounds"),
            ),
            _ => return Ok(None),
        }))
    }
    /// Requested-currency values of the given rates. Unknown-currency free, on-request and
    /// externally calculated rates have none.
    pub(super) fn display(
        &mut self,
        rates: impl IntoIterator<Item = AdvertisedRate>,
    ) -> Result<ShippingDisplay, FxRateSnapshotError> {
        let mut converted = Vec::new();
        for rate in rates {
            if let Some(price) = self.price(rate.price)? {
                converted.push(ConvertedShippingRate {
                    zone_index: rate.zone_index,
                    service_index: rate.service_index,
                    price,
                });
            }
        }
        Ok(ShippingDisplay {
            currency: self.currency,
            fx_rate_id: self.snapshot.id(),
            captured_at: self.snapshot.captured_at(),
            rates: converted,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxrate_core::FxRateId;
    use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};
    use shipping_core::pricing::ChargeBasis;
    use shipping_core::{
        coverage::{
            CoverageCompleteness, DeliveryArea, ReferenceCountry, ShippingCoverage, ShippingZone,
        },
        inputs::Applicability,
        selection::{
            ComponentOverride, DeliveryOverrides, ListingDeliveryConfiguration, ProfileSelection,
        },
        terms::{DeliveryTerms, ShippingService, ShippingServiceCode, ShippingServiceTerms},
    };
    use strum::IntoEnumIterator;
    use time::OffsetDateTime;

    fn snapshot() -> FxRateSnapshot {
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    match currency {
                        Currency::Usd => 1_100_000,
                        Currency::Jpy => 160_000_000,
                        _ => FX_RATE_SCALE,
                    },
                )
            }),
        )
        .unwrap()
        .into_persisted(1_i64.try_into().unwrap())
    }
    fn terms(prices: &[ShippingPrice]) -> shipping_core::terms::DeliveryTerms {
        let release = geo::reference::ReferenceRelease::CURRENT;
        DeliveryTerms::new(
            Some(ShippingPolicy::coverage(
                ShippingCoverage::new(
                    release,
                    CoverageCompleteness::Partial,
                    Applicability::default(),
                    vec![],
                    vec![
                        ShippingZone::new(
                            vec![DeliveryArea::Country(
                                ReferenceCountry::new("DE", release).unwrap(),
                            )],
                            Applicability::default(),
                            prices
                                .iter()
                                .enumerate()
                                .map(|(i, price)| {
                                    ShippingService::new(
                                        ShippingServiceCode::new(format!("service-{i}")).unwrap(),
                                        ShippingServiceTerms {
                                            rate: Some(shipping_core::pricing::ShippingRate::new(
                                                *price,
                                                Some(ChargeBasis::PerItem),
                                                None,
                                                Default::default(),
                                            )),
                                            ..Default::default()
                                        },
                                    )
                                })
                                .collect(),
                        )
                        .unwrap(),
                    ],
                    None,
                )
                .unwrap(),
            )),
            None,
        )
    }
    #[test]
    fn effective_multi_currency_profile_and_override_preserve_source_terms_and_reuse_conversions() {
        let mut gallery = shipping_core::fixtures::gallery();
        let eur = Price::new(100_u64.into(), Currency::Eur);
        let jpy = Price::new(160_u64.into(), Currency::Jpy);
        let mut content = gallery.profile.content().clone();
        content.terms = terms(&[ShippingPrice::Fixed(eur), ShippingPrice::Fixed(jpy)]);
        gallery.profile.replace_content(content).unwrap();
        let shared = EffectiveDeliveryTerms::resolve(
            &gallery.painting,
            Some(&gallery.source_default),
            &[gallery.profile],
        );
        let original = shared.clone();
        let snapshot = snapshot();
        let mut conversion = ShippingConversion::new(&snapshot, Currency::Usd);
        for _ in 0..50 {
            let display = conversion.display(advertised_rates(&shared)).unwrap();
            assert_eq!(snapshot.id(), display.fx_rate_id);
            assert_eq!(snapshot.captured_at(), display.captured_at);
            assert_eq!(2, display.rates.len());
            for rate in display.rates {
                assert_eq!(
                    ShippingPrice::Fixed(Price::new(110_u64.into(), Currency::Usd)),
                    rate.price
                );
            }
        }
        assert_eq!(2, conversion.amounts.len());
        assert_eq!(original, shared);
        let override_terms = terms(&[ShippingPrice::Fixed(Price::new(
            220_u64.into(),
            Currency::Usd,
        ))]);
        let override_config = ListingDeliveryConfiguration::new(
            gallery.source_id,
            ProfileSelection::SuppressInheritance,
            Default::default(),
            DeliveryOverrides {
                shipping: ComponentOverride::Replace(override_terms.shipping().unwrap().clone()),
                ..Default::default()
            },
        )
        .unwrap();
        let effective = EffectiveDeliveryTerms::resolve(&override_config, None, &[]);
        assert_eq!(
            ShippingPrice::Fixed(Price::new(220_u64.into(), Currency::Usd)),
            conversion
                .display(advertised_rates(&effective))
                .unwrap()
                .rates[0]
                .price
        );
        let mut yen = ShippingConversion::new(&snapshot, Currency::Jpy);
        assert_eq!(
            Some(ShippingPrice::Fixed(jpy)),
            yen.price(ShippingPrice::Fixed(eur)).unwrap()
        );
    }
    #[test]
    fn conversion_retains_uncertainty_and_basis_without_inventing_amounts() {
        let snapshot = snapshot();
        let mut conversion = ShippingConversion::new(&snapshot, Currency::Usd);
        let eur = Price::new(100_u64.into(), Currency::Eur);
        let usd = Price::new(110_u64.into(), Currency::Usd);
        assert_eq!(
            Some(ShippingPrice::Free(Some(Currency::Usd))),
            conversion
                .price(ShippingPrice::Free(Some(Currency::Jpy)))
                .unwrap()
        );
        assert_eq!(
            Some(ShippingPrice::From(usd)),
            conversion.price(ShippingPrice::From(eur)).unwrap()
        );
        assert_eq!(
            Some(ShippingPrice::EstimatedRange(
                PriceRange::new(usd, Price::new(220_u64.into(), Currency::Usd)).unwrap()
            )),
            conversion
                .price(ShippingPrice::EstimatedRange(
                    PriceRange::new(eur, Price::new(200_u64.into(), Currency::Eur)).unwrap()
                ))
                .unwrap()
        );
        for price in [
            ShippingPrice::Free(None),
            ShippingPrice::OnRequest(None),
            ShippingPrice::OnRequest(Some(Currency::Eur)),
            ShippingPrice::ExternallyCalculated(Some(Currency::Eur)),
        ] {
            assert_eq!(None, conversion.price(price).unwrap());
        }
        assert_eq!(
            Err(FxRateSnapshotError::ConversionOverflow),
            conversion.price(ShippingPrice::Fixed(Price::new(
                u64::MAX.into(),
                Currency::Eur
            )))
        );
    }
    #[test]
    fn missing_legacy_quote_fails_instead_of_substituting_a_currency() {
        let snapshot = snapshot();
        let legacy = FxRateSnapshot::rehydrate(
            snapshot.id(),
            snapshot.generation(),
            snapshot.captured_at(),
            snapshot.source(),
            snapshot.quotes().iter().copied().take(19),
        )
        .unwrap();
        let mut conversion = ShippingConversion::new(&legacy, Currency::Eur);
        assert_eq!(
            Err(FxRateSnapshotError::MissingQuote(Currency::Krw)),
            conversion.price(ShippingPrice::Fixed(Price::new(
                1_u64.into(),
                Currency::Krw
            )))
        );
    }
}
