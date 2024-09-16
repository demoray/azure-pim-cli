use anyhow::{Context, Result};
use azure_pim_cli::{
    check_latest_version,
    graph::ObjectType,
    models::{
        roles::{Role, RoleAssignment},
        scope::{Scope, ScopeBuilder},
    },
    ListFilter, PimClient,
};
use clap::{ArgAction, Args, Parser, ValueEnum};
use csv::{QuoteStyle, WriterBuilder};
use rayon::prelude::*;
use serde::Serialize;
use std::{
    collections::BTreeSet,
    io::{stderr, stdout},
};
use tracing::debug;
use tracing_subscriber::filter::LevelFilter;

#[derive(ValueEnum, Clone)]
enum Format {
    Json,
    Csv,
}

#[derive(Parser)]
#[command(version, disable_help_subcommand = true, name = "dump-roles")]
struct Cmd {
    #[command(flatten)]
    verbose: Verbosity,

    #[clap(flatten)]
    scope: ScopeBuilder,

    #[clap(long)]
    active: bool,

    /// Include nested scopes
    #[clap(long)]
    nested: bool,

    /// Expand groups to include their members
    #[clap(long)]
    expand: bool,

    #[clap(long, default_value = "json")]
    format: Format,
}

#[derive(Serialize, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Entry {
    role: Role,
    scope: Scope,
    id: String,
    display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    upn: Option<String>,
    object_type: ObjectType,
    #[serde(skip_serializing_if = "Option::is_none")]
    via_group: Option<String>,
}

fn output(data: BTreeSet<Entry>, format: &Format) -> Result<()> {
    match format {
        Format::Json => serde_json::to_writer_pretty(stdout(), &data)?,
        Format::Csv => {
            let mut wtr = WriterBuilder::new()
                .quote_style(QuoteStyle::Always)
                .from_writer(stdout());
            for mut entry in data {
                // Ensure all fields have content such that CSV renders appropriately
                entry.upn = Some(entry.upn.take().unwrap_or_default());
                entry.via_group = Some(entry.via_group.take().unwrap_or_default());
                wtr.serialize(entry)?;
            }
            wtr.flush()?;
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    let Cmd {
        verbose,
        scope,
        nested,
        active,
        expand,
        format,
    } = Cmd::parse();

    let filter = if let Ok(x) = tracing_subscriber::EnvFilter::try_from_default_env() {
        x
    } else {
        tracing_subscriber::EnvFilter::builder()
            .with_default_directive(verbose.get_level().into())
            .parse("")?
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(stderr)
        .try_init()
        .ok();

    if let Err(err) = check_latest_version() {
        debug!("unable to check latest version: {err}");
    }

    let scope = scope.build().context("scope required")?;
    let client = PimClient::new()?;

    let mut scopes = [scope.clone()].into_iter().collect::<BTreeSet<_>>();
    if nested {
        scopes.extend(
            client
                .eligible_child_resources(&scope)?
                .into_iter()
                .map(|x| x.id),
        );
    }

    let mut results = BTreeSet::new();
    let result: Vec<(Scope, Result<BTreeSet<RoleAssignment>>)> = scopes
        .into_par_iter()
        .map(|scope| {
            let entries = if active {
                client.list_active_role_assignments(Some(scope.clone()), Some(ListFilter::AtScope))
            } else {
                client
                    .list_eligible_role_assignments(Some(scope.clone()), Some(ListFilter::AtScope))
            };
            (scope.clone(), entries)
        })
        .collect();

    for (scope, assignments) in result {
        for entry in assignments? {
            let Some(object) = entry.object else { continue };
            results.insert(Entry {
                role: entry.role,
                id: object.id,
                display_name: object.display_name,
                upn: object.upn,
                object_type: object.object_type,
                scope: scope.clone(),
                via_group: None,
            });
        }
    }

    if expand {
        let mut expanded = BTreeSet::new();
        for entry in &results {
            if entry.object_type != ObjectType::Group {
                continue;
            }

            let members = client.group_members(&entry.id, true)?;
            for member in members {
                expanded.insert(Entry {
                    role: entry.role.clone(),
                    id: member.id,
                    display_name: member.display_name,
                    upn: member.upn,
                    object_type: member.object_type,
                    scope: entry.scope.clone(),
                    via_group: Some(entry.display_name.clone()),
                });
            }
        }
        results.extend(expanded);
    }

    output(results, &format)
}

#[derive(Args)]
#[command(about = None)]
struct Verbosity {
    /// Increase logging verbosity.  Provide repeatedly to increase the verbosity.
    #[clap(long, action = ArgAction::Count, global = true)]
    verbose: u8,

    /// Only show errors
    #[clap(long, global = true, conflicts_with = "verbose")]
    quiet: bool,
}

impl Verbosity {
    fn get_level(&self) -> LevelFilter {
        if self.quiet {
            LevelFilter::ERROR
        } else {
            match self.verbose {
                0 => LevelFilter::INFO,
                1 => LevelFilter::DEBUG,
                _ => LevelFilter::TRACE,
            }
        }
    }
}
