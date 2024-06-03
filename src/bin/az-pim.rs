use anyhow::{Context, Result};
use azure_pim_cli::{
    az_cli::get_token,
    elevate::{elevate_role, ElevateConfig},
    roles::list,
};
use clap::{Command, CommandFactory, Parser, Subcommand};
use std::{cmp::min, io::stdout};

#[derive(Parser)]
#[command(
    author,
    version,
    propagate_version = true,
    disable_help_subcommand = true
)]
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

    #[command(hide = true)]
    Readme,
}

fn build_readme(cmd: &mut Command, mut names: Vec<String>) -> String {
    let mut readme = String::new();
    let base_name = cmd.get_name().to_owned();

    names.push(base_name);

    // add positions to the display name if there are any
    for positional in cmd.get_positionals() {
        names.push(format!("<{}>", positional.get_id().as_str().to_uppercase()));
    }

    let name = names.join(" ");

    // once we're at 6 levels of nesting, don't nest anymore.  This is the max
    // that shows up on crates.io and GitHub.
    for _ in 0..(min(names.iter().filter(|f| !f.starts_with('<')).count(), 6)) {
        readme.push('#');
    }

    readme.push_str(&format!(
        " {name}\n\n```\n{}\n```\n",
        cmd.render_long_help()
    ));

    for cmd in cmd.get_subcommands_mut() {
        if cmd.get_name() == "readme" {
            continue;
        }
        readme.push_str(&build_readme(cmd, names.clone()));
    }
    readme
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
        SubCommand::Readme => {
            let mut cmd = Cmd::command();
            let readme = build_readme(&mut cmd, Vec::new())
                .replace("azure-pim-cli", "az-pim")
                .replacen(
                    "# az-pim",
                    &format!("# Azure PIM CLI\n\n{}", env!("CARGO_PKG_DESCRIPTION")),
                    1,
                )
                .lines()
                .map(str::trim_end)
                .collect::<Vec<_>>()
                .join("\n")
                .replace("\n\n\n", "\n");
            print!("{readme}");
            Ok(())
        }
    }
}
