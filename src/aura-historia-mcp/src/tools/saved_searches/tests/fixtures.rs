use super::*;
use search_filter_service::use_cases::{
    CreateSearchFilterCommand, CreateSearchFilterResult, DeleteOwnedSearchFilterError,
    DeleteOwnedSearchFilterResult, GetOwnedSearchFilterHandler, ListOwnedSearchFiltersHandler,
    ListSearchFilterMatchesError, ListSearchFilterMatchesRequest, ListSearchFilterMatchesResult,
    UpdateOwnedSearchFilterCommand, UpdateOwnedSearchFilterResult,
    UpdateSearchFilterMatchFeedbackError, UpdateSearchFilterMatchFeedbackResult,
};
#[derive(Debug, Clone)]
pub(super) enum Call {
    Create(OperationContext, CreateSearchFilterCommand),
    Update(OperationContext, UpdateOwnedSearchFilterCommand),
    Delete(OperationContext, DeleteOwnedSearchFilterCommand),
    Matches(OperationContext, ListSearchFilterMatchesRequest),
    Feedback(OperationContext, UpdateSearchFilterMatchFeedbackCommand),
}
#[derive(Clone, Default)]
pub(super) struct Fixture {
    pub(super) user: UserId,
    pub(super) include_match: bool,
    pub(super) filter: UserSearchFilterId,
    pub(super) calls: Arc<Mutex<Vec<Call>>>,
    pub(super) fail_create: Arc<Mutex<Option<&'static str>>>,
}
impl Fixture {
    fn view(&self) -> SearchFilterView {
        SearchFilterView {
            search_filter_id: self.filter,
            user_id: self.user,
            name: "Saved".into(),
            notifications: true,
            state: SearchFilterState::Active,
            search: product_listing_core::product_listing_search::ProductListingSearch::new(
                localization::Language::En,
                money::Currency::Eur,
            ),
            embedding: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }
    fn services(&self) -> SavedSearches {
        SavedSearches {
            list: Arc::new(ListOwnedSearchFiltersHandler::new(self.clone())),
            get: Arc::new(GetOwnedSearchFilterHandler::new(self.clone())),
            create: Arc::new(self.clone()),
            update: Arc::new(self.clone()),
            delete: Arc::new(self.clone()),
            matches: Arc::new(self.clone()),
            feedback: Arc::new(self.clone()),
        }
    }
    pub(super) fn registry(&self) -> Registry {
        Registry::new(operations(self.services()).unwrap()).unwrap()
    }
    pub(super) fn context(&self, scope: CredentialCapability) -> InvocationContext {
        InvocationContext {
            operation: OperationContext {
                principal: Principal::DelegatedUser {
                    user_id: self.user,
                    capabilities: [scope].into(),
                },
                request_id: RequestId::new("saved-search-test"),
                correlation_id: CorrelationId::new("correlation"),
            },
            deadline: tokio::time::Instant::now() + std::time::Duration::from_secs(5),
            cancellation: tokio_util::sync::CancellationToken::new(),
            max_response_bytes: crate::MAX_RESPONSE_BYTES,
        }
    }
}
#[async_trait::async_trait]
impl SearchFilterReader for Fixture {
    async fn find_for_user(
        &self,
        user_id: UserId,
    ) -> Result<Vec<SearchFilterView>, SearchFilterReadError> {
        Ok(if user_id == self.user {
            vec![self.view()]
        } else {
            vec![]
        })
    }
}
#[async_trait::async_trait]
impl CreateSearchFilterUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateSearchFilterCommand,
    ) -> Result<CreateSearchFilterResult, CreateSearchFilterError> {
        context
            .require()
            .credential_capability(CredentialCapability::SearchFiltersWrite)
            .user(&command.user_id)
            .authorize::<CreateSearchFilterError>()?;
        self.calls
            .lock()
            .unwrap()
            .push(Call::Create(context.clone(), command.clone()));
        match *self.fail_create.lock().unwrap() {
            Some("quota") => {
                return Err(CreateSearchFilterError::SearchFilterQuotaExceeded {
                    active_count: 2,
                    quota: 2,
                });
            }
            Some("feature") => {
                return Err(CreateSearchFilterError::SearchFilterFeatureRestricted {
                    feature: "geography",
                });
            }
            Some("unavailable") => {
                return Err(CreateSearchFilterError::CommitTransactionFailed(
                    static_error("private DB diagnostic"),
                ));
            }
            _ => {}
        }
        let mut filter = self.view();
        filter.name = command.name;
        filter.notifications = command.notifications;
        filter.search = command.search;
        Ok(CreateSearchFilterResult { filter })
    }
}
#[async_trait::async_trait]
impl UpdateOwnedSearchFilterUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateOwnedSearchFilterCommand,
    ) -> Result<UpdateOwnedSearchFilterResult, UpdateOwnedSearchFilterError> {
        context
            .require()
            .credential_capability(CredentialCapability::SearchFiltersWrite)
            .user(&command.user_id)
            .authorize::<UpdateOwnedSearchFilterError>()?;
        self.calls
            .lock()
            .unwrap()
            .push(Call::Update(context.clone(), command.clone()));
        if command.search_filter_id != self.filter || command.user_id != self.user {
            return Err(UpdateOwnedSearchFilterError::SearchFilterNotFound);
        }
        Ok(UpdateOwnedSearchFilterResult {
            filter: self.view(),
        })
    }
}
#[async_trait::async_trait]
impl DeleteOwnedSearchFilterUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteOwnedSearchFilterCommand,
    ) -> Result<DeleteOwnedSearchFilterResult, DeleteOwnedSearchFilterError> {
        context
            .require()
            .credential_capability(CredentialCapability::SearchFiltersWrite)
            .user(&command.user_id)
            .authorize::<DeleteOwnedSearchFilterError>()?;
        self.calls
            .lock()
            .unwrap()
            .push(Call::Delete(context.clone(), command.clone()));
        if command.search_filter_id != self.filter || command.user_id != self.user {
            return Err(DeleteOwnedSearchFilterError::SearchFilterNotFound);
        }
        Ok(DeleteOwnedSearchFilterResult)
    }
}
#[async_trait::async_trait]
impl ListSearchFilterMatchesUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListSearchFilterMatchesRequest,
    ) -> Result<ListSearchFilterMatchesResult, ListSearchFilterMatchesError> {
        context
            .require()
            .credential_capability(CredentialCapability::SearchFiltersRead)
            .user(&request.user_id)
            .authorize::<ListSearchFilterMatchesError>()?;
        self.calls
            .lock()
            .unwrap()
            .push(Call::Matches(context.clone(), request.clone()));
        if request.search_filter_id != self.filter || request.user_id != self.user {
            return Err(ListSearchFilterMatchesError::SearchFilterNotFound);
        }
        Ok(CursoredResult {
            items: if self.include_match {
                vec![matched_listing(
                    self.filter,
                    request.language,
                    request.currency,
                )]
            } else {
                vec![]
            },
            cursor: Cursor {
                size: request.cursor.as_ref().unwrap().size,
                search_after: Some(SearchFilterMatchCursor {
                    created: OffsetDateTime::UNIX_EPOCH,
                    product_listing_id: Default::default(),
                }),
            },
            total: Some(11),
        })
    }
}
#[async_trait::async_trait]
impl UpdateSearchFilterMatchFeedbackUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateSearchFilterMatchFeedbackCommand,
    ) -> Result<UpdateSearchFilterMatchFeedbackResult, UpdateSearchFilterMatchFeedbackError> {
        context
            .require()
            .credential_capability(CredentialCapability::SearchFiltersWrite)
            .user(&command.user_id)
            .authorize::<UpdateSearchFilterMatchFeedbackError>()?;
        self.calls
            .lock()
            .unwrap()
            .push(Call::Feedback(context.clone(), command.clone()));
        if command.search_filter_id != self.filter || command.user_id != self.user {
            return Err(UpdateSearchFilterMatchFeedbackError::SearchFilterNotFound);
        }
        if command.product_listing_id.to_string() != "pl_01k6k0w000e008000000000001" {
            return Err(UpdateSearchFilterMatchFeedbackError::SearchFilterMatchNotFound);
        }
        Ok(UpdateSearchFilterMatchFeedbackResult {
            search_filter_match: SearchFilterMatchView {
                user_id: command.user_id,
                search_filter_id: command.search_filter_id,
                search_filter_name: Some("Captured name".into()),
                product_listing_id: command.product_listing_id,
                origin_event_id: Default::default(),
                shipping_match_valuation: Some(search_filter_core::ShippingMatchValuation {
                    fx_rate_id: Default::default(),
                    captured_at: OffsetDateTime::UNIX_EPOCH,
                    maximum_per_item: money::Price::new(100_u64.into(), money::Currency::Jpy),
                }),
                feedback: match command.feedback {
                    PatchField::Set(v) => Some(v),
                    _ => None,
                },
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
        })
    }
}
pub(super) fn create_input() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../aura-historia-contracts/tests/fixtures/saved-search.json"
    ))
    .unwrap()
}

fn matched_listing(
    filter_id: UserSearchFilterId,
    language: localization::Language,
    currency: money::Currency,
) -> product_listing_service::use_cases::PersonalizedProductListingDetailsView {
    use product_listing_service::use_cases::{
        DisplayProductListingPricing, ProductListingDetailsView, ProductListingImageView,
        ProductListingPricingPresentation, ProductListingPricingValuation,
    };
    let valuation = search_filter_core::ShippingMatchValuation {
        fx_rate_id: Default::default(),
        captured_at: OffsetDateTime::UNIX_EPOCH,
        maximum_per_item: money::Price::new(9200_u64.into(), money::Currency::Eur),
    };
    application::personalized::Personalized {
        item: ProductListingDetailsView {
            shipping: None,
            product_listing_id: "pl_01k6k0w000e008000000000001".parse().unwrap(),
            product_listing_title_slug_id: None,
            event_id: Default::default(),
            source: product_listing_service::ports::ListingSourceSummary {
                listing_source_id: Default::default(),
                name: "Source".try_into().unwrap(),
                slug_id: listing_source_core::ListingSourceSlugId::raw("source").unwrap(),
            },
            source_listing_id: "merchant:local/42".try_into().unwrap(),
            product_title: None,
            product_description: None,
            title: Some(localization::Localized::new(language, "Schrank".into())),
            description: None,
            pricing: ProductListingPricingPresentation {
                source: product_listing_core::product_listing::ProductListingPricing {
                    price: Some(money::Price::new(u64::MAX.into(), money::Currency::Eur).into()),
                    price_estimate_min: None,
                    price_estimate_max: None,
                },
                display: DisplayProductListingPricing {
                    price: Some(money::Price::new(u64::MAX.into(), currency).into()),
                    price_estimate_min: None,
                    price_estimate_max: None,
                },
                valuation: ProductListingPricingValuation::Current {
                    fx_rate_id: Default::default(),
                    captured_at: OffsetDateTime::UNIX_EPOCH,
                },
            },
            availability: Some(
                product_listing_core::listing_availability::ListingAvailability::InStock,
            ),
            lifecycle: product_listing_core::listing_lifecycle::ListingLifecycle::Active,
            url: "https://source.example/product/42".parse().unwrap(),
            view_url: "https://aura.example/product/42".parse().unwrap(),
            images: vec![ProductListingImageView {
                url: Some("https://source.example/image.jpg".parse().unwrap()),
            }],
            content_policy: None,
            auction: None,
            lot: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        },
        user_state: Some(
            product_listing_service::user_state::ProductListingUserStateView {
                search_filter: Some(product_listing_service::user_state::SearchFilterUserState {
                    matched: true,
                    hidden: false,
                    user_search_filter_id: Some(filter_id),
                    user_search_filter_name: Some("Snapshot name".into()),
                    match_feedback: Some(false),
                    shipping_match_valuation: Some(valuation),
                }),
                ..Default::default()
            },
        ),
    }
}
