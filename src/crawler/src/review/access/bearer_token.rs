//! The tokens a request's `Authorization` header may present, compared by digest.

use super::ReviewServerConfig;
use crate::review::model::ReviewActor;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// The credential a request's `Authorization` header presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Bearer {
    ReviewToken,
    /// The token of the review agent acting as this actor.
    Agent(ReviewActor),
    /// The request has no `Authorization` header.
    Absent,
    /// The header is not a bearer of a configured token.
    Invalid,
}

/// The tokens a bearer may present: the review token and, in `agent` authoring mode, the author
/// and reviewer agents' tokens, each kept as its digest.
#[derive(Clone, Copy)]
pub(super) struct BearerTokens {
    review: Option<TokenDigest>,
    author: Option<TokenDigest>,
    reviewer: Option<TokenDigest>,
}

impl BearerTokens {
    pub(super) fn of(config: &ReviewServerConfig) -> Self {
        let agents = config.agent_tokens.as_ref();
        Self {
            review: config.auth_token.as_deref().map(TokenDigest::of),
            author: agents.map(|tokens| TokenDigest::of(tokens.author())),
            reviewer: agents.map(|tokens| TokenDigest::of(tokens.reviewer())),
        }
    }

    /// Which token `presented` is. It is compared with every token, each on every request, so the
    /// timing tells none apart.
    pub(super) fn resolve(&self, presented: &str) -> Bearer {
        let presented = TokenDigest::of(presented);
        let matches =
            |token: Option<TokenDigest>| token.is_some_and(|token| token.matches(presented));
        match (
            matches(self.review),
            matches(self.author),
            matches(self.reviewer),
        ) {
            (true, _, _) => Bearer::ReviewToken,
            (false, true, _) => Bearer::Agent(ReviewActor::AgentAuthor),
            (false, false, true) => Bearer::Agent(ReviewActor::AgentReviewer),
            (false, false, false) => Bearer::Invalid,
        }
    }
}

/// A token's SHA-256 digest. Comparing digests instead of the tokens takes the same time whatever
/// either token's length, so it tells a client nothing about a configured token's length; a
/// configured token is hashed once, when the console starts, and a request hashes only the value
/// it presents.
#[derive(Clone, Copy)]
struct TokenDigest([u8; 32]);

impl TokenDigest {
    fn of(token: &str) -> Self {
        Self(Sha256::digest(token.as_bytes()).into())
    }

    /// Whether both are the digest of the same token. `subtle` reads every byte whatever the
    /// first difference and keeps the optimizer from turning the comparison into an early exit.
    fn matches(self, other: Self) -> bool {
        self.0.as_slice().ct_eq(other.0.as_slice()).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REVIEW_TOKEN: &str = "review-token";

    /// A presented token matches only the token it equals; one of another length is compared like
    /// any other value.
    #[rstest::rstest]
    #[case::equal("review-token", true)]
    #[case::different_of_equal_length("review-tokem", false)]
    #[case::shorter("review-toke", false)]
    #[case::longer("review-token-2", false)]
    #[case::empty("", false)]
    fn should_match_a_presented_token_only_when_it_equals_the_configured_one(
        #[case] presented: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(
            TokenDigest::of(REVIEW_TOKEN).matches(TokenDigest::of(presented)),
            expected
        );
    }
}
