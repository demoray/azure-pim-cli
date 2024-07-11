use anyhow::{Context, Result};
use azure_pim_cli::{models::roles::RoleAssignments, ListFilter, PimClient};
use clap::Parser;
use std::{
    io::{stderr, stdin},
    time::Duration,
};
use tracing::{debug, info, level_filters::LevelFilter, warn};
use tracing_subscriber::{fmt, EnvFilter};

/// Cleanup orphaned role assignments for an arbitrary set of subscriptions
#[derive(Parser)]
struct Args {
    /// Automatically answer yes to all prompts
    #[clap(long)]
    yes: bool,
}

fn choice(msg: &str) -> bool {
    info!("Are you sure you want to {msg}? (y/n): ");
    loop {
        let mut input = String::new();
        let Ok(_) = stdin().read_line(&mut input) else {
            continue;
        };
        match input.trim().to_lowercase().as_str() {
            "y" => break true,
            "n" => break false,
            _ => {
                warn!("Please enter 'y' or 'n': ");
            }
        }
    }
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
    total.0.extend(active.0.clone());

    for role_assignment in total.0 {
        if role_assignment.scope.subscription().is_none() {
            continue;
        }

        if !["Owner", "Role Based Access Control Administrator"]
            .contains(&role_assignment.role.0.as_str())
        {
            continue;
        }
        info!("checking {}", role_assignment.scope_name);

        if !active.0.contains(&role_assignment) {
            client.activate_role_assignment(
                &role_assignment,
                "cleaning up orphaned resources",
                Duration::from_secs(60 * 60 * 8),
            )?;
            let mut set = RoleAssignments::default();
            set.insert(role_assignment.clone());
            client.wait_for_role_activation(&set, Duration::from_secs(60 * 5))?;
        }

        let mut objects = client
            .role_assignments(&role_assignment.scope)
            .context("unable to list active assignments")?;
        let definitions = client.role_definitions(&role_assignment.scope)?;
        debug!("{} total entries", objects.len());
        objects.retain(|x| x.object.is_none());
        debug!("{} orphaned entries", objects.len());
        for entry in objects {
            let definition = definitions
                .iter()
                .find(|x| x.id == entry.properties.role_definition_id);
            let value = format!(
                "role:\"{}\" principal:{} (type: {}) scope:{}",
                definition.map_or(entry.name.as_str(), |x| x.properties.role_name.as_str()),
                entry.properties.principal_id,
                entry.properties.principal_type,
                entry.properties.scope
            );
            if !yes && !choice(&format!("delete {value}")) {
                info!("skipping {value}");
                continue;
            }

            client
                .delete_role_assignment(&entry.properties.scope, &entry.name)
                .context("unable to delete assignment")?;
        }
    }

    Ok(())
}
