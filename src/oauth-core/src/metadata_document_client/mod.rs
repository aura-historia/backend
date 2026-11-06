//! Public OAuth clients identified by the HTTPS URL of their Client ID Metadata Document
//! (draft-ietf-oauth-client-id-metadata-document). See `docs/oauth.md`.

mod document;
mod policy;
mod redirect_uri;

pub use document::{
    ClientMetadataDocument, ClientMetadataDocumentClaims, InvalidClientMetadataDocument,
    MAX_METADATA_DOCUMENT_CLIENT_NAME_CHARS,
};
pub use policy::{
    DeniedMetadataDocumentHosts, InvalidDeniedMetadataDocumentHost,
    MetadataDocumentClientNotAdmitted, MetadataDocumentClientPolicy,
};
pub use redirect_uri::{InvalidMetadataDocumentRedirectUri, MetadataDocumentRedirectUri};

use crate::ordinary_user_scopes::ORDINARY_USER_SCOPES;
use credential_core::scope::Scope;
use std::collections::HashSet;

/// The scope ceiling of a metadata document client: the ordinary-user vocabulary.
pub fn metadata_document_client_scopes() -> HashSet<Scope> {
    HashSet::from(ORDINARY_USER_SCOPES)
}
