use anyhow::{ensure, Context, Result};
use azure_pim_cli::{
    az_cli::get_userid,
    interactive::{interactive_ui, Action},
    roles::{Assignments, Role, Scope},
    PimClient,
};
use clap::{ArgAction, Args, Command, CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use rayon::{prelude::*, ThreadPoolBuilder};
use serde::{Deserialize, Serialize};
use std::{
    cmp::min, collections::BTreeSet, error::Error, fs::File, io::stdout, path::PathBuf,
    str::FromStr,
};
use tracing::{error, info};
use tracing_subscriber::filter::LevelFilter;

// empirical testing shows we need to keep under 5 concurrent requests to keep
// from rate limiting.  In the future, we may move to a model where we go as
// fast as possible and only slow down once Azure says to do so.
const DEFAULT_CONCURRENCY: usize = 4;

const DEFAULT_DURATION: u32 = 480;

#[derive(Parser)]
#[command(disable_help_subcommand = true, name = "az-pim")]
struct Cmd {
    #[command(flatten)]
    verbose: Verbosity,

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
            "az-pim" | "az-pim interactive" => None,
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
$ az-pim list --active
[
  {
    "role": "Storage Blob Data Contributor",
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000001",
    "scope_name": "contoso-development-2",
  }
]
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
    /// List active or eligible assignments
    List {
        #[clap(long)]
        /// List active assignments
        active: bool,
    },

    /// Activate a specific role
    Activate {
        /// Name of the role to elevate
        role: Role,
        /// Scope to elevate
        scope: Scope,
        /// Justification for the request
        justification: String,
        /// Duration in minutes
        #[clap(long, default_value_t = DEFAULT_DURATION)]
        duration: u32,
    },

    /// Activate a set of roles
    ///
    /// This command can be used to activate multiple roles at once.  It can be
    /// used with a config file or by specifying roles on the command line.
    ActivateSet {
        /// Justification for the request
        justification: String,
        #[clap(long, default_value_t = DEFAULT_DURATION)]
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
        /// Concurrency rate
        ///
        /// Specify how many roles to elevate concurrently.  This can be used to
        /// speed up activation of roles.
        #[clap(long, default_value_t = DEFAULT_CONCURRENCY)]
        concurrency: usize,
    },

    /// Activate roles interactively
    Interactive {
        #[clap(long)]
        /// Justification for the request
        justification: Option<String>,

        /// Concurrency rate
        ///
        /// Specify how many roles to elevate concurrently.  This can be used to
        /// speed up activation of roles.
        #[clap(long, default_value_t = DEFAULT_CONCURRENCY)]
        concurrency: usize,

        #[clap(long, default_value_t = DEFAULT_DURATION)]
        /// Duration in minutes
        duration: u32,
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

pub(crate) fn output<T>(value: &T) -> Result<()>
where
    T: ?Sized + Serialize,
{
    serde_json::to_writer_pretty(stdout(), value).context("unable to serialize results")
}

#[derive(Deserialize)]
struct ElevateEntry {
    role: Role,
    scope: Scope,
}

#[derive(Deserialize)]
struct Roles(Vec<ElevateEntry>);

fn main() -> Result<()> {
    let args = Cmd::parse();

    let filter = if let Ok(x) = tracing_subscriber::EnvFilter::try_from_default_env() {
        x
    } else {
        tracing_subscriber::EnvFilter::builder()
            .with_default_directive(args.verbose.get_level().into())
            .parse("")?
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init()
        .ok();

    let client = PimClient::new()?;

    match args.command {
        SubCommand::List { active } => {
            let roles = if active {
                client.list_active_assignments()?
            } else {
                client.list_eligible_assignments()?
            };
            output(&roles)
        }
        SubCommand::Interactive {
            justification,
            concurrency,
            duration,
        } => {
            let roles = client.list_eligible_assignments()?;
            if let Action::Activate {
                scopes,
                justification,
                duration,
            } = interactive_ui(roles.0, justification, duration)?
            {
                activate_set(
                    &client,
                    &Assignments(scopes),
                    &justification,
                    duration,
                    concurrency,
                )?;
            }
            Ok(())
        }
        SubCommand::Activate {
            role,
            scope,
            justification,
            duration,
        } => {
            let roles = client
                .list_eligible_assignments()
                .context("unable to list eligible assignments")?;
            let entry = roles.find(&role, &scope).context("role not found")?;
            info!("activating {} in {}", entry.role, entry.scope_name);
            let principal_id = get_userid().context("unable to obtain the current user")?;

            if let Some(request_id) =
                client.activate_assignment(&principal_id, entry, &justification, duration)?
            {
                info!("submitted request: {request_id}");
            }
            Ok(())
        }
        SubCommand::ActivateSet {
            config,
            role,
            justification,
            duration,
            concurrency,
        } => {
            let set = build_set(&client, config, role)?;
            activate_set(&client, &set, &justification, duration, concurrency)?;
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

fn build_set(
    client: &PimClient,
    config: Option<PathBuf>,
    role: Option<Vec<(Role, Scope)>>,
) -> Result<Assignments> {
    let mut desired_roles = role.unwrap_or_default();

    if let Some(path) = config {
        let handle = File::open(path).context("unable to open activate-set config file")?;
        let Roles(roles) =
            serde_json::from_reader(handle).context("unable to parse config file")?;
        for entry in roles {
            desired_roles.push((entry.role, entry.scope));
        }
    }

    let available = client
        .list_eligible_assignments()
        .context("unable to list available assignments in PIM")?;

    let mut to_add = BTreeSet::new();
    for (role, scope) in desired_roles {
        let entry = available
            .find(&role, &scope)
            .with_context(|| format!("role not found.  role:{role} scope:{scope}"))?;
        to_add.insert(entry);
    }

    Ok(Assignments(to_add.into_iter().cloned().collect()))
}

fn activate_set(
    client: &PimClient,
    assignments: &Assignments,
    justification: &str,
    duration: u32,
    concurrency: usize,
) -> Result<()> {
    ensure!(!assignments.0.is_empty(), "no roles specified");

    let principal_id = get_userid().context("unable to obtain the current user")?;
    ThreadPoolBuilder::new()
        .num_threads(concurrency)
        .build_global()?;

    let results = assignments
        .0
        .par_iter()
        .map(|entry| {
            info!("activating {} in {}", entry.role, entry.scope_name);
            match client.activate_assignment(&principal_id, entry, justification, duration) {
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
