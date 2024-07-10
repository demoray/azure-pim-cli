use anyhow::{bail, Context, Result};
use azure_pim_cli::{
    assignments::Assignment,
    check_latest_version,
    interactive::{interactive_ui, Selected},
    roles::{Role, RoleAssignments},
    scope::Scope,
    ListFilter, PimClient,
};
use clap::{ArgAction, Args, Command, CommandFactory, Parser, Subcommand, ValueHint};
use clap_complete::{generate, Shell};
use humantime::Duration as HumanDuration;
use serde::{Deserialize, Serialize};
use std::{
    cmp::min,
    collections::BTreeSet,
    error::Error,
    fs::{read, File},
    io::{stderr, stdin, stdout},
    path::PathBuf,
    str::FromStr,
    time::Duration,
};
use tracing::{debug, info, warn};
use tracing_subscriber::filter::LevelFilter;
use uuid::Uuid;

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
            | "az-pim deactivate interactive"
            | "az-pim role"
            | "az-pim role assignment"
            | "az-pim role definition" => None,
            "az-pim list" => Some(include_str!("../help/az-pim-list.txt")),
            "az-pim activate role <ROLE> <SCOPE> <JUSTIFICATION>" => {
                Some(include_str!("../help/az-pim-activate-role.txt"))
            }
            "az-pim activate set <JUSTIFICATION>" => {
                Some(include_str!("../help/az-pim-activate-set.txt"))
            }
            "az-pim deactivate role <ROLE> <SCOPE>" => {
                Some(include_str!("../help/az-pim-deactivate-role.txt"))
            }
            "az-pim deactivate set" => Some(include_str!("../help/az-pim-deactivate-set.txt")),
            "az-pim init <SHELL>" => Some(include_str!("../help/az-pim-init.txt")),
            "az-pim role assignment list" => {
                Some(include_str!("../help/az-pim-role-assignment-list.txt"))
            }
            "az-pim role assignment delete <ASSIGNMENT_NAME>" => {
                Some(include_str!("../help/az-pim-role-assignment-delete.txt"))
            }
            "az-pim role assignment delete-set <CONFIG>" => Some(include_str!(
                "../help/az-pim-role-assignment-delete-set.txt"
            )),
            "az-pim role assignment delete-orphan-entries" => Some(include_str!(
                "../help/az-pim-role-assignment-delete-orphan-entries.txt"
            )),
            "az-pim role definition list" => {
                Some(include_str!("../help/az-pim-role-definition-list.txt"))
            }
            unsupported => unimplemented!("unable to generate example for {unsupported}"),
        }
    }
}

#[derive(Subcommand)]
enum SubCommand {
    /// List active or eligible assignments
    List {
        /// List active assignments
        #[clap(long)]
        active: bool,

        /// Filter to apply on the operation
        ///
        /// Specifying `as-target` will return results for the current user.
        ///
        /// Specifying `at-scope` will return results at or above the specified scope.
        #[clap(long, default_value_t = ListFilter::AsTarget)]
        filter: ListFilter,

        /// Filter at the Subscription level
        #[arg(long)]
        subscription: Option<Uuid>,

        /// List at the Resource Group level
        ///
        /// This argument requires `subscription` to be set.
        #[arg(long, requires = "subscription")]
        resource_group: Option<String>,

        /// List at the Provider level
        ///
        /// This argument requires `subscription` and `resource_group` to be set.
        #[arg(long, requires = "resource_group")]
        provider: Option<String>,

        /// List at a specified scope level
        #[arg(long, conflicts_with = "subscription")]
        scope: Option<Scope>,
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

    /// Manage Azure role-based access control (Azure RBAC).
    Role {
        #[clap(subcommand)]
        cmd: RoleSubCommand,
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
    fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::Role {
                role,
                scope,
                justification,
                duration,
                wait,
            } => {
                let roles = client
                    .list_eligible_role_assignments(None, Some(ListFilter::AsTarget))
                    .context("unable to list eligible assignments")?;
                let entry = roles
                    .find(&role, &scope)
                    .with_context(|| format!("role not found ({role:?} {scope:?})"))?;
                client.activate_role_assignment(entry, &justification, duration.into())?;

                if let Some(wait) = wait {
                    client.wait_for_role_activation(
                        &RoleAssignments([entry.clone()].into()),
                        wait.into(),
                    )?;
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
                let set = build_set(client, config, role, false)?;
                client.activate_role_assignment_set(
                    &set,
                    &justification,
                    duration.into(),
                    concurrency,
                )?;

                if let Some(wait) = wait {
                    client.wait_for_role_activation(&set, wait.into())?;
                }
            }
            Self::Interactive {
                justification,
                concurrency,
                duration,
                wait,
            } => {
                let roles =
                    client.list_eligible_role_assignments(None, Some(ListFilter::AsTarget))?;
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
                    client.activate_role_assignment_set(
                        &assignments,
                        &justification,
                        duration,
                        concurrency,
                    )?;

                    if let Some(wait) = wait {
                        client.wait_for_role_activation(&assignments, wait.into())?;
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
    fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::Role { role, scope } => {
                let roles = client
                    .list_active_role_assignments(None, Some(ListFilter::AsTarget))
                    .context("unable to list active assignments")?;
                let entry = roles.find(&role, &scope).context("role not found")?;
                client.deactivate_role_assignment(entry)?;
            }
            Self::Set {
                config,
                role,
                concurrency,
            } => {
                let set = build_set(client, config, role, true)?;
                client.deactivate_role_assignment_set(&set, concurrency)?;
            }
            Self::Interactive { concurrency } => {
                let roles =
                    client.list_active_role_assignments(None, Some(ListFilter::AsTarget))?;
                if let Some(Selected { assignments, .. }) = interactive_ui(roles, None, None)? {
                    client.deactivate_role_assignment_set(&assignments, concurrency)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Subcommand)]
enum RoleSubCommand {
    /// Manage role assignments
    Assignment {
        #[clap(subcommand)]
        cmd: AssignmentSubCommand,
    },

    /// Manage role definitions
    Definition {
        #[clap(subcommand)]
        cmd: DefinitionSubCommand,
    },
}

#[derive(Subcommand)]
enum AssignmentSubCommand {
    /// List assignments
    List {
        /// Subscription
        #[arg(long)]
        subscription: Option<Uuid>,

        /// Resource Group
        ///
        /// This argument requires `subscription` to be set.
        #[arg(long, requires = "subscription")]
        resource_group: Option<String>,

        /// Provider
        ///
        /// This argument requires `subscription` and `resource_group` to be set.
        #[arg(long, requires = "resource_group")]
        provider: Option<String>,

        /// Specify scope directly
        #[arg(long, conflicts_with = "subscription", required_unless_present_any = ["subscription", "resource_group", "provider"])]
        scope: Option<Scope>,
    },

    /// Delete an assignment
    Delete {
        /// Assignment name
        assignment_name: String,

        /// Subscription
        #[arg(long)]
        subscription: Option<Uuid>,

        /// Resource Group
        ///
        /// This argument requires `subscription` to be set.
        #[arg(long, requires = "subscription")]
        resource_group: Option<String>,

        /// Provider
        ///
        /// This argument requires `subscription` and `resource_group` to be set.
        #[arg(long, requires = "resource_group")]
        provider: Option<String>,

        /// Specify scope directly
        #[arg(long, conflicts_with = "subscription", required_unless_present_any = ["subscription", "resource_group", "provider"])]
        scope: Option<Scope>,
    },

    /// Delete a set of assignments
    DeleteSet {
        #[clap(value_hint = ValueHint::FilePath)]
        /// Path to a JSON config file containing a set of assignments to delete
        config: PathBuf,
    },

    /// Delete assignments that objects in Microsoft Graph cannot be found
    DeleteOrphanEntries {
        /// Subscription
        #[arg(long)]
        subscription: Option<Uuid>,

        /// Resource Group
        ///
        /// This argument requires `subscription` to be set.
        #[arg(long, requires = "subscription")]
        resource_group: Option<String>,

        /// Provider
        ///
        /// This argument requires `subscription` and `resource_group` to be set.
        #[arg(long, requires = "resource_group")]
        provider: Option<String>,

        /// Specify scope directly
        #[arg(long, conflicts_with = "subscription", required_unless_present_any = ["subscription", "resource_group", "provider"])]
        scope: Option<Scope>,

        /// Always respond yes to confirmations
        #[arg(long)]
        yes: bool,
    },
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

impl AssignmentSubCommand {
    fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::List {
                subscription,
                resource_group,
                scope,
                provider,
            } => {
                let scope = build_scope(subscription, resource_group, scope, provider)?
                    .context("valid scope must be provided")?;
                let objects = client
                    .list_assignments(&scope)
                    .context("unable to list active assignments")?;
                output(&objects)?;
            }
            Self::Delete {
                assignment_name,
                subscription,
                resource_group,
                scope,
                provider,
            } => {
                let scope = build_scope(subscription, resource_group, scope, provider)?
                    .context("valid scope must be provided")?;
                client
                    .delete_assignment(&scope, &assignment_name)
                    .context("unable to delete assignment")?;
            }
            Self::DeleteSet { config } => {
                let data = read(config)?;
                let entries = serde_json::from_slice::<Vec<Assignment>>(&data)
                    .context("unable to parse config file")?;
                for entry in entries {
                    client
                        .delete_assignment(&entry.properties.scope, &entry.name)
                        .context("unable to delete assignment")?;
                }
            }
            Self::DeleteOrphanEntries {
                subscription,
                resource_group,
                provider,
                scope,
                yes,
            } => {
                let scope = build_scope(subscription, resource_group, scope, provider)?
                    .context("valid scope must be provided")?;
                let mut objects = client
                    .list_assignments(&scope)
                    .context("unable to list active assignments")?;
                let definitions = client.role_definitions(&scope)?;
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
                        .delete_assignment(&entry.properties.scope, &entry.name)
                        .context("unable to delete assignment")?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Subcommand)]
enum DefinitionSubCommand {
    /// List the definitions for the specific scope
    List {
        /// Limit the scope by the specified Subscription
        #[arg(long)]
        subscription: Option<Uuid>,

        /// Limit the scope by the specified Resource Group
        ///
        /// This argument requires `subscription` to be set.
        #[arg(long, requires = "subscription")]
        resource_group: Option<String>,

        /// Provider
        ///
        /// This argument requires `subscription` and `resource_group` to be set.
        #[arg(long, requires = "resource_group")]
        provider: Option<String>,

        /// Specify scope directly
        #[arg(long, conflicts_with = "subscription", required_unless_present_any = ["subscription", "resource_group", "provider"])]
        scope: Option<Scope>,
    },
}
impl DefinitionSubCommand {
    fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::List {
                subscription,
                resource_group,
                scope,
                provider,
            } => {
                let scope = build_scope(subscription, resource_group, scope, provider)?
                    .context("valid scope must be provided")?;
                output(&client.role_definitions(&scope)?)?;
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

    let client = PimClient::new()?;

    match args.command {
        SubCommand::List {
            active,
            subscription,
            resource_group,
            provider,
            scope,
            filter,
        } => {
            let scope = build_scope(subscription, resource_group, scope, provider)?;

            let roles = if active {
                client.list_active_role_assignments(scope, Some(filter))?
            } else {
                client.list_eligible_role_assignments(scope, Some(filter))?
            };
            output(&roles)
        }
        SubCommand::Activate { cmd } => cmd.run(&client),
        SubCommand::Deactivate { cmd } => cmd.run(&client),
        SubCommand::Role { cmd } => match cmd {
            RoleSubCommand::Assignment { cmd } => cmd.run(&client),
            RoleSubCommand::Definition { cmd } => cmd.run(&client),
        },
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

fn build_scope(
    subscription: Option<Uuid>,
    resource_group: Option<String>,
    scope: Option<Scope>,
    provider: Option<String>,
) -> Result<Option<Scope>> {
    match (subscription, resource_group, provider, scope) {
        (Some(subscription), Some(group), Some(provider), None) => {
            Ok(Some(Scope::from_provider(&subscription, &group, &provider)))
        }
        (Some(subscription), Some(group), None, None) => {
            Ok(Some(Scope::from_resource_group(&subscription, &group)))
        }
        (Some(subscription), None, None, None) => Ok(Some(Scope::from_subscription(&subscription))),
        (None, None, None, Some(scope)) => Ok(Some(scope)),
        (None, None, None, None) => Ok(None),
        _ => {
            bail!("invalid scope");
        }
    }
}

fn build_set(
    client: &PimClient,
    config: Option<PathBuf>,
    role: Option<Vec<(Role, Scope)>>,
    active: bool,
) -> Result<RoleAssignments> {
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
            .list_active_role_assignments(None, Some(ListFilter::AsTarget))
            .context("unable to list active assignments in PIM")?
    } else {
        client
            .list_eligible_role_assignments(None, Some(ListFilter::AsTarget))
            .context("unable to list available assignments in PIM")?
    };

    let mut to_add = BTreeSet::new();
    for (role, scope) in desired_roles {
        let entry = assignments
            .find(&role, &scope)
            .with_context(|| format!("role not found.  role:{role} scope:{scope}"))?;
        to_add.insert(entry);
    }

    Ok(RoleAssignments(to_add.into_iter().cloned().collect()))
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
