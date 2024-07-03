use anyhow::{Context, Result};
use azure_pim_cli::{
    check_latest_version,
    interactive::{interactive_ui, Selected},
    roles::{Assignments, Role, Scope},
    PimClient,
};
use clap::{ArgAction, Args, Command, CommandFactory, Parser, Subcommand, ValueHint};
use clap_complete::{generate, Shell};
use humantime::Duration as HumanDuration;
use serde::{Deserialize, Serialize};
use std::{
    cmp::min,
    collections::BTreeSet,
    error::Error,
    fs::File,
    io::{stderr, stdout},
    path::PathBuf,
    str::FromStr,
    time::Duration,
};
use tracing::debug;
use tracing_subscriber::filter::LevelFilter;

// empirical testing shows we need to keep under 5 concurrent requests to keep
// from rate limiting.  In the future, we may move to a model where we go as
// fast as possible and only slow down once Azure says to do so.
const DEFAULT_CONCURRENCY: usize = 4;

const DEFAULT_DURATION: &str = "8 hours";

#[derive(Parser)]
#[command(version, disable_help_subcommand = true, name = "az-pim")]
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
            "az-pim"
            | "az-pim activate"
            | "az-pim activate interactive"
            | "az-pim deactivate"
            | "az-pim deactivate interactive" => None,
            "az-pim list" => Some(
                r#"
$ az-pim list
[
  {
    "role": "Owner",
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000000",
    "scope_name": "My Subscription"
  },
  {
    "role": "Storage Blob Data Contributor",
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000000",
    "scope_name": "My Subscription"
  }
]
$ az-pim list --active
[
  {
    "role": "Storage Blob Data Contributor",
    "scope": "/subscriptions/00000000-0000-0000-0000-000000000000",
    "scope_name": "My Subscription"
  }
]
$
"#,
            ),
            "az-pim activate role <ROLE> <SCOPE> <JUSTIFICATION>" => Some(
                r#"
$ az-pim activate role Owner "My Subscription" "developing pim"
2024-06-27T16:55:27.676291Z  INFO az_pim: activating Owner in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$
"#,
            ),
            "az-pim activate set <JUSTIFICATION>" => Some(
                r#"
$ az-pim activate set 'continued development' --role 'Owner=My Subscription'
2024-06-27T17:23:03.981067Z  INFO azure_pim_cli: activating Owner in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$ cat config.json
[
  {
    "role": "Owner",
    "scope_name": "My Subscription"
  },
  {
    "role": "Storage Blob Data Contributor",
    "scope_name": "My Subscription"
  }
]
$ az-pim activate set 'continued development' --config ./config.json
2024-06-27T17:23:03.981067Z  INFO azure_pim_cli: activating Owner in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
2024-06-27T17:23:03.981067Z  INFO azure_pim_cli: activating Storabe Blob Data Contributor in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$ az-pim list | jq 'map(select(.role | contains("Contributor")))' | az-pim activate set "deploying new code" --config /dev/stdin
2024-06-27T17:23:03.981067Z  INFO azure_pim_cli: activating Storabe Blob Data Contributor in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$
"#,
            ),
            "az-pim deactivate role <ROLE> <SCOPE>" => Some(
                r#"
$ az-pim deactivate role "Storage Queue Data Contributor" "My Subscription"
2024-06-27T17:57:53.462674Z  INFO az_pim: deactivating Storage Queue Data Contributor in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$
                "#,
            ),
            "az-pim deactivate set" => Some(
                r#"
$ az-pim deactivate set --role "Owner=My Subscription"
2024-06-27T17:57:53.462674Z  INFO az_pim: deactivating Owner in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$ # deactivate all roles by listing active roles, then deactivating all of them
$ az-pim list | az-pim deactivate set --config /dev/stdin
2024-06-27T17:57:53.462674Z  INFO az_pim: deactivating Storage Blob Data Contributor in My Subscription (/subscriptions/00000000-0000-0000-0000-000000000000)
$
                "#,
            ),
            "az-pim init <SHELL>" => Some(
                r"
$ # In bash shell
$ eval $(az-pim init bash)
$ # In zsh shell
$ source <(az-pim init zsh)
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

    /// Activate roles
    Activate {
        #[clap(subcommand)]
        cmd: ActivateSubCommand,
    },

    /// Deactivate roles
    Deactivate {
        #[clap(subcommand)]
        cmd: DeactivateSubCommand,
    },

    /// Setup shell tab completions
    ///
    /// This command will generate shell completions for the specified shell.
    Init { shell: Shell },

    #[command(hide = true)]
    /// Generate the README.md file dynamically
    Readme,
}

#[derive(Subcommand, Debug)]
enum ActivateSubCommand {
    /// Activate a specific role
    Role {
        /// Name of the role to activate
        role: Role,

        /// Scope to activate
        scope: Scope,

        /// Justification for the request
        justification: String,

        #[clap(long, default_value = DEFAULT_DURATION)]
        /// Duration for the role to be active
        ///
        /// Examples include '8h', '8 hours', '1h30m', '1 hour 30 minutes', '1h30m'
        duration: HumanDuration,

        #[clap(long)]
        /// Duration to wait for the roles to be activated
        ///
        /// Examples include '8h', '8 hours', '1h30m', '1 hour 30 minutes', '1h30m'
        wait: Option<HumanDuration>,
    },

    /// Activate a set of roles
    ///
    /// This command can be used to activate multiple roles at once.  It can be
    /// used with a config file or by specifying roles on the command line.
    Set {
        /// Justification for the request
        justification: String,

        #[clap(long, default_value = DEFAULT_DURATION)]
        /// Duration for the role to be active
        ///
        /// Examples include '8h', '8 hours', '1h30m', '1 hour 30 minutes', '1h30m'
        duration: HumanDuration,

        #[clap(long, value_hint = ValueHint::FilePath)]
        /// Path to a JSON config file containing a set of roles to activate
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
            value_parser = parse_key_val::<Role, Scope>,
            action = clap::ArgAction::Append
        )]
        /// Specify a role to activate
        ///
        /// Specify multiple times to include multiple key/value pairs
        role: Option<Vec<(Role, Scope)>>,

        /// Concurrency rate
        ///
        /// Specify how many roles to activate concurrently.  This can be used to
        /// speed up activation of roles.
        #[clap(long, default_value_t = DEFAULT_CONCURRENCY)]
        concurrency: usize,

        #[clap(long)]
        /// Duration to wait for the roles to be activated
        ///
        /// Examples include '8h', '8 hours', '1h30m', '1 hour 30 minutes', '1h30m'
        wait: Option<HumanDuration>,
    },

    /// Activate roles interactively
    Interactive {
        #[clap(long)]
        /// Justification for the request
        justification: Option<String>,

        /// Concurrency rate
        ///
        /// Specify how many roles to activate concurrently.  This can be used to
        /// speed up activation of roles.
        #[clap(long, default_value_t = DEFAULT_CONCURRENCY)]
        concurrency: usize,

        #[clap(long, default_value = DEFAULT_DURATION)]
        /// Duration for the role to be active
        ///
        /// Examples include '8h', '8 hours', '1h30m', '1 hour 30 minutes', '1h30m'
        duration: HumanDuration,

        #[clap(long)]
        /// Duration to wait for the roles to be activated
        ///
        /// Examples include '8h', '8 hours', '1h30m', '1 hour 30 minutes', '1h30m'
        wait: Option<HumanDuration>,
    },
}

impl ActivateSubCommand {
    fn run(self) -> Result<()> {
        match self {
            Self::Role {
                role,
                scope,
                justification,
                duration,
                wait,
            } => {
                let client = PimClient::new()?;
                let roles = client
                    .list_eligible_assignments()
                    .context("unable to list eligible assignments")?;
                let entry = roles
                    .find(&role, &scope)
                    .with_context(|| format!("role not found ({role:?} {scope:?})"))?;
                client.activate_assignment(entry, &justification, duration.into())?;

                if let Some(wait) = wait {
                    client
                        .wait_for_activation(&Assignments([entry.clone()].into()), wait.into())?;
                }
            }
            Self::Set {
                config,
                role,
                justification,
                duration,
                concurrency,
                wait,
            } => {
                let client = PimClient::new()?;
                let set = build_set(&client, config, role, false)?;
                client.activate_assignment_set(
                    &set,
                    &justification,
                    duration.into(),
                    concurrency,
                )?;

                if let Some(wait) = wait {
                    client.wait_for_activation(&set, wait.into())?;
                }
            }
            Self::Interactive {
                justification,
                concurrency,
                duration,
                wait,
            } => {
                let client = PimClient::new()?;
                let roles = client.list_eligible_assignments()?;
                if let Some(Selected {
                    assignments,
                    justification,
                    duration,
                }) = interactive_ui(
                    roles,
                    Some(justification.unwrap_or_default()),
                    Some(duration.as_secs() / 60),
                )? {
                    let duration = Duration::from_secs(duration * 60);
                    client.activate_assignment_set(
                        &assignments,
                        &justification,
                        duration,
                        concurrency,
                    )?;

                    if let Some(wait) = wait {
                        client.wait_for_activation(&assignments, wait.into())?;
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Subcommand)]
enum DeactivateSubCommand {
    /// Deactivate a specific role
    Role {
        /// Name of the role to deactivate
        role: Role,
        /// Scope to deactivate
        scope: Scope,
    },
    /// Deactivate a set of roles
    Set {
        #[clap(long, value_hint = ValueHint::FilePath)]
        /// Path to a JSON config file containing a set of roles to deactivate
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
            value_parser = parse_key_val::<Role, Scope>,
            action = clap::ArgAction::Append
        )]

        /// Specify a role to deactivate
        ///
        /// Specify multiple times to include multiple key/value pairs
        role: Option<Vec<(Role, Scope)>>,

        /// Concurrency rate
        ///
        /// Specify how many roles to deactivate concurrently.  This can be used to
        /// speed up activation of roles.
        #[clap(long, default_value_t = DEFAULT_CONCURRENCY)]
        concurrency: usize,
    },
    /// Deactivate roles interactively
    Interactive {
        /// Concurrency rate
        ///
        /// Specify how many roles to deactivate concurrently.  This can be used to
        /// speed up deactivation of roles.
        #[clap(long, default_value_t = DEFAULT_CONCURRENCY)]
        concurrency: usize,
    },
}

impl DeactivateSubCommand {
    fn run(self) -> Result<()> {
        match self {
            Self::Role { role, scope } => {
                let client = PimClient::new()?;
                let roles = client
                    .list_active_assignments()
                    .context("unable to list active assignments")?;
                let entry = roles.find(&role, &scope).context("role not found")?;
                client.deactivate_assignment(entry)?;
            }
            Self::Set {
                config,
                role,
                concurrency,
            } => {
                let client = PimClient::new()?;
                let set = build_set(&client, config, role, true)?;
                client.deactivate_assignment_set(&set, concurrency)?;
            }
            Self::Interactive { concurrency } => {
                let client = PimClient::new()?;
                let roles = client.list_active_assignments()?;
                if let Some(Selected { assignments, .. }) = interactive_ui(roles, None, None)? {
                    client.deactivate_assignment_set(&assignments, concurrency)?;
                }
            }
        }
        Ok(())
    }
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
        .with_writer(stderr)
        .try_init()
        .ok();

    if let Err(err) = check_latest_version() {
        debug!("unable to check latest version: {err}");
    }

    match args.command {
        SubCommand::List { active } => {
            let client = PimClient::new()?;
            let roles = if active {
                client.list_active_assignments()?
            } else {
                client.list_eligible_assignments()?
            };
            output(&roles)
        }
        SubCommand::Activate { cmd } => cmd.run(),
        SubCommand::Deactivate { cmd } => cmd.run(),
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
    active: bool,
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

    let assignments = if active {
        client
            .list_active_assignments()
            .context("unable to list active assignments in PIM")?
    } else {
        client
            .list_eligible_assignments()
            .context("unable to list available assignments in PIM")?
    };

    let mut to_add = BTreeSet::new();
    for (role, scope) in desired_roles {
        let entry = assignments
            .find(&role, &scope)
            .with_context(|| format!("role not found.  role:{role} scope:{scope}"))?;
        to_add.insert(entry);
    }

    Ok(Assignments(to_add.into_iter().cloned().collect()))
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
