use super::*;
use crate::{Isolation, TestContext};
use opensearch::IndexParts;
use product_listing_core::product_listing_id::ProductListingId;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn should_use_fresh_clients_for_setup_reads_and_cleanup_after_prior_runtimes_stop() {
    let setup = runtime();
    let id = ProductListingId::new().to_string();
    let (session, index) = setup.block_on(async {
        crate::aws::get_aws_emulator(&["opensearch"], &[]).await;
        let session = TestContext::acquire("OpenSearch runtime lifetime", Isolation::Isolated)
            .await
            .unwrap();
        let context = session.context().clone();
        let index = context
            .scope(async {
                OpenSearch().set_up().await;
                let index = context.product_listings_index().unwrap();
                get_opensearch_client()
                    .await
                    .index(IndexParts::IndexId(&index, &id))
                    .body(json!({"productListingId": id}))
                    .send()
                    .await
                    .unwrap()
                    .error_for_status_code()
                    .unwrap();
                refresh_index(&index).await;
                index
            })
            .await;
        (session, index)
    });
    drop(setup);

    let reads = runtime();
    reads.block_on(session.context().scope(async {
        let source: serde_json::Value = read_by_id(&index, &id).await;
        assert_eq!(id, source["productListingId"]);
        assert!(
            get_opensearch_client()
                .await
                .ping()
                .send()
                .await
                .unwrap()
                .status_code()
                .is_success()
        );
    }));
    drop(reads);

    let cleanup = runtime();
    cleanup.block_on(async {
        session.finish().await.unwrap();
        let response = get_opensearch_client()
            .await
            .indices()
            .exists(IndicesExistsParts::Index(&[&index]))
            .send()
            .await
            .unwrap();
        assert_eq!(StatusCode::NOT_FOUND, response.status_code());
    });
}
