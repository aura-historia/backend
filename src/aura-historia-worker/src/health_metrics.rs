//! The host health metrics: once a minute the worker publishes the few figures the host's
//! CloudWatch alarms watch. The [README](../README.md#health-metrics) owns the metric contract;
//! the alarms are in the CDK app's monitoring stack.

use crate::{
    config::HealthMetricsConfig,
    scope::{BacklogClass, ConsumedScope},
};
use application::error::BoxError;
use aws_config::BehaviorVersion;
use aws_sdk_cloudwatch::{
    Client,
    types::{Dimension, MetricDatum, StandardUnit},
};
use aws_smithy_types::{DateTime, timeout::TimeoutConfig};
use delivery_outbox::OutboxStats;
use platform_aws::AwsSdkFailure;
use platform_postgres_queue::QueueOverview;
use sqlx::PgPool;
use std::{
    ffi::CString,
    os::unix::ffi::OsStrExt,
    path::Path,
    time::{Duration, SystemTime},
};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

pub const HEARTBEAT: &str = "Heartbeat";
pub const POSTGRES_REACHABLE: &str = "PostgresReachable";
pub const OLDEST_JOB_AGE: &str = "OldestJobAgeSeconds";
pub const PARKED_JOBS: &str = "ParkedJobs";
pub const OLDEST_PENDING_EMBEDDING_AGE: &str = "OldestPendingEmbeddingAgeSeconds";
pub const FREE_DISK: &str = "FreeDiskPercent";
pub const SINCE_LAST_BACKUP: &str = "SecondsSinceLastBackup";
pub const SINCE_LAST_JOB_SUCCESS: &str = "SecondsSinceLastSuccess";
/// The dimension that splits [`OLDEST_JOB_AGE`] by [`BacklogClass`].
pub const CLASS_DIMENSION: &str = "Class";
/// The dimension that names the scheduled job of [`SINCE_LAST_JOB_SUCCESS`].
pub const JOB_DIMENSION: &str = "Job";
/// Job markers beyond this many are ignored, so a stray directory cannot flood the metrics.
const MAX_JOB_MARKERS: usize = 16;

const INTERVAL: Duration = Duration::from_secs(60);
/// Bounds the reachability probe.
const REACHABILITY_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounds the queue figures, so a hanging database delays no heartbeat. The figures scan the job
/// table, which a large backlog makes slower than the pool's statement bound allows; their own
/// statement bound stays inside this.
const DATABASE_TIMEOUT: Duration = Duration::from_secs(20);
const FIGURES_STATEMENT_TIMEOUT: &str = "15s";

/// What one round publishes.
#[derive(Clone, Debug, PartialEq)]
pub struct HealthSnapshot {
    /// Whether PostgreSQL answered a trivial query in time.
    pub postgres_reachable: bool,
    /// The queue figures, or `None` when they could not be read in time.
    pub queues: Option<QueueHealth>,
    /// Free space of the host filesystem, or `None` when it could not be read.
    pub free_disk_percent: Option<f64>,
    /// Time since the last successful backup, or `None` while no backup is recorded.
    pub since_last_backup: Option<Duration>,
    /// Time since each scheduled job's last success, by job name.
    pub since_last_job_success: Vec<(String, Duration)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueHealth {
    /// The oldest waiting job of a normal scope, or the oldest undispatched outbox snapshot.
    pub oldest_normal: Duration,
    /// The oldest waiting job of a bulk scope.
    pub oldest_bulk: Duration,
    /// Jobs and outbox snapshots in failure custody.
    pub parked: u64,
    /// The oldest product embedding work that is neither committed nor parked.
    pub oldest_pending_embedding: Duration,
}

impl QueueHealth {
    /// A scope no consumer knows counts as normal, so its backlog alarms soonest.
    pub fn from_figures(queue: &QueueOverview, outbox: &OutboxStats) -> Self {
        let mut health = Self {
            oldest_normal: outbox.oldest_pending_age.unwrap_or_default(),
            oldest_bulk: Duration::ZERO,
            parked: queue.in_custody + outbox.custody,
            oldest_pending_embedding: Duration::ZERO,
        };
        for (scope, age) in &queue.oldest_waiting {
            let class = ConsumedScope::parse(scope.as_str())
                .map_or(BacklogClass::Normal, |scope| scope.backlog_class());
            let oldest = match class {
                BacklogClass::Normal => &mut health.oldest_normal,
                BacklogClass::Bulk => &mut health.oldest_bulk,
            };
            *oldest = (*oldest).max(*age);
        }
        health
    }
}

/// The CloudWatch client of the host's credentials, with bounds well inside one round.
pub async fn client() -> Client {
    let config = aws_config::defaults(BehaviorVersion::v2026_11_17())
        .timeout_config(
            TimeoutConfig::builder()
                .connect_timeout(Duration::from_secs(5))
                .operation_attempt_timeout(Duration::from_secs(10))
                .operation_timeout(Duration::from_secs(25))
                .build(),
        )
        .load()
        .await;
    Client::new(&config)
}

/// Publishes a round every minute until `shutdown`. A failed read or publish is logged and the
/// next round tries again; the missing data is what the alarms watch.
pub async fn run(
    pool: PgPool,
    config: HealthMetricsConfig,
    client: Client,
    shutdown: CancellationToken,
) {
    let mut rounds = tokio::time::interval(INTERVAL);
    rounds.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = rounds.tick() => {}
        }
        let snapshot = collect(&pool, &config).await;
        let data = metric_data(&snapshot, SystemTime::now());
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = publish(&client, &config.namespace, data) => {}
        }
    }
}

async fn collect(pool: &PgPool, config: &HealthMetricsConfig) -> HealthSnapshot {
    let postgres_reachable = matches!(
        tokio::time::timeout(REACHABILITY_TIMEOUT, sqlx::query("SELECT 1").execute(pool)).await,
        Ok(Ok(_))
    );
    let queues = if postgres_reachable {
        match tokio::time::timeout(DATABASE_TIMEOUT, queue_health(pool)).await {
            Ok(Ok(health)) => Some(health),
            Ok(Err(_)) | Err(_) => {
                tracing::warn!(
                    event = "worker.health_metrics.queue_figures_unreadable",
                    dependency = "postgres",
                    "The queue figures could not be read in time; leaving them out"
                );
                None
            }
        }
    } else {
        tracing::warn!(
            event = "worker.health_metrics.postgres_unreachable",
            dependency = "postgres",
            "PostgreSQL did not answer; publishing it as unreachable"
        );
        None
    };
    let free_disk_percent = free_disk_percent(&config.disk_path)
        .inspect_err(|error| {
            tracing::warn!(
                event = "worker.health_metrics.disk_unreadable",
                error_category = ?error.kind(),
            );
        })
        .ok();
    let since_last_backup = since_last_backup(&config.backup_marker, SystemTime::now())
        .inspect_err(|error| {
            // Before the first backup the marker is absent; the backup alarm covers that.
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    event = "worker.health_metrics.backup_marker_unreadable",
                    error_category = ?error.kind(),
                );
            }
        })
        .ok();
    let since_last_job_success = since_last_job_success(&config.job_markers, SystemTime::now())
        .unwrap_or_else(|error| {
            tracing::warn!(
                event = "worker.health_metrics.job_markers_unreadable",
                error_category = ?error.kind(),
            );
            Vec::new()
        });
    HealthSnapshot {
        postgres_reachable,
        queues,
        free_disk_percent,
        since_last_backup,
        since_last_job_success,
    }
}

/// Both reads in one read-only transaction, under a statement bound of their own.
async fn queue_health(pool: &PgPool) -> Result<QueueHealth, BoxError> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(FIGURES_STATEMENT_TIMEOUT)
        .execute(&mut *transaction)
        .await?;
    let queue = platform_postgres_queue::overview(&mut transaction).await?;
    let outbox = delivery_outbox::inspect(&mut transaction).await?;
    let oldest_pending_embedding =
        product_listing_postgres::oldest_unembedded_age(&mut transaction).await?;
    transaction.rollback().await?;
    Ok(QueueHealth {
        oldest_pending_embedding: oldest_pending_embedding.unwrap_or_default(),
        ..QueueHealth::from_figures(&queue, &outbox)
    })
}

/// Space available to unprivileged writers, as a share of the filesystem's size.
fn free_disk_percent(path: &Path) -> std::io::Result<f64> {
    let path = CString::new(path.as_os_str().as_bytes())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and outlives the call; `stats` is valid for writes.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a successful statvfs initialized every field.
    let stats = unsafe { stats.assume_init() };
    if stats.f_blocks == 0 {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    Ok(stats.f_bavail as f64 * 100.0 / stats.f_blocks as f64)
}

/// The backup script touches the marker after each successful backup.
fn since_last_backup(marker: &Path, now: SystemTime) -> std::io::Result<Duration> {
    let succeeded_at = std::fs::metadata(marker)?.modified()?;
    Ok(now.duration_since(succeeded_at).unwrap_or_default())
}

/// The host's scheduled jobs touch `<directory>/<job>` after each success. Entries that are not
/// files with a plain job name are skipped.
fn since_last_job_success(
    directory: &Path,
    now: SystemTime,
) -> std::io::Result<Vec<(String, Duration)>> {
    let mut jobs = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let plain = !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        let metadata = entry.metadata()?;
        if plain && metadata.is_file() {
            jobs.push((
                name,
                now.duration_since(metadata.modified()?).unwrap_or_default(),
            ));
        }
    }
    jobs.sort();
    jobs.truncate(MAX_JOB_MARKERS);
    Ok(jobs)
}

/// The heartbeat and PostgreSQL's reachability are always published; every other figure only
/// when it was read.
pub fn metric_data(snapshot: &HealthSnapshot, at: SystemTime) -> Vec<MetricDatum> {
    let at = DateTime::from(at);
    let datum = |name: &str, value: f64, unit: StandardUnit| {
        MetricDatum::builder()
            .metric_name(name)
            .value(value)
            .unit(unit)
            .timestamp(at)
    };
    let reachable = if snapshot.postgres_reachable {
        1.0
    } else {
        0.0
    };
    let mut data = vec![
        datum(HEARTBEAT, 1.0, StandardUnit::Count).build(),
        datum(POSTGRES_REACHABLE, reachable, StandardUnit::None).build(),
    ];
    if let Some(queues) = snapshot.queues {
        for (class, age) in [
            (BacklogClass::Normal, queues.oldest_normal),
            (BacklogClass::Bulk, queues.oldest_bulk),
        ] {
            data.push(
                datum(OLDEST_JOB_AGE, age.as_secs_f64(), StandardUnit::Seconds)
                    .dimensions(
                        Dimension::builder()
                            .name(CLASS_DIMENSION)
                            .value(class.as_str())
                            .build(),
                    )
                    .build(),
            );
        }
        data.push(datum(PARKED_JOBS, queues.parked as f64, StandardUnit::Count).build());
        data.push(
            datum(
                OLDEST_PENDING_EMBEDDING_AGE,
                queues.oldest_pending_embedding.as_secs_f64(),
                StandardUnit::Seconds,
            )
            .build(),
        );
    }
    if let Some(percent) = snapshot.free_disk_percent {
        data.push(datum(FREE_DISK, percent, StandardUnit::Percent).build());
    }
    if let Some(age) = snapshot.since_last_backup {
        data.push(datum(SINCE_LAST_BACKUP, age.as_secs_f64(), StandardUnit::Seconds).build());
    }
    for (job, age) in &snapshot.since_last_job_success {
        data.push(
            datum(
                SINCE_LAST_JOB_SUCCESS,
                age.as_secs_f64(),
                StandardUnit::Seconds,
            )
            .dimensions(Dimension::builder().name(JOB_DIMENSION).value(job).build())
            .build(),
        );
    }
    data
}

async fn publish(client: &Client, namespace: &str, data: Vec<MetricDatum>) {
    let Err(error) = client
        .put_metric_data()
        .namespace(namespace)
        .set_metric_data(Some(data))
        .send()
        .await
    else {
        return;
    };
    let failure = AwsSdkFailure::classify(&error);
    tracing::warn!(
        event = "worker.health_metrics.publish_failed",
        dependency = "cloudwatch",
        error_category = failure.kind().diagnostic_category(),
        http_status = failure.http_status(),
        "Host health metrics were not published; the next round tries again"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_postgres_queue::ScopeName;

    fn scope(name: &str) -> ScopeName {
        ScopeName::new(name).unwrap()
    }

    fn minutes(minutes: u64) -> Duration {
        Duration::from_secs(minutes * 60)
    }

    #[test]
    fn should_report_each_class_oldest_job_and_all_custody() {
        let queue = QueueOverview {
            oldest_waiting: vec![
                (scope("geo-resolution"), minutes(3)),
                (scope("product-listing-opensearch"), minutes(7)),
                (scope("product-translation"), minutes(90)),
                (scope("product-listing-opensearch-g7"), minutes(200)),
                (scope("product-embedding"), minutes(30)),
            ],
            in_custody: 10,
        };
        let outbox = OutboxStats {
            pending: 4,
            oldest_pending_age: Some(minutes(2)),
            custody: 1,
        };

        assert_eq!(
            QueueHealth {
                oldest_normal: minutes(7),
                oldest_bulk: minutes(200),
                parked: 11,
                oldest_pending_embedding: Duration::ZERO,
            },
            QueueHealth::from_figures(&queue, &outbox)
        );
    }

    #[test]
    fn should_count_a_stuck_outbox_and_unknown_scopes_as_normal_backlog() {
        let queue = QueueOverview {
            oldest_waiting: vec![(scope("retired-scope"), minutes(20))],
            in_custody: 0,
        };
        let stuck_outbox = OutboxStats {
            pending: 1,
            oldest_pending_age: Some(minutes(45)),
            custody: 0,
        };

        let health = QueueHealth::from_figures(&queue, &stuck_outbox);
        assert_eq!(
            (minutes(45), Duration::ZERO),
            (health.oldest_normal, health.oldest_bulk)
        );

        let idle = OutboxStats {
            pending: 0,
            oldest_pending_age: None,
            custody: 0,
        };
        assert_eq!(
            minutes(20),
            QueueHealth::from_figures(&queue, &idle).oldest_normal
        );
    }

    fn names(data: &[MetricDatum]) -> Vec<String> {
        data.iter()
            .map(|datum| {
                let class = datum
                    .dimensions()
                    .iter()
                    .map(|dimension| format!("[{}]", dimension.value().unwrap_or_default()))
                    .collect::<String>();
                format!("{}{class}", datum.metric_name().unwrap_or_default())
            })
            .collect()
    }

    #[test]
    fn should_publish_every_figure_of_a_healthy_round() {
        let snapshot = HealthSnapshot {
            postgres_reachable: true,
            queues: Some(QueueHealth {
                oldest_normal: Duration::from_secs(5),
                oldest_bulk: Duration::ZERO,
                parked: 2,
                oldest_pending_embedding: Duration::from_secs(7200),
            }),
            free_disk_percent: Some(59.5),
            since_last_backup: Some(Duration::from_secs(3600)),
            since_last_job_success: vec![("fx-refresh".to_owned(), Duration::from_secs(60))],
        };

        let data = metric_data(&snapshot, SystemTime::UNIX_EPOCH);

        assert_eq!(
            vec![
                "Heartbeat",
                "PostgresReachable",
                "OldestJobAgeSeconds[normal]",
                "OldestJobAgeSeconds[bulk]",
                "ParkedJobs",
                "OldestPendingEmbeddingAgeSeconds",
                "FreeDiskPercent",
                "SecondsSinceLastBackup",
                "SecondsSinceLastSuccess[fx-refresh]",
            ],
            names(&data)
        );
        let values: Vec<_> = data.iter().filter_map(MetricDatum::value).collect();
        assert_eq!(
            vec![1.0, 1.0, 5.0, 0.0, 2.0, 7200.0, 59.5, 3600.0, 60.0],
            values
        );
    }

    #[test]
    fn should_keep_the_heartbeat_and_report_postgres_unreachable_when_nothing_was_read() {
        let snapshot = HealthSnapshot {
            postgres_reachable: false,
            queues: None,
            free_disk_percent: None,
            since_last_backup: None,
            since_last_job_success: Vec::new(),
        };

        let data = metric_data(&snapshot, SystemTime::UNIX_EPOCH);

        assert_eq!(vec!["Heartbeat", "PostgresReachable"], names(&data));
        assert_eq!(Some(0.0), data[1].value());
    }

    #[test]
    fn should_report_postgres_reachable_when_only_the_queue_figures_timed_out() {
        let snapshot = HealthSnapshot {
            postgres_reachable: true,
            queues: None,
            free_disk_percent: None,
            since_last_backup: None,
            since_last_job_success: Vec::new(),
        };

        let data = metric_data(&snapshot, SystemTime::UNIX_EPOCH);

        assert_eq!(vec!["Heartbeat", "PostgresReachable"], names(&data));
        assert_eq!(Some(1.0), data[1].value());
    }

    #[test]
    fn should_measure_the_backup_age_from_the_markers_modification_time() {
        let directory = std::env::temp_dir().join(format!(
            "health-metrics-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let marker = directory.join("last-success");

        assert_eq!(
            std::io::ErrorKind::NotFound,
            since_last_backup(&marker, SystemTime::now())
                .unwrap_err()
                .kind()
        );
        std::fs::write(&marker, b"").unwrap();
        let written = std::fs::metadata(&marker).unwrap().modified().unwrap();
        assert_eq!(
            minutes(90),
            since_last_backup(&marker, written + minutes(90)).unwrap()
        );
        let percent = free_disk_percent(&directory).unwrap();
        assert!((0.0..=100.0).contains(&percent), "{percent}");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn should_measure_each_jobs_age_from_its_marker_and_skip_other_entries() {
        let directory = std::env::temp_dir().join(format!(
            "health-metrics-jobs-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(directory.join("periodic-match.d")).unwrap();
        for name in ["fx-refresh", "backend-cleanup", "Not A Job"] {
            std::fs::write(directory.join(name), b"").unwrap();
        }
        let written = std::fs::metadata(directory.join("fx-refresh"))
            .unwrap()
            .modified()
            .unwrap();

        let jobs = since_last_job_success(&directory, written + minutes(30)).unwrap();

        assert_eq!(
            vec!["backend-cleanup", "fx-refresh"],
            jobs.iter().map(|(job, _)| job.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(minutes(30), jobs[1].1);
        std::fs::remove_dir_all(&directory).unwrap();
        assert!(since_last_job_success(&directory, SystemTime::now()).is_err());
    }
}
