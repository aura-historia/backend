//! The `ops` command line.

use crate::renormalization::RenormalizationOptions;
use delivery_outbox::RedriveSelection as OutboxRedriveSelection;
use listing_source_core::ListingSourceId;
use platform_postgres_queue::{JobId, RedriveSelection};
use platform_worker_queue_contract::job_queue::{ProjectionGeneration, ProjectionTarget};
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_raw_id::ProductListingRawStreamId,
};
use product_service::use_cases::RawRevisionReapplicationMode;
use std::{num::NonZeroU32, path::PathBuf};

/// The scope name that selects the delivery outbox in `redrive`.
pub const OUTBOX: &str = "delivery-outbox";
const DEFAULT_REPROJECTION_PAGE_SIZE: u32 = 500;
const DEFAULT_RENORMALIZATION_PAGE_SIZE: u32 = 200;
const DEFAULT_RENORMALIZATION_MAX_BACKLOG: u64 = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Applies a reviewed migration plan (JSON) as the migrator role.
    Migrate { plan: PathBuf },
    /// Prints the migration ledger and schema fingerprint as the read-only inspector role.
    InspectSchema,
    /// Creates the application roles on an empty database and applies every migration.
    Bootstrap,
    /// Prints depth, leases, oldest waiting age and custody per job scope, and the outbox.
    Inspect,
    /// Moves custody jobs of a scope back into the queue, preserving identity and order.
    Redrive { scope: String, selection: Selection },
    /// Enqueues one projector job per listing.
    ReprojectAll {
        target: ProjectionTarget,
        page_size: NonZeroU32,
        start_after: Option<ProductListingId>,
    },
    /// Normalizes the latest raw revision of caught-up streams again with the current normalizer.
    RenormalizeRaw(RenormalizationOptions),
    /// Expired-data cleanup and recovery maintenance.
    BackendCleanup,
    /// Captures the current FX rate snapshot.
    FxRefresh,
    /// Periodic saved-search matching.
    PeriodicMatch,
}

/// Which custody entries a redrive selects: every one, or the given IDs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    All,
    Ids(Vec<String>),
}

impl Selection {
    pub fn jobs(&self) -> Result<RedriveSelection, String> {
        Ok(match self {
            Self::All => RedriveSelection::All,
            Self::Ids(ids) => RedriveSelection::Jobs(
                ids.iter()
                    .map(|id| {
                        uuid::Uuid::parse_str(id)
                            .map(JobId::from_uuid)
                            .map_err(|_| format!("invalid job ID {id}"))
                    })
                    .collect::<Result<_, _>>()?,
            ),
        })
    }

    pub fn outbox_rows(&self) -> Result<OutboxRedriveSelection, String> {
        Ok(match self {
            Self::All => OutboxRedriveSelection::All,
            Self::Ids(ids) => OutboxRedriveSelection::Rows(
                ids.iter()
                    .map(|id| {
                        id.parse::<i64>()
                            .map_err(|_| format!("invalid outbox ID {id}"))
                    })
                    .collect::<Result<_, _>>()?,
            ),
        })
    }
}

impl Command {
    pub const USAGE: &str = "usage: ops <command>

commands:
  migrate --plan <file>          apply a reviewed migration plan
  inspect-schema                 print the migration ledger and schema fingerprint
  bootstrap                      create the roles and apply every migration on a new database
  inspect                        print queue and outbox figures
  redrive <scope> --all          move every custody job of <scope> back into the queue
  redrive <scope> <id>...        move the given custody jobs back; <scope> may be delivery-outbox
  reproject-all (--live | --generation <g>) [--page-size <n>] [--start-after <listing-id>]
                                 enqueue one projector job per listing
  renormalize-raw (--dry-run | --apply) [--listing-source <ls-id>] [--page-size <n>]
                  [--limit <n>] [--after <prs-id>] [--max-backlog <n>]
                                 normalize processed raw revisions again with this normalizer
  backend-cleanup                expired-data cleanup and recovery maintenance
  fx-refresh                     capture the current FX rate snapshot
  periodic-match                 periodic saved-search matching";

    pub fn name(&self) -> &'static str {
        match self {
            Self::Migrate { .. } => "migrate",
            Self::InspectSchema => "inspect-schema",
            Self::Bootstrap => "bootstrap",
            Self::Inspect => "inspect",
            Self::Redrive { .. } => "redrive",
            Self::ReprojectAll { .. } => "reproject-all",
            Self::RenormalizeRaw(_) => "renormalize-raw",
            Self::BackendCleanup => "backend-cleanup",
            Self::FxRefresh => "fx-refresh",
            Self::PeriodicMatch => "periodic-match",
        }
    }

    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut args = args.into_iter();
        let command = args.next().ok_or("missing command")?;
        let rest: Vec<String> = args.collect();
        let no_arguments = |command: Self| {
            if rest.is_empty() {
                Ok(command)
            } else {
                Err(format!("{} takes no arguments", command.name()))
            }
        };
        match command.as_str() {
            "migrate" => match rest.as_slice() {
                [flag, plan] if flag == "--plan" => Ok(Self::Migrate {
                    plan: PathBuf::from(plan),
                }),
                _ => Err("migrate needs --plan <file>".into()),
            },
            "inspect-schema" => no_arguments(Self::InspectSchema),
            "bootstrap" => no_arguments(Self::Bootstrap),
            "inspect" => no_arguments(Self::Inspect),
            "redrive" => parse_redrive(&rest),
            "reproject-all" => parse_reprojection(&rest),
            "renormalize-raw" => parse_renormalization(&rest),
            "backend-cleanup" => no_arguments(Self::BackendCleanup),
            "fx-refresh" => no_arguments(Self::FxRefresh),
            "periodic-match" => no_arguments(Self::PeriodicMatch),
            other => Err(format!("unknown command {other}")),
        }
    }
}

fn parse_redrive(args: &[String]) -> Result<Command, String> {
    let (scope, selection) = args.split_first().ok_or("redrive needs a scope")?;
    let selection = match selection {
        [] => return Err("redrive needs --all or the IDs to redrive".into()),
        [all] if all == "--all" => Selection::All,
        ids if ids.iter().any(|id| id.starts_with("--")) => {
            return Err("redrive takes either --all or IDs".into());
        }
        ids => Selection::Ids(ids.to_vec()),
    };
    // Validate the IDs now, so a typo fails before connecting.
    if scope == OUTBOX {
        selection.outbox_rows()?;
    } else {
        selection.jobs()?;
    }
    Ok(Command::Redrive {
        scope: scope.clone(),
        selection,
    })
}

fn parse_reprojection(args: &[String]) -> Result<Command, String> {
    let mut target = None;
    let mut page_size = NonZeroU32::new(DEFAULT_REPROJECTION_PAGE_SIZE).expect("positive");
    let mut start_after = None;
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--live" if target.is_none() => target = Some(ProjectionTarget::Live),
            "--generation" if target.is_none() => {
                let generation = value()?;
                target = Some(ProjectionTarget::Generation(
                    ProjectionGeneration::new(generation.as_str())
                        .map_err(|error| error.to_string())?,
                ));
            }
            "--page-size" => {
                page_size = value()?
                    .parse()
                    .map_err(|_| "--page-size must be a positive integer".to_owned())?;
            }
            "--start-after" => {
                start_after = Some(
                    value()?
                        .parse::<ProductListingId>()
                        .map_err(|_| "--start-after must be a ProductListing ID".to_owned())?,
                );
            }
            other => return Err(format!("unexpected reproject-all argument {other}")),
        }
    }
    Ok(Command::ReprojectAll {
        // Writing the live index is never a default: it must be chosen explicitly.
        target: target.ok_or("reproject-all needs --live or --generation <g>")?,
        page_size,
        start_after,
    })
}

fn parse_renormalization(args: &[String]) -> Result<Command, String> {
    let mut mode = None;
    let mut options = RenormalizationOptions {
        mode: RawRevisionReapplicationMode::DryRun,
        listing_source_id: None,
        page_size: NonZeroU32::new(DEFAULT_RENORMALIZATION_PAGE_SIZE).expect("positive"),
        limit: None,
        after: None,
        max_backlog: DEFAULT_RENORMALIZATION_MAX_BACKLOG,
    };
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--dry-run" if mode.is_none() => mode = Some(RawRevisionReapplicationMode::DryRun),
            "--apply" if mode.is_none() => mode = Some(RawRevisionReapplicationMode::Apply),
            "--listing-source" => {
                options.listing_source_id = Some(
                    value()?
                        .parse::<ListingSourceId>()
                        .map_err(|_| "--listing-source must be a ListingSource ID".to_owned())?,
                );
            }
            "--page-size" => {
                options.page_size = value()?
                    .parse()
                    .map_err(|_| "--page-size must be a positive integer".to_owned())?;
            }
            "--limit" => {
                options.limit = Some(
                    value()?
                        .parse::<std::num::NonZeroU64>()
                        .map_err(|_| "--limit must be a positive integer".to_owned())?
                        .get(),
                );
            }
            "--after" => {
                options.after = Some(
                    value()?
                        .parse::<ProductListingRawStreamId>()
                        .map_err(|_| "--after must be a raw stream ID".to_owned())?,
                );
            }
            "--max-backlog" => {
                options.max_backlog = value()?
                    .parse()
                    .map_err(|_| "--max-backlog must be a non-negative integer".to_owned())?;
            }
            other => return Err(format!("unexpected renormalize-raw argument {other}")),
        }
    }
    // Writing is never a default: it must be chosen explicitly.
    options.mode = mode.ok_or("renormalize-raw needs --dry-run or --apply")?;
    Ok(Command::RenormalizeRaw(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<Command, String> {
        Command::parse(line.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn should_parse_every_command() {
        assert_eq!(
            Ok(Command::Migrate {
                plan: PathBuf::from("plan.json")
            }),
            parse("migrate --plan plan.json")
        );
        for (line, name) in [
            ("inspect-schema", "inspect-schema"),
            ("bootstrap", "bootstrap"),
            ("inspect", "inspect"),
            ("backend-cleanup", "backend-cleanup"),
            ("fx-refresh", "fx-refresh"),
            ("periodic-match", "periodic-match"),
        ] {
            assert_eq!(name, parse(line).unwrap().name());
        }
        assert_eq!(
            Ok(Command::Redrive {
                scope: "geo-resolution".into(),
                selection: Selection::All
            }),
            parse("redrive geo-resolution --all")
        );
        assert_eq!(
            Ok(Command::Redrive {
                scope: OUTBOX.into(),
                selection: Selection::Ids(vec!["7".into(), "9".into()])
            }),
            parse("redrive delivery-outbox 7 9")
        );
        let Ok(Command::ReprojectAll {
            target,
            page_size,
            start_after,
        }) = parse("reproject-all --generation g7 --page-size 50")
        else {
            panic!("reproject-all parses");
        };
        assert_eq!(
            ProjectionTarget::Generation(ProjectionGeneration::new("g7").unwrap()),
            target
        );
        assert_eq!(50, page_size.get());
        assert_eq!(None, start_after);
        let Ok(Command::RenormalizeRaw(options)) = parse(
            "renormalize-raw --apply --listing-source ls_01h455vb4pex5vy7enb1p677vn --limit 1000 \
             --after prs_01h455vb4pex5vy7enb1p677vn --max-backlog 500",
        ) else {
            panic!("renormalize-raw parses");
        };
        assert_eq!(RawRevisionReapplicationMode::Apply, options.mode);
        assert_eq!(
            "ls_01h455vb4pex5vy7enb1p677vn",
            options.listing_source_id.unwrap().to_string()
        );
        assert_eq!(Some(1000), options.limit);
        assert_eq!(
            "prs_01h455vb4pex5vy7enb1p677vn",
            options.after.unwrap().to_string()
        );
        assert_eq!(500, options.max_backlog);
        assert_eq!(DEFAULT_RENORMALIZATION_PAGE_SIZE, options.page_size.get());
        let Ok(Command::RenormalizeRaw(options)) = parse("renormalize-raw --dry-run") else {
            panic!("a dry run parses");
        };
        assert_eq!(RawRevisionReapplicationMode::DryRun, options.mode);
    }

    #[test]
    fn should_reject_ambiguous_or_invalid_arguments_before_connecting() {
        for line in [
            "",
            "unknown",
            "inspect extra",
            "migrate",
            "redrive",
            "redrive geo-resolution",
            "redrive geo-resolution --all 0190",
            "redrive geo-resolution not-a-uuid",
            "redrive delivery-outbox abc",
            "reproject-all",
            "reproject-all --live --generation g1",
            "reproject-all --generation G1",
            "reproject-all --live --page-size 0",
            "reproject-all --live --start-after usr_123",
            "renormalize-raw",
            "renormalize-raw --dry-run --apply",
            "renormalize-raw --apply --limit 0",
            "renormalize-raw --dry-run --page-size 0",
            "renormalize-raw --dry-run --after pl_01h455vb4pex5vy7enb1p677vn",
            "renormalize-raw --dry-run --listing-source prs_01h455vb4pex5vy7enb1p677vn",
            "renormalize-raw --apply --max-backlog -1",
        ] {
            assert!(parse(line).is_err(), "{line}");
        }
    }
}
