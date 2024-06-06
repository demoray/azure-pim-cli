use anyhow::{bail, ensure, Context, Result};
use azure_pim_cli::{
    activate::activate_role,
    az_cli::{get_token, get_userid},
    roles::list_roles,
};
use clap::{Command, CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use serde::Deserialize;
use std::{
    cmp::min, collections::BTreeSet, error::Error, fs::File, io::stdout, path::PathBuf,
    str::FromStr,
};
use tracing::{error, info};

#[derive(Parser)]
#[command(disable_help_subcommand = true, name = "az-pim")]
struct Cmd {
    #[clap(subcommand)]
    command: SubCommand,
}

impl Cmd {
    fn shell_completion(shell: Shell) {
        let mut cmd = Self::command();
        let name = cmd.get_name().to_string();
        generate(shell, &mut cmd, name, &mut stdout());
    }

    fn example(cmd: &str) -> Option<&'static str> {
        match cmd {
            "az-pim" | "az-pim generate" => None,
            "az-pim list" => Some(
                r#"
$ az-pim list
[
  {
    "role": "Storage Blob Data Contributor",
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000000",
    "scope_name": "contoso-development",
  },
  {
    "role": "Storage Blob Data Contributor",
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000001",
    "scope_name": "contoso-development-2",
  }
]
$
"#,
            ),
            "az-pim activate <ROLE> <SCOPE> <JUSTIFICATION>" => Some(
                r#"
$ az-pim activate "Storage Blob Data Contributor" "/subscriptions/00000000-0000-0000-0000-000000000000" "accessing storage data"
2024-06-04T15:35:50.330623Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development
$
"#,
            ),
            "az-pim activate-set <JUSTIFICATION>" => Some(
                r#"
$ # specifying multiple roles using a configuration file
$ az-pim activate-set "deploying new code" --config roles.json
2024-06-04T15:22:03.1051Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development
2024-06-04T15:22:07.25Z    INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development-2
$ cat roles.json
[
  {
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000000",
    "role": "Storage Blob Data Contributor"
  },
  {
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000001",
    "role": "Storage Blob Data Contributor"
  }
]
$ # specifying multiple roles via the command line
$ az-pim activate-set "deploying new code" --role "/subscriptions/00000000-0000-0000-0000-000000000001=Storage Blob Data Contributor" --role "/subscriptions/00000000-0000-0000-0000-000000000001=Storage Blob Data Contributor"
2024-06-04T15:21:39.9341Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development
2024-06-04T15:21:43.1522Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development-2
$ # use `jq` to select roles to activate from the current role assignments
$ az-pim list | jq 'map(select(.role | contains("Contributor")))' | az-pim activate-set "deploying new code" --config /dev/stdin
2024-06-04T18:47:15.489917Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development
2024-06-04T18:47:20.510941Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development-2
$
"#,
            ),
            "az-pim init <SHELL>" => Some(
                r"
* bash: `eval $(az-pim init bash)`
* zsh: `source <(az-pim init zsh)`
",
            ),
            unsupported => unimplemented!("unable to generate example for {unsupported}"),
        }
    }
}

#[derive(Subcommand)]
enum SubCommand {
    /// List eligible assignments
    List,

    /// Activate a specific role
    ///
    /// Example usage:
    /// ```
    /// ```
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
        /// `
        ///     [
        ///         {
        ///             "scope": "/subscriptions/00000000-0000-0000-0000-000000000000",
        ///             "role": "Owner"
        ///         },
        ///         {
        ///             "scope": "/subscriptions/00000000-0000-0000-0000-000000000001",
        ///             "role": "Owner"
        ///         }
        ///     ]
        /// `
        config: Option<PathBuf>,
        #[clap(long, conflicts_with = "config", value_name = "SCOPE=NAME", value_parser = parse_key_val::<String, String>, action = clap::ArgAction::Append)]
        /// Specify a role to elevate
        ///
        /// Specify multiple times to include multiple key/value pairs
        role: Option<Vec<(String, String)>>,
    },

    /// Setup shell tab completions
    ///
    /// This command will generate shell completions for the specified shell.
    Init { shell: Shell },

    #[command(hide = true)]
    /// Generate the README.md file dynamically
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

fn build_readme_entry(cmd: &mut Command, mut names: Vec<String>) -> String {
    let mut readme = String::new();
    let current = cmd.get_name().to_string();

    names.push(current);

    // add positions to the display name if there are any
    for positional in cmd.get_positionals() {
        names.push(format!("<{}>", positional.get_id().as_str().to_uppercase()));
    }

    let name = names.join(" ");

    // once we're at 6 levels of nesting, don't nest anymore.  This is the max
    // that shows up on crates.io and GitHub.
    let depth = min(names.iter().filter(|f| !f.starts_with('<')).count(), 5);
    for _ in 0..depth {
        readme.push('#');
    }

    let long_help = cmd.render_long_help().to_string().replace("```", "\n```\n");
    readme.push_str(&format!(" {name}\n\n```\n{long_help}\n```\n",));

    if let Some(example) = Cmd::example(&name) {
        for _ in 0..=depth {
            readme.push('#');
        }
        readme.push_str(&format!(
            " Example Usage\n\n```\n{}\n```\n\n",
            example.trim()
        ));
    }

    for cmd in cmd.get_subcommands_mut() {
        if cmd.get_name() == "readme" {
            continue;
        }
        readme.push_str(&build_readme_entry(cmd, names.clone()));
    }
    readme
}

fn build_readme() {
    let mut cmd = Cmd::command();
    let readme = build_readme_entry(&mut cmd, Vec::new())
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

    match args.command {
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
            let principal_id = get_userid().context("unable to obtain the current user")?;
            let token = get_token().context("unable to obtain access token")?;
            let roles = list_roles(&token).context("unable to list available roles in PIM")?;
            let entry = roles
                .iter()
                .find(|v| v.role == role && v.scope == scope)
                .context("role not found")?;

            info!("activating {role:?} in {}", entry.scope_name);

            activate_role(
                &principal_id,
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
                let handle = File::open(path).context("unable to open activate-set config file")?;
                let Roles(roles) =
                    serde_json::from_reader(handle).context("unable to parse config file")?;
                for entry in roles {
                    desired_roles.insert((entry.scope, entry.role));
                }
            }

            if desired_roles.is_empty() {
                bail!("no roles specified");
            }

            let principal_id = get_userid().context("unable to obtain the current user")?;
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
                if let Err(error) = activate_role(
                    &principal_id,
                    &token,
                    scope,
                    role_definition_id,
                    &justification,
                    duration,
                ) {
                    error!("scope: {scope} definition: {role_definition_id} error: {error:?}");
                    success = false;
                }
            }

            ensure!(success, "unable to elevate to all roles");

            Ok(())
        }
        SubCommand::Readme => {
            build_readme();
            Ok(())
        }
        SubCommand::Init { shell } => {
            Cmd::shell_completion(shell);
            Ok(())
        }
    }
}
