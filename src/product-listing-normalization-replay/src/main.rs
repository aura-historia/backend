//! Replays the retained normalization corpus. See `docs/normalization-replay.md`.
//!
//! ```text
//! normalization-replay [FILTERS] [--page-size N] [--output FILE]   # replay the whole corpus
//! normalization-replay --input FILE.jsonl [--output FILE]          # offline replay of an export
//! normalization-replay export [FILTERS] --output FILE.jsonl        # export cases with evidence
//! normalization-replay seed [--page-size N]                        # seed from raw revisions
//!
//! FILTERS: --entry-point CODE  --listing-source UUID  --case UUID (repeatable)
//!          --field CODE  --reason CODE
//! ```
//!
//! Exit status: 0 when the run covered its whole snapshot (even if cases remain unresolved),
//! 2 when the run is incomplete, 64 for invalid usage.

use product_listing_normalization_replay::{
    DEFAULT_REPLAY_PAGE_SIZE, RunCompletion, export_corpus, replay_corpus, replay_jsonl,
    seed_from_raw_revisions, write_record,
};
use product_listing_postgres::SqlxNormalizationDiagnosticCaseStore;
use product_service::ports::{
    NormalizationDiagnosticCaseFilter, NormalizationDiagnosticCaseId,
    NormalizationDiagnosticEntryPoint,
};
use std::io::{BufReader, Write};
use std::process::ExitCode;

const INCOMPLETE: u8 = 2;
const USAGE: u8 = 64;

#[derive(Debug, Default)]
struct Arguments {
    command: Command,
    filter: NormalizationDiagnosticCaseFilter,
    page_size: Option<u32>,
    input: Option<String>,
    output: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum Command {
    #[default]
    Replay,
    Export,
    Seed,
}

fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Arguments, String> {
    let mut parsed = Arguments::default();
    let mut arguments = arguments.into_iter().peekable();
    match arguments.peek().map(String::as_str) {
        Some("export") => {
            parsed.command = Command::Export;
            arguments.next();
        }
        Some("seed") => {
            parsed.command = Command::Seed;
            arguments.next();
        }
        _ => {}
    }
    while let Some(flag) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--entry-point" => {
                let code = value()?;
                parsed.filter.entry_point = Some(
                    NormalizationDiagnosticEntryPoint::from_code(&code)
                        .ok_or_else(|| format!("unknown entry point {code}"))?,
                );
            }
            "--listing-source" => {
                let id = value()?;
                parsed.filter.listing_source_id = Some(
                    uuid::Uuid::parse_str(&id)
                        .ok()
                        .and_then(|id| listing_source_core::ListingSourceId::try_from(id).ok())
                        .ok_or_else(|| format!("invalid listing source ID {id}"))?,
                );
            }
            "--case" => {
                let id = value()?;
                parsed
                    .filter
                    .case_ids
                    .push(NormalizationDiagnosticCaseId::from_uuid(
                        uuid::Uuid::parse_str(&id).map_err(|_| format!("invalid case ID {id}"))?,
                    ));
            }
            "--field" => parsed.filter.field = Some(value()?),
            "--reason" => parsed.filter.reason = Some(value()?),
            "--page-size" => {
                let size = value()?;
                parsed.page_size = Some(
                    size.parse::<u32>()
                        .ok()
                        .filter(|size| *size > 0)
                        .ok_or_else(|| format!("invalid page size {size}"))?,
                );
            }
            "--input" => parsed.input = Some(value()?),
            "--output" => parsed.output = Some(value()?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if parsed.input.is_some() && parsed.command != Command::Replay {
        return Err("--input is only valid for replay".to_owned());
    }
    if parsed.command == Command::Export && parsed.output.is_none() {
        return Err("export requires --output".to_owned());
    }
    Ok(parsed)
}

#[tokio::main]
async fn main() -> ExitCode {
    let arguments = match parse(std::env::args().skip(1)) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("normalization-replay: {error}");
            return ExitCode::from(USAGE);
        }
    };
    let mut out: Box<dyn Write> = match &arguments.output {
        Some(path) => match std::fs::File::create(path) {
            Ok(file) => Box::new(std::io::BufWriter::new(file)),
            Err(error) => {
                eprintln!("normalization-replay: cannot create {path}: {error}");
                return ExitCode::from(USAGE);
            }
        },
        None => Box::new(std::io::BufWriter::new(std::io::stdout().lock())),
    };
    let page_size = arguments.page_size.unwrap_or(DEFAULT_REPLAY_PAGE_SIZE);

    if let Some(path) = &arguments.input {
        let completion = match std::fs::File::open(path) {
            Ok(file) => replay_jsonl(BufReader::new(file), out.as_mut()),
            Err(error) => {
                eprintln!("normalization-replay: cannot open {path}: {error}");
                return ExitCode::from(USAGE);
            }
        };
        return finish(out.as_mut(), &completion);
    }

    let Ok(database_url) = std::env::var("BUSINESS_DATABASE_URL") else {
        eprintln!("normalization-replay: BUSINESS_DATABASE_URL must be set");
        return ExitCode::from(USAGE);
    };
    let pool = match sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
    {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("normalization-replay: cannot connect to the business database: {error}");
            return ExitCode::from(INCOMPLETE);
        }
    };
    let store = SqlxNormalizationDiagnosticCaseStore::new(pool);
    match arguments.command {
        Command::Replay => {
            let completion = replay_corpus(&store, arguments.filter, page_size, out.as_mut()).await;
            finish(out.as_mut(), &completion)
        }
        Command::Export => {
            let completion = export_corpus(&store, arguments.filter, page_size, out.as_mut()).await;
            eprintln!(
                "normalization-replay: exported {} cases (complete: {})",
                completion.summary.cases,
                completion.completed()
            );
            if let Err(error) = out.flush() {
                eprintln!("normalization-replay: {error}");
                return ExitCode::from(INCOMPLETE);
            }
            exit_code(completion.completed())
        }
        Command::Seed => {
            let completion = seed_from_raw_revisions(&store, &store, page_size).await;
            let record = completion.to_json();
            let written = write_record(out.as_mut(), &record)
                .and_then(|()| out.flush().map_err(|error| error.to_string()));
            eprintln!("{record}");
            exit_code(completion.error.is_none() && written.is_ok())
        }
    }
}

fn finish(out: &mut dyn Write, completion: &RunCompletion) -> ExitCode {
    let summary = completion
        .summary
        .to_json(completion.completed(), completion.error.as_deref());
    let written = write_record(out, &summary).and_then(|()| out.flush().map_err(|e| e.to_string()));
    eprintln!("{summary}");
    exit_code(completion.completed() && written.is_ok())
}

fn exit_code(completed: bool) -> ExitCode {
    if completed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(INCOMPLETE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Result<Arguments, String> {
        parse(values.iter().map(|value| (*value).to_owned()))
    }

    #[test]
    fn no_selector_replays_the_whole_corpus() {
        let parsed = args(&[]).unwrap();
        assert_eq!(Command::Replay, parsed.command);
        assert_eq!(NormalizationDiagnosticCaseFilter::default(), parsed.filter);
    }

    #[test]
    fn filters_and_modes_parse() {
        let parsed = args(&[
            "export",
            "--entry-point",
            "RAW_VALUES_NORMALIZATION_V1",
            "--reason",
            "PRICE_NEGATIVE",
            "--output",
            "cases.jsonl",
        ])
        .unwrap();
        assert_eq!(Command::Export, parsed.command);
        assert_eq!(
            Some(NormalizationDiagnosticEntryPoint::RawValuesNormalizationV1),
            parsed.filter.entry_point
        );
        assert!(args(&["export"]).is_err());
        assert!(args(&["seed", "--input", "x"]).is_err());
        assert!(args(&["--page-size", "0"]).is_err());
    }
}
