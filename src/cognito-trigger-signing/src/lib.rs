//! Authenticated requests from the Cognito triggers, which run outside the host, to the host's
//! internal endpoints: the signature protocol both sides share and the triggers' signed client.
pub mod signature;
pub mod signed_request;
