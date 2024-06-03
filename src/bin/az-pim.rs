use anyhow::{Context, Result};
use azure_pim_cli::{
    az_cli::get_token,
    elevate::{elevate_role, ElevateConfig},
    roles::list,
};
use clap::{Parser, Subcommand};
use std::io::stdout;

#[derive(Parser)]
/// CLI to list and enable Azure Privileged Identity Management roles
struct Cmd {
    #[clap(subcommand)]
    commands: SubCommand,
}

#[derive(Subcommand)]
enum SubCommand {
    /// List eligible assignments
    List,

    /// Elevate to a specific role
    Elevate(ElevateConfig),
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or(
                tracing_subscriber::EnvFilter::new(format!("{}=info", env!("CARGO_CRATE_NAME"))),
            ),
        )
        .try_init()
        .ok();

    let args = Cmd::parse();

    let token = get_token().context("unable to obtain access token")?;

    let roles = list(&token)?;
    match args.commands {
        SubCommand::List => {
            serde_json::to_writer_pretty(stdout(), &roles)?;
            Ok(())
        }
        SubCommand::Elevate(config) => {
            elevate_role(&token, &config, &roles)?;
            Ok(())
        }
    }
}
