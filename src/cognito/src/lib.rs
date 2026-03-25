use crate::{
    access_token_verifier_service::{AccessTokenVerifierService, AccessTokenVerifierServiceImpl},
    localstack_access_token_verifier_service::LocalStackAccessTokenVerifierServiceImpl,
};

pub mod access_token_verifier_service;
pub mod localstack_access_token_verifier_service;

pub fn load_access_token_verifier_service<'a>(
    user_pool_id: &'a str,
    user_pool_client_ids: &'a [&'a str],
) -> Box<dyn AccessTokenVerifierService + Sync + Send + 'a> {
    match std::env::var("LOCALSTACK_HOSTNAME") {
        Ok(_) => {
            let mapped_port =
                std::env::var("LOCALSTACK_MAPPED_PORT").unwrap_or_else(|_| "4566".to_owned());
            // `host.docker.internal` resolves inside the Lambda container thanks to
            // `--add-host=host.docker.internal:host-gateway` in LAMBDA_DOCKER_FLAGS.
            let cognito_idp_endpoint = format!("http://host.docker.internal:{mapped_port}");
            Box::new(
                LocalStackAccessTokenVerifierServiceImpl::new(
                    &cognito_idp_endpoint,
                    "eu-central-1",
                    user_pool_id,
                    user_pool_client_ids,
                )
                .expect("shouldn't fail creating 'LocalStackAccessTokenVerifierServiceImpl'"),
            )
        }
        Err(_) => Box::new(
            AccessTokenVerifierServiceImpl::new("eu-central-1", user_pool_id, user_pool_client_ids)
                .expect("shouldn't fail creating 'AccessTokenVerifierServiceImpl'"),
        ),
    }
}
