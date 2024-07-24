use anyhow::Result;
use azure_pim_cli::{ListFilter, PimClient};
use clap::Parser;
use std::{collections::BTreeSet, io::stderr, time::Duration};
use tracing::{info, level_filters::LevelFilter};
use tracing_subscriber::{fmt, EnvFilter};

/// Cleanup orphaned role assignments for an arbitrary set of subscriptions
#[derive(Parser)]
struct Args {
    /// Automatically answer yes to all prompts
    #[clap(long)]
    yes: bool,
}

fn main() -> Result<()> {
    fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env()?,
        )
        .with_writer(stderr)
        .init();

    let Args { yes } = Args::parse();

    let client = PimClient::new()?;
    let active = client.list_active_role_assignments(None, Some(ListFilter::AsTarget))?;
    let mut total = client.list_eligible_role_assignments(None, Some(ListFilter::AsTarget))?;
    total.extend(active.clone());

    let mut to_activate = BTreeSet::new();

    let mut scopes = BTreeSet::new();
    for role_assignment in total {
        if role_assignment.scope.subscription().is_none() {
            continue;
        }

        if !["Owner", "Role Based Access Control Administrator"]
            .contains(&role_assignment.role.0.as_str())
        {
            continue;
        }

        info!("checking {}", role_assignment.scope_name);

        if !active.contains(&role_assignment) {
            to_activate.insert(role_assignment.clone());
        }

        scopes.insert(role_assignment.scope);
    }

    if !to_activate.is_empty() {
        client.activate_role_assignment_set(
            &to_activate,
            "cleaning up orphaned resources",
            Duration::from_secs(60 * 60 * 8),
            5,
        )?;
        client.wait_for_role_activation(&to_activate, Duration::from_secs(60 * 5))?;
    }

    for scope in scopes {
        info!("deleting orphaned role assignments for {scope}");
        client.delete_orphaned_role_assignments(&scope, yes, true)?;
        info!("deleting orphaned eligible role assignments for {scope}");
        client.delete_orphaned_eligible_role_assignments(&scope, yes, true)?;
    }

    Ok(())
}
