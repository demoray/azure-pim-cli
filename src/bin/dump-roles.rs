use anyhow::{Context, Result};
use azure_pim_cli::{
    check_latest_version,
    graph::PrincipalType,
    models::{
        roles::{Role, RoleAssignment},
        scope::{Scope, ScopeBuilder},
    },
    ListFilter, PimClient,
};
use clap::{ArgAction, Args, CommandFactory, Parser};
use rayon::prelude::*;
use serde::Serialize;
use std::{
    collections::BTreeSet,
    io::{stderr, stdout},
};
use tracing::debug;
use tracing_subscriber::filter::LevelFilter;

/// A CLI to dump all the roles in a given scope
#[derive(Parser)]
#[command(version, disable_help_subcommand = true, name = "dump-roles")]
struct Cmd {
    #[command(flatten)]
    verbose: Verbosity,

    #[clap(flatten)]
    scope: ScopeBuilder,

    /// Show role assignments that are eligibile to be activated rather than active assignments
    #[clap(long)]
    eligible: bool,

    /// Expand groups to include their members
    #[clap(long)]
    expand_groups: bool,
}

impl Cmd {
    pub fn build() -> Result<Self> {
        let help = r#"Examples:
# Find users that have an assignment but don't start with "sc-"
$ dump-roles --subscription 00000000-0000-0000-0000-000000000000 --expand-groups | jq '[.[]| select(.principal_type | contains("User"))] | [.[]| select(.upn | ascii_downcase | contains("sc-") | not)]'

# Find users that can elevate to Owner
$ dump-roles --subscription 00000000-0000-0000-0000-000000000000 --expand-groups --eligible | jq '[.[]| select(.principal_type | contains("User"))] | [.[] | select(.role | contains("Owner"))]'
"#;

        let mut result = Cmd::command();
        result = result.after_long_help(help);

        let mut matches = result.get_matches();
        Ok(<Self as clap::FromArgMatches>::from_arg_matches_mut(
            &mut matches,
        )?)
    }
}

#[derive(Serialize, Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
struct Entry {
    role: Role,
    scope: Scope,
    id: String,
    display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    upn: Option<String>,
    principal_type: PrincipalType,
    #[serde(skip_serializing_if = "Option::is_none")]
    via_group: Option<String>,
}

impl Entry {
    fn is_dominated(&self, other: &Self) -> bool {
        self.id == other.id && self.role == other.role && other.scope.contains(&self.scope)
    }
}

fn main() -> Result<()> {
    let Cmd {
        verbose,
        scope,
        eligible,
        expand_groups,
    } = Cmd::build()?;

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

    let mut scopes = client
        .eligible_child_resources(&scope, true)?
        .into_iter()
        .map(|x| x.id)
        .collect::<BTreeSet<_>>();
    scopes.insert(scope);

    let mut results = BTreeSet::new();
    let result: Vec<(Scope, Result<BTreeSet<RoleAssignment>>)> = scopes
        .into_par_iter()
        .map(|scope| {
            let entries = if eligible {
                client
                    .list_eligible_role_assignments(Some(scope.clone()), Some(ListFilter::AtScope))
            } else {
                client.list_active_role_assignments(Some(scope.clone()), Some(ListFilter::AtScope))
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
                principal_type: object.object_type,
                scope: scope.clone(),
                via_group: None,
            });
        }
    }

    if expand_groups {
        let mut expanded = BTreeSet::new();
        for entry in &results {
            if entry.principal_type != PrincipalType::Group {
                continue;
            }

            let members = client.group_members(&entry.id, true)?;
            for member in members {
                expanded.insert(Entry {
                    role: entry.role.clone(),
                    id: member.id,
                    display_name: member.display_name,
                    upn: member.upn,
                    principal_type: member.object_type,
                    scope: entry.scope.clone(),
                    via_group: Some(entry.display_name.clone()),
                });
            }
        }
        results.extend(expanded);
    }

    let results = remove_dominated_scopes(results);

    serde_json::to_writer_pretty(stdout(), &results)?;
    Ok(())
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

fn remove_dominated_scopes(data: BTreeSet<Entry>) -> BTreeSet<Entry> {
    let mut results = BTreeSet::new();
    let mut rest = BTreeSet::new();

    for entry in data {
        if entry.scope.is_subscription() {
            results.insert(entry);
        } else {
            rest.insert(entry);
        }
    }

    for entry in rest {
        if !results.iter().any(|x| entry.is_dominated(x)) {
            results.insert(entry);
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    #[test]
    fn remove_dominated() {
        let base = Entry {
            scope: Scope::from_subscription(&Uuid::nil()),
            role: Role("Contributor".to_string()),
            id: "1".to_string(),
            display_name: "User 1".to_string(),
            upn: Some("wut".to_string()),
            principal_type: PrincipalType::User,
            via_group: None,
        };

        let mut dominated = base.clone();
        dominated.scope = Scope::from_resource_group(&Uuid::nil(), "rg");
        let mut other_user = dominated.clone();
        other_user.id = "2".to_string();

        let entries = [base.clone(), dominated.clone(), other_user.clone()]
            .into_iter()
            .collect::<BTreeSet<_>>();

        println!("before {entries:#?}");
        let results = remove_dominated_scopes(entries);
        println!("after {results:#?}");
        assert!(results.contains(&base));
        assert!(results.contains(&other_user));
        assert!(!results.contains(&dominated));
    }
}
