use application::error::{box_error, static_error};
use serde::Deserialize;
use std::{collections::HashSet, time::Duration};
use user_core::{stripe_customer_id::StripeCustomerId, tier::UserTier, user_id::UserId};
use user_service::ports::stripe_subscription_sync::{
    StripeCustomerSubscriptionReader, StripeCustomerSubscriptionState, StripeSubscriptionSyncError,
};

#[derive(Clone)]
pub struct StripeSubscriptionReader {
    client: reqwest::Client,
    api_key: String,
    pro_product_id: String,
    ultimate_product_id: String,
    base_url: url::Url,
}

impl StripeSubscriptionReader {
    pub fn new(
        api_key: String,
        pro_product_id: String,
        ultimate_product_id: String,
    ) -> Result<Self, StripeSubscriptionSyncError> {
        if api_key.is_empty()
            || pro_product_id.is_empty()
            || ultimate_product_id.is_empty()
            || pro_product_id == ultimate_product_id
        {
            return Err(invalid("invalid Stripe subscription configuration"));
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(10))
                .build()
                .map_err(unavailable)?,
            api_key,
            pro_product_id,
            ultimate_product_id,
            base_url: url::Url::parse(super::STRIPE_API_BASE_URL).map_err(unavailable)?,
        })
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, StripeSubscriptionSyncError> {
        self.client
            .get(self.base_url.join(path).map_err(unavailable)?)
            .bearer_auth(&self.api_key)
            .query(query)
            .send()
            .await
            .map_err(unavailable)?
            .error_for_status()
            .map_err(unavailable)?
            .json()
            .await
            .map_err(|source| StripeSubscriptionSyncError::InvalidState {
                source: box_error(source),
            })
    }

    async fn read_current(
        &self,
        customer: &StripeCustomerId,
    ) -> Result<StripeCustomerSubscriptionState, StripeSubscriptionSyncError> {
        let customer_data: Customer = self
            .get(&format!("/v1/customers/{}", customer.as_ref()), &[])
            .await?;
        if customer_data.id != customer.as_ref() {
            return Err(invalid("Stripe customer identity mismatch"));
        }
        let user_id = customer_data
            .metadata
            .user_id
            .map(|id| id.parse::<UserId>())
            .transpose()
            .map_err(|source| StripeSubscriptionSyncError::InvalidState {
                source: box_error(source),
            })?;
        if customer_data.deleted {
            return Ok(StripeCustomerSubscriptionState {
                user_id,
                tier: UserTier::Free,
            });
        }
        let mut tier = UserTier::Free;
        let mut cursor = None;
        let mut seen = HashSet::new();
        loop {
            let mut query = vec![
                ("customer", customer.as_ref()),
                ("status", "all"),
                ("limit", "100"),
            ];
            if let Some(cursor) = cursor.as_deref() {
                query.push(("starting_after", cursor));
            }
            let page: SubscriptionPage = self.get("/v1/subscriptions", &query).await?;
            for subscription in &page.data {
                if subscription.customer != customer.as_ref()
                    || subscription.id.is_empty()
                    || !seen.insert(subscription.id.clone())
                {
                    return Err(invalid("inconsistent Stripe subscription page"));
                }
                let entitled = match subscription.status.as_str() {
                    "active" | "trialing" | "past_due" => true,
                    "canceled" | "unpaid" | "incomplete" | "incomplete_expired" | "paused" => false,
                    _ => return Err(invalid("unknown Stripe subscription status")),
                };
                if !entitled {
                    continue;
                }
                if subscription.items.has_more || subscription.items.data.is_empty() {
                    return Err(invalid("incomplete Stripe subscription items"));
                }
                for item in &subscription.items.data {
                    let item_tier = if item.price.product == self.pro_product_id {
                        UserTier::Pro
                    } else if item.price.product == self.ultimate_product_id {
                        UserTier::Ultimate
                    } else {
                        return Err(invalid("unmapped Stripe subscription product"));
                    };
                    tier = tier.max(item_tier);
                }
            }
            if !page.has_more {
                break;
            }
            cursor = Some(
                page.data
                    .last()
                    .ok_or_else(|| invalid("empty Stripe subscription page with more results"))?
                    .id
                    .clone(),
            );
        }
        Ok(StripeCustomerSubscriptionState { user_id, tier })
    }
}

#[async_trait::async_trait]
impl StripeCustomerSubscriptionReader for StripeSubscriptionReader {
    async fn read(
        &self,
        customer: &StripeCustomerId,
    ) -> Result<StripeCustomerSubscriptionState, StripeSubscriptionSyncError> {
        tokio::time::timeout(Duration::from_secs(15), self.read_current(customer))
            .await
            .map_err(unavailable)?
    }
}

fn unavailable(
    source: impl std::error::Error + Send + Sync + 'static,
) -> StripeSubscriptionSyncError {
    StripeSubscriptionSyncError::Unavailable {
        source: box_error(source),
    }
}

fn invalid(message: &'static str) -> StripeSubscriptionSyncError {
    StripeSubscriptionSyncError::InvalidState {
        source: static_error(message),
    }
}

#[derive(Deserialize)]
struct Customer {
    id: String,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    metadata: CustomerMetadata,
}

#[derive(Default, Deserialize)]
struct CustomerMetadata {
    #[serde(rename = "userId")]
    user_id: Option<String>,
}

#[derive(Deserialize)]
struct SubscriptionPage {
    data: Vec<Subscription>,
    has_more: bool,
}

#[derive(Deserialize)]
struct Subscription {
    id: String,
    customer: String,
    status: String,
    items: SubscriptionItems,
}

#[derive(Deserialize)]
struct SubscriptionItems {
    data: Vec<SubscriptionItem>,
    has_more: bool,
}

#[derive(Deserialize)]
struct SubscriptionItem {
    price: Price,
}

#[derive(Deserialize)]
struct Price {
    product: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn server(
        responses: Vec<(&'static str, u16, Value)>,
    ) -> (StripeSubscriptionReader, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            for (path, status, body) in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 2048];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let size = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(0, size);
                    request.extend_from_slice(&buffer[..size]);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with(&format!("GET {path} HTTP/1.1\r\n")));
                let body = body.to_string();
                let response = format!(
                    "HTTP/1.1 {status} response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let mut reader = StripeSubscriptionReader::new(
            "test-key".to_owned(),
            "prod_pro".to_owned(),
            "prod_ultimate".to_owned(),
        )
        .unwrap();
        reader.base_url = url::Url::parse(&format!("http://{address}")).unwrap();
        reader.client = reqwest::Client::builder().no_proxy().build().unwrap();
        (reader, task)
    }
    fn subscription(id: &str, status: &str, product: &str) -> Value {
        json!({"id": id, "customer": "cus_1", "status": status,
            "items": {"has_more": false, "data": [{"price": {"product": product}}]}})
    }
    const LIST: &str = "/v1/subscriptions?customer=cus_1&status=all&limit=100";

    #[tokio::test]
    async fn reconciles_all_pages_and_uses_customer_identity() {
        let user_id = UserId::new();
        let (reader, task) = server(vec![
            ("/v1/customers/cus_1", 200, json!({"id": "cus_1", "metadata": {"userId": user_id}})),
            (LIST, 200, json!({"has_more": true, "data": [subscription("sub_1", "active", "prod_pro")]})),
            ("/v1/subscriptions?customer=cus_1&status=all&limit=100&starting_after=sub_1", 200, json!({"has_more": false, "data": [subscription("sub_2", "trialing", "prod_ultimate")]})),
        ]).await;
        let state = reader.read(&"cus_1".into()).await.unwrap();
        assert_eq!(Some(user_id), state.user_id);
        assert_eq!(UserTier::Ultimate, state.tier);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn only_entitled_current_statuses_grant_access() {
        for (status, expected) in [
            ("active", UserTier::Pro),
            ("trialing", UserTier::Pro),
            ("past_due", UserTier::Pro),
            ("canceled", UserTier::Free),
            ("unpaid", UserTier::Free),
            ("paused", UserTier::Free),
            ("incomplete", UserTier::Free),
            ("incomplete_expired", UserTier::Free),
        ] {
            let (reader, task) = server(vec![
                (
                    "/v1/customers/cus_1",
                    200,
                    json!({"id": "cus_1", "metadata": {}}),
                ),
                (
                    LIST,
                    200,
                    json!({"has_more": false, "data": [subscription("sub_1", status, "prod_pro")]}),
                ),
            ])
            .await;
            assert_eq!(expected, reader.read(&"cus_1".into()).await.unwrap().tier);
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn fails_closed_on_unknown_or_incomplete_provider_state() {
        let mut truncated = subscription("sub_1", "active", "prod_pro");
        truncated["items"]["has_more"] = json!(true);
        for body in [
            json!({"has_more": false, "data": [subscription("sub_1", "unknown", "prod_pro")]}),
            json!({"has_more": false, "data": [subscription("sub_1", "active", "prod_unknown")]}),
            json!({"has_more": true, "data": []}),
            json!({"has_more": false, "data": [truncated]}),
        ] {
            let (reader, task) = server(vec![
                (
                    "/v1/customers/cus_1",
                    200,
                    json!({"id": "cus_1", "metadata": {}}),
                ),
                (LIST, 200, body),
            ])
            .await;
            assert!(matches!(
                reader.read(&"cus_1".into()).await,
                Err(StripeSubscriptionSyncError::InvalidState { .. })
            ));
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn provider_outage_is_retryable() {
        let (reader, task) = server(vec![("/v1/customers/cus_1", 503, json!({}))]).await;
        assert!(matches!(
            reader.read(&"cus_1".into()).await,
            Err(StripeSubscriptionSyncError::Unavailable { .. })
        ));
        task.await.unwrap();
    }
}
