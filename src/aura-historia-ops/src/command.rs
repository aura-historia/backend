//! The `ops` command line.

use delivery_outbox::RedriveSelection as OutboxRedriveSelection;
use platform_postgres_queue::{JobId, RedriveSelection};
use platform_worker_queue_contract::job_queue::{ProjectionGeneration, ProjectionTarget};
use product_listing_core::product_listing_id::ProductListingId;
use std::{num::NonZeroU32, path::PathBuf};

/// The scope name that selects the delivery outbox in `redrive`.
pub const OUTBOX: &str = "delivery-outbox";
const DEFAULT_REPROJECTION_PAGE_SIZE: u32 = 500;

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
        ] {
            assert!(parse(line).is_err(), "{line}");
        }
    }
}
