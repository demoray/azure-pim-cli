use anyhow::{bail, Context, Result};
use azure_pim_cli::{activate::activate_role, az_cli::get_token, roles::list_roles};
use clap::{Command, CommandFactory, Parser, Subcommand};
use serde::Deserialize;
use std::{
    cmp::min, collections::BTreeSet, error::Error, fs::File, io::stdout, path::PathBuf,
    str::FromStr,
};
use tracing::{error, info};

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

    /// Activate a specific role
    Activate {
        /// Name of the role to elevate
        role: String,
        /// Scope to elevate
        scope: String,
        /// Justification for the request
        justification: String,
        /// Duration in minutes
        #[clap(long, default_value_t = 480)]
        duration: u32,
    },

    /// Activate a set of roles
    /// 
    /// This command can be used to activate multiple roles at once.  It can be
    /// used with a config file or by specifying roles on the command line.
    ActivateSet {
        /// Justification for the request
        justification: String,
        #[clap(long, default_value_t = 480)]
        /// Duration in minutes
        duration: u32,
        #[clap(long)]
        /// Path to a JSON config file containing a set of roles to elevate
        ///
        /// Example config file:
        /// [
        ///     {
        ///         "scope": "/subscriptions/00000000-0000-0000-0000-000000000000",
        ///         "role": "Owner"
        ///     },
        ///     {
        ///         "scope": "/subscriptions/00000000-0000-0000-0000-000000000001",
        ///         "role": "Owner"
        ///     }
        /// ]
        config: Option<PathBuf>,
        #[clap(long, conflicts_with = "config", value_name = "SCOPE=NAME", value_parser = parse_key_val::<String, String>, action = clap::ArgAction::Append)]
        /// Specify a role to elevate
        ///
        /// Specify multiple times to include multiple key/value pairs
        role: Option<Vec<(String, String)>>,
    },

    #[command(hide = true)]
    Readme,
}

/// Parse a single key-value pair of `X=Y` into a typed tuple of `(X, Y)`.
///
/// # Errors
/// Returns an `Err` if any of the keys or values cannot be parsed or if no `=` is found.
pub fn parse_key_val<T, U>(s: &str) -> Result<(T, U), Box<dyn Error + Send + Sync + 'static>>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
    U: FromStr,
    U::Err: Error + Send + Sync + 'static,
{
    if let Some((key, value)) = s.split_once('=') {
        Ok((key.parse()?, value.parse()?))
    } else {
        Err(format!("invalid KEY=value: no `=` found in `{s}`").into())
    }
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

#[derive(Deserialize)]
struct Role {
    scope: String,
    role: String,
}

#[derive(Deserialize)]
struct Roles(Vec<Role>);

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

    match args.commands {
        SubCommand::List => {
            let token = get_token().context("unable to obtain access token")?;
            let roles = list_roles(&token).context("unable to list available roles in PIM")?;
            serde_json::to_writer_pretty(stdout(), &roles)?;
            Ok(())
        }
        SubCommand::Activate {
            role,
            scope,
            justification,
            duration,
        } => {
            let token = get_token().context("unable to obtain access token")?;
            let roles = list_roles(&token).context("unable to list available roles in PIM")?;
            let entry = roles
                .iter()
                .find(|v| v.role == role && v.scope == scope)
                .context("role not found")?;

            info!("activating {role:?} in {}", entry.scope_name);

            activate_role(
                &token,
                &entry.scope,
                &entry.role_definition_id,
                &justification,
                duration,
            )
            .context("unable to elevate to specified role")?;
            Ok(())
        }
        SubCommand::ActivateSet {
            config,
            role,
            justification,
            duration,
        } => {
            let mut desired_roles = role
                .unwrap_or_default()
                .into_iter()
                .collect::<BTreeSet<_>>();

            if let Some(path) = config {
                let handle =
                    File::open(&path).context("unable to open activate-set config file")?;
                let Roles(roles) =
                    serde_json::from_reader(handle).context("unable to parse config file")?;
                for entry in roles {
                    desired_roles.insert((entry.scope, entry.role));
                }
            }

            if desired_roles.is_empty() {
                bail!("no roles specified");
            }

            let token = get_token().context("unable to obtain access token")?;
            let available = list_roles(&token).context("unable to list available roles in PIM")?;

            let mut to_add = BTreeSet::new();
            for (scope, role) in &desired_roles {
                let entry = &available
                    .iter()
                    .find(|v| &v.role == role && &v.scope == scope)
                    .with_context(|| format!("role not found.  role:{role} scope:{scope}"))?;

                to_add.insert((scope, role, &entry.role_definition_id, &entry.scope_name));
            }

            let mut success = true;
            for (scope, role, role_definition_id, scope_name) in to_add {
                info!("activating {role:?} in {scope_name}");
                if let Err(error) =
                    activate_role(&token, scope, role_definition_id, &justification, duration)
                {
                    error!(
                        "scope: {scope} role_definition_id: {role_definition_id} error: {error:?}"
                    );
                    success = false;
                }
            }

            if !success {
                bail!("unable to elevate to all roles");
            }

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
