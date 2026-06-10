use super::job::CrawlerCronJob;
use crate::network::policy::{NetworkErrorKind, retry_cooldown_for};
use crate::spider::advisory_lock::DomainLock;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, info, warn};

impl CrawlerCronJob {
    #[tracing::instrument(name = "crawler_run_spider_once", skip(self))]
    pub(super) async fn run_spider_once(&self) {
        match self
            .spider_candidates
            .get_candidates(self.config.spider_batch_size)
            .await
        {
            Ok(candidates) => {
                if candidates.is_empty() {
                    debug!("No spider candidates, skipping batch");
                    return;
                }
                let total = candidates.len();
                let batch_start = tokio::time::Instant::now();
                info!(candidates = total, "Spider batch starting");

                let spider_concurrency = self.config.spider_concurrency;
                if spider_concurrency == 0 {
                    warn!(
                        spider_concurrency,
                        "spider_concurrency is 0, skipping spider batch"
                    );
                    return;
                }

                let semaphore = Arc::new(Semaphore::new(spider_concurrency));
                let mut join_set: JoinSet<bool> = JoinSet::new();

                for candidate in candidates {
                    let spider_candidates = Arc::clone(&self.spider_candidates);
                    let spider_service = Arc::clone(&self.spider_service);
                    let lock_manager = Arc::clone(&self.lock_manager);
                    let permit_pool = Arc::clone(&semaphore);
                    let threshold = self.config.spider_classify_threshold;
                    let shop_url = if candidate.shop_domain.starts_with("http") {
                        candidate.shop_domain.clone()
                    } else {
                        format!("https://{}", candidate.shop_domain)
                    };
                    let span = tracing::info_span!(
                        "spider_candidate",
                        shop_id = %candidate.shop_id,
                        domain_id = %candidate.domain_id,
                        shop_url = %shop_url
                    );

                    join_set.spawn(async move {
                        let Ok(_permit) = permit_pool.acquire_owned().await else {
                            error!("Spider semaphore closed unexpectedly");
                            return false;
                        };

                        let Some(_lock) =
                            DomainLock::try_acquire(&lock_manager, candidate.domain_id)
                        else {
                            warn!(
                                shop_id = %candidate.shop_id,
                                domain_id = %candidate.domain_id,
                                "Skipping domain — lock held by another worker"
                            );
                            return false;
                        };

                        match spider_service
                            .run(
                                &candidate.shop_id,
                                &candidate.domain_id,
                                &shop_url,
                                threshold,
                            )
                            .await
                        {
                            Ok(_) => {
                                if let Err(err) = spider_candidates
                                    .reset_crawl_failure(&candidate.domain_id)
                                    .await
                                {
                                    warn!(
                                        error = ?err,
                                        domain = %candidate.shop_domain,
                                        "Failed to reset crawl failure metadata"
                                    );
                                }
                                true
                            }
                            Err(e) => {
                                let cooldown = retry_cooldown_for(NetworkErrorKind::Unknown);
                                let next_crawl_at = time::OffsetDateTime::now_utc()
                                    + time::Duration::seconds(cooldown.as_secs() as i64);
                                let error_kind = match &e {
                                    crate::spider::service::SpiderServiceError::UrlPattern(
                                        crate::spider::classification::url_pattern_service::UrlPatternServiceError::PendingReview {
                                            ..
                                        },
                                    ) => "PendingUrlPatternReview",
                                    crate::spider::service::SpiderServiceError::TinyCrawl {
                                        ..
                                    } => "TinyCrawl",
                                    crate::spider::service::SpiderServiceError::InsufficientInferenceSample {
                                        ..
                                    } => "InsufficientInferenceSample",
                                    _ => "spider_run_error",
                                };
                                if let Err(err) = spider_candidates
                                    .mark_crawl_failure(
                                        &candidate.domain_id,
                                        error_kind,
                                        next_crawl_at,
                                    )
                                    .await
                                {
                                    warn!(
                                        error = ?err,
                                        domain = %candidate.shop_domain,
                                        "Failed to persist crawl failure metadata"
                                    );
                                }
                                match &e {
                                    crate::spider::service::SpiderServiceError::TinyCrawl {
                                        total_links,
                                        ..
                                    } => warn!(
                                        domain = %candidate.shop_domain,
                                        error = %e,
                                        error_kind,
                                        total_crawled = *total_links,
                                        min_required_links = 2,
                                        next_crawl_at = %next_crawl_at,
                                        cooldown_seconds = cooldown.as_secs(),
                                        "Spider run failed"
                                    ),
                                    crate::spider::service::SpiderServiceError::InsufficientInferenceSample {
                                        stage,
                                        sample_size,
                                        min_sample_size,
                                        ..
                                    } => warn!(
                                        domain = %candidate.shop_domain,
                                        error = %e,
                                        error_kind,
                                        stage,
                                        sample_size = *sample_size,
                                        min_sample_size = *min_sample_size,
                                        next_crawl_at = %next_crawl_at,
                                        cooldown_seconds = cooldown.as_secs(),
                                        "Spider run failed"
                                    ),
                                    _ => warn!(
                                        domain = %candidate.shop_domain,
                                        error = %e,
                                        error_kind,
                                        next_crawl_at = %next_crawl_at,
                                        cooldown_seconds = cooldown.as_secs(),
                                        "Spider run failed"
                                    ),
                                }
                                false
                            }
                        }
                    }
                        .instrument(span));
                }

                let mut results: Vec<bool> = Vec::new();
                while let Some(joined) = join_set.join_next().await {
                    match joined {
                        Ok(ok) => results.push(ok),
                        Err(e) => {
                            error!(error = %e, "Spider worker task failed to join");
                            results.push(false);
                        }
                    }
                }

                let succeeded = results.iter().filter(|&&ok| ok).count();
                let failed = total - succeeded;
                let duration_ms = batch_start.elapsed().as_millis() as u64;
                info!(
                    total,
                    succeeded, failed, duration_ms, "Spider batch complete"
                );

                self.spider_perf.record(total as u64, duration_ms);
            }
            Err(e) => warn!(error = %e, "Failed to retrieve spider candidates"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::candidate_service::MockScraperCandidateService;
    use crate::scraper::scraper_service::MockScraperService;
    use crate::service::cron::config::CrawlerCronConfig;
    use crate::service::cron::test_support::{noop_product_push, noop_shop_registration};
    use crate::spider::advisory_lock::LocalLockManager;
    use crate::spider::candidate_service::{MockSpiderCandidateService, SpiderCandidate};
    use crate::spider::service::{MockSpiderService, SpiderRunResult};
    use common::shop_id::ShopId;

    #[tokio::test]
    async fn should_run_spider_candidates() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = uuid::Uuid::new_v4();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(vec![SpiderCandidate {
                        shop_id: ShopId::new(),
                        domain_id: expected_domain_id,
                        shop_domain: "example.com".to_string(),
                    }])
                })
            });
        spider_candidates
            .expect_reset_crawl_failure()
            .withf(move |domain_id| *domain_id == expected_domain_id)
            .returning(|_| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .withf(move |_, domain_id, _, _| *domain_id == expected_domain_id)
            .returning(|_, _, _, _| {
                Box::pin(async {
                    Ok(SpiderRunResult {
                        total_links: 10,
                        product_urls_count: 5,
                        product_pattern: None,
                    })
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            noop_product_push(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_mark_crawl_failure_when_spider_run_errors() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = uuid::Uuid::new_v4();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(vec![SpiderCandidate {
                        shop_id: ShopId::new(),
                        domain_id: expected_domain_id,
                        shop_domain: "example.com".to_string(),
                    }])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, _| {
                *domain_id == expected_domain_id && error_kind == "spider_run_error"
            })
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _| {
            Box::pin(async {
                Err(crate::spider::service::SpiderServiceError::Database(
                    sqlx::Error::RowNotFound,
                ))
            })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            noop_product_push(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_mark_tiny_crawl_failure_with_specific_error_kind() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = uuid::Uuid::new_v4();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(vec![SpiderCandidate {
                        shop_id: ShopId::new(),
                        domain_id: expected_domain_id,
                        shop_domain: "example.com".to_string(),
                    }])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, _| {
                *domain_id == expected_domain_id && error_kind == "TinyCrawl"
            })
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, shop_url, _| {
            let shop_url = shop_url.to_string();
            Box::pin(async move {
                Err(crate::spider::service::SpiderServiceError::TinyCrawl {
                    shop_url,
                    total_links: 1,
                })
            })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            noop_product_push(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_mark_insufficient_inference_sample_failure_with_specific_error_kind() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = uuid::Uuid::new_v4();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(vec![SpiderCandidate {
                        shop_id: ShopId::new(),
                        domain_id: expected_domain_id,
                        shop_domain: "example.com".to_string(),
                    }])
                })
            });
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, error_kind, _| {
                *domain_id == expected_domain_id && error_kind == "InsufficientInferenceSample"
            })
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, shop_url, _| {
            let shop_url = shop_url.to_string();
            Box::pin(async move {
                Err(
                    crate::spider::service::SpiderServiceError::InsufficientInferenceSample {
                        shop_url,
                        stage: "end_of_crawl",
                        sample_size: 16,
                        min_sample_size: 20,
                    },
                )
            })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            noop_product_push(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_skip_spider_candidate_when_domain_lock_is_already_held() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let locked_domain_id = uuid::Uuid::new_v4();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(vec![SpiderCandidate {
                        shop_id: ShopId::new(),
                        domain_id: locked_domain_id,
                        shop_domain: "example.com".to_string(),
                    }])
                })
            });

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().times(0);

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();
        let lock_manager = Arc::new(LocalLockManager::new());
        let _prelock = DomainLock::try_acquire(&lock_manager, locked_domain_id).unwrap();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::clone(&lock_manager),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            noop_product_push(),
        );

        job.run_spider_once().await;
    }
}
