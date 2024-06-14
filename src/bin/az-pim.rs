use anyhow::{ensure, Context, Result};
use azure_pim_cli::{
    activate::activate_role,
    az_cli::{get_token, get_userid},
    interactive::{interactive_ui, Action},
    roles::{list_roles, Role, Scope},
};
use clap::{Command, CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use rayon::{prelude::*, ThreadPoolBuilder};
use serde::Deserialize;
use std::{
    cmp::min, collections::BTreeSet, error::Error, fs::File, io::stdout, path::PathBuf,
    str::FromStr,
};
use tracing::{error, info};

// empirical testing shows we need to keep under 5 concurrent requests to keep
// from rate limiting.  In the future, we may move to a model where we go as
// fast as possible and only slow down once Azure says to do so.
const CONCURRENCY: usize = 4;

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
            "az-pim" | "az-pim generate" | "az-pim interactive" => None,
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
$ az-pim activate "Storage Blob Data Contributor" "contoso-development-2" "accessing storage data"
2024-06-04T15:35:54.714131Z  INFO az_pim: activating "Storage Blob Data Contributor" in contoso-development-2
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
    "scope": "contoso-development-2",
    "role": "Storage Blob Data Contributor"
  }
]
$ # specifying multiple roles via the command line
$ az-pim activate-set "deploying new code" --role "Storage Blob Data Contributor=/subscriptions/00000000-0000-0000-0000-000000000000" --role "Storage Blob Data Contributor=contoso-development-2"
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
        role: Role,
        /// Scope to elevate
        scope: Scope,
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
        ///             "role": "Owner",
        ///             "scope": "/subscriptions/00000000-0000-0000-0000-000000000000"
        ///         },
        ///         {
        ///             "role": "Owner",
        ///             "scope": "/subscriptions/00000000-0000-0000-0000-000000000001"
        ///         }
        ///     ]
        /// `
        config: Option<PathBuf>,
        #[clap(
            long,
            conflicts_with = "config",
            value_name = "ROLE=SCOPE",
            value_parser = parse_key_val::<String, String>,
            action = clap::ArgAction::Append
        )]
        /// Specify a role to elevate
        ///
        /// Specify multiple times to include multiple key/value pairs
        role: Option<Vec<(Role, Scope)>>,
    },

    /// Activate roles interactively
    Interactive {
        #[clap(long)]
        /// Justification for the request
        justification: Option<String>,
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
struct ElevateEntry {
    role: Role,
    scope: Scope,
}

#[derive(Deserialize)]
struct Roles(Vec<ElevateEntry>);

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or(tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init()
        .ok();

    let args = Cmd::parse();

    match args.command {
        SubCommand::Interactive { justification } => interactive(justification),
        SubCommand::List => list(),
        SubCommand::Activate {
            role,
            scope,
            justification,
            duration,
        } => activate(&role, &scope, &justification, duration),
        SubCommand::ActivateSet {
            config,
            role,
            justification,
            duration,
        } => activate_set(config, role, &justification, duration),
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

fn interactive(justification: Option<String>) -> Result<()> {
    let token = get_token().context("unable to obtain access token")?;
    let roles = list_roles(&token).context("unable to list available roles in PIM")?;
    let action = interactive_ui(roles.0, justification)?;
    match action {
        Action::Activate {
            scopes,
            justification,
        } => {
            let scopes = Some(scopes.into_iter().map(|x| (x.role, x.scope)).collect());
            activate_set(None, scopes, &justification, 480)?;
        }
        Action::Quit => {}
    }

    Ok(())
}

fn list() -> Result<()> {
    let token = get_token().context("unable to obtain access token")?;
    let roles = list_roles(&token).context("unable to list available roles in PIM")?;
    serde_json::to_writer_pretty(stdout(), &roles)?;
    Ok(())
}

fn activate(role: &Role, scope: &Scope, justification: &str, duration: u32) -> Result<()> {
    let principal_id = get_userid().context("unable to obtain the current user")?;
    let token = get_token().context("unable to obtain access token")?;
    let roles = list_roles(&token).context("unable to list available roles in PIM")?;
    let entry = roles.find(role, scope).context("role not found")?;

    info!("activating {} in {}", entry.role, entry.scope_name);
    if let Some(request_id) = activate_role(&principal_id, &token, entry, justification, duration)
        .context("unable to elevate to specified role")?
    {
        info!("submitted request: {request_id}");
    }

    Ok(())
}

fn activate_set(
    config: Option<PathBuf>,
    role: Option<Vec<(Role, Scope)>>,
    justification: &str,
    duration: u32,
) -> Result<()> {
    let mut desired_roles = role.unwrap_or_default();

    if let Some(path) = config {
        let handle = File::open(path).context("unable to open activate-set config file")?;
        let Roles(roles) =
            serde_json::from_reader(handle).context("unable to parse config file")?;
        for entry in roles {
            desired_roles.push((entry.role, entry.scope));
        }
    }

    ensure!(!desired_roles.is_empty(), "no roles specified");

    let principal_id = get_userid().context("unable to obtain the current user")?;
    let token = get_token().context("unable to obtain access token")?;
    let available = list_roles(&token).context("unable to list available roles in PIM")?;

    let mut to_add = BTreeSet::new();
    for (role, scope) in &desired_roles {
        let entry = available
            .find(role, scope)
            .with_context(|| format!("role not found.  role:{role} scope:{scope}"))?;
        to_add.insert(entry);
    }

    ThreadPoolBuilder::new()
        .num_threads(CONCURRENCY)
        .build_global()?;

    // let mut success = true;
    let results = to_add
        .par_iter()
        .map(|entry| {
            info!("activating {} in {}", entry.role, entry.scope_name);
            match activate_role(&principal_id, &token, entry, justification, duration) {
                Ok(Some(request_id)) => {
                    info!("submitted request: {request_id}");
                    true
                }
                Ok(None) => true,
                Err(error) => {
                    error!(
                        "scope: {} definition: {} error: {error:?}",
                        entry.scope, entry.role_definition_id
                    );
                    false
                }
            }
        })
        .collect::<Vec<_>>();

    ensure!(results.iter().all(|x| *x), "unable to elevate to all roles");

    Ok(())
}
