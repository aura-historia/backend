use crate::error::OAuthServiceError;
use crate::ports::ClientMetadataDocumentSource;
use application::error::box_error;
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use oauth_core::metadata_document_client::{ClientMetadataDocument, MetadataDocumentClientPolicy};

/// The operator policy and document source for public clients identified by a metadata
/// document URL. Authorization and consent resolve such a client through it.
#[derive(Debug, Clone)]
pub struct MetadataDocumentClients<S> {
    policy: MetadataDocumentClientPolicy,
    source: S,
}

impl<S> MetadataDocumentClients<S> {
    pub fn new(policy: MetadataDocumentClientPolicy, source: S) -> Self {
        Self { policy, source }
    }

    pub fn policy(&self) -> &MetadataDocumentClientPolicy {
        &self.policy
    }
}

impl<S: ClientMetadataDocumentSource> MetadataDocumentClients<S> {
    /// Admits the client under the policy, then loads and validates its current document.
    /// Any failure makes the client invalid; no callback has been validated at that point.
    pub(crate) async fn resolve(
        &self,
        client_id: &MetadataDocumentClientId,
    ) -> Result<ClientMetadataDocument, OAuthServiceError> {
        let document = match self.policy.admit(client_id) {
            Ok(()) => match self.source.load(client_id).await {
                Ok(claims) => {
                    ClientMetadataDocument::validate(client_id, claims).map_err(box_error)
                }
                Err(error) => Err(box_error(error)),
            },
            Err(error) => Err(box_error(error)),
        };
        document.map_err(|source| {
            tracing::warn!(
                event = "oauth_metadata_document_client.rejected",
                client_host = client_id.host(),
                error_category = %cause_chain(source.as_ref()),
                outcome = "failure",
            );
            OAuthServiceError::InvalidMetadataDocumentClient { source }
        })
    }
}

/// Every message in the chain. Adapters keep document content and URLs out of them.
fn cause_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut chain = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        chain.push_str(": ");
        chain.push_str(&cause.to_string());
        source = cause.source();
    }
    chain
}
