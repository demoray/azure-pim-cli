use anyhow::{Context, Result};
use azure_pim_cli::{
    check_latest_version,
    interactive::{interactive_ui, Selected},
    models::{
        assignments::Assignment,
        roles::{Role, RoleAssignment, RolesExt},
        scope::{Scope, ScopeBuilder},
    },
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
    io::{stderr, stdout},
    path::PathBuf,
    str::FromStr,
    time::Duration,
};
use tracing::{debug, info};
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
            | "az-pim activate interactive"
            | "az-pim activate"
            | "az-pim cleanup all"
            | "az-pim cleanup auto"
            | "az-pim cleanup orphaned-assignments"
            | "az-pim cleanup orphaned-eligible-assignments"
            | "az-pim cleanup"
            | "az-pim deactivate interactive"
            | "az-pim deactivate"
            | "az-pim delete interactive"
            | "az-pim delete orphaned-entries"
            | "az-pim delete role <ROLE> <SCOPE>"
            | "az-pim delete set"
            | "az-pim delete"
            | "az-pim role assignment"
            | "az-pim role definition"
            | "az-pim role resources"
            | "az-pim role" => None,
            "az-pim activate role <ROLE> <JUSTIFICATION>" => {
                Some(include_str!("../help/az-pim-activate-role.txt"))
            }
            "az-pim activate set <JUSTIFICATION>" => {
                Some(include_str!("../help/az-pim-activate-set.txt"))
            }
            "az-pim deactivate role <ROLE>" => {
                Some(include_str!("../help/az-pim-deactivate-role.txt"))
            }
            "az-pim deactivate set" => Some(include_str!("../help/az-pim-deactivate-set.txt")),
            "az-pim init <SHELL>" => Some(include_str!("../help/az-pim-init.txt")),
            "az-pim list" => Some(include_str!("../help/az-pim-list.txt")),
            "az-pim role assignment delete-orphaned-entries" => Some(include_str!(
                "../help/az-pim-role-assignment-delete-orphan-entries.txt"
            )),
            "az-pim role assignment delete-set <CONFIG>" => Some(include_str!(
                "../help/az-pim-role-assignment-delete-set.txt"
            )),
            "az-pim role assignment delete <ASSIGNMENT_NAME>" => {
                Some(include_str!("../help/az-pim-role-assignment-delete.txt"))
            }
            "az-pim role assignment list" => {
                Some(include_str!("../help/az-pim-role-assignment-list.txt"))
            }
            "az-pim role definition list" => {
                Some(include_str!("../help/az-pim-role-definition-list.txt"))
            }
            "az-pim role resources list" => {
                Some(include_str!("../help/az-pim-role-resources-list.txt"))
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

        #[clap(flatten)]
        scope: ScopeBuilder,
    },

    /// Activate eligible role assignments
    Activate {
        #[clap(subcommand)]
        cmd: ActivateSubCommand,
    },

    /// Deactivate eligible role assignments
    Deactivate {
        #[clap(subcommand)]
        cmd: DeactivateSubCommand,
    },

    /// Manage Azure role-based access control (Azure RBAC).
    Role {
        #[clap(subcommand)]
        cmd: RoleSubCommand,
    },

    Cleanup {
        #[clap(subcommand)]
        cmd: CleanupSubCommand,
    },

    /// Setup shell tab completions
    ///
    /// This command will generate shell completions for the specified shell.
    Init { shell: Shell },

    #[command(hide = true)]
    /// Generate the README.md file dynamically
    Readme,
}

#[derive(Subcommand)]
enum ActivateSubCommand {
    /// Activate a specific role
    Role {
        /// Name of the role to activate
        role: Role,

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

        #[clap(flatten)]
        scope: ScopeBuilder,
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
                justification,
                duration,
                wait,
                scope,
            } => {
                let roles = client
                    .list_eligible_role_assignments(None, Some(ListFilter::AsTarget))
                    .context("unable to list eligible assignments")?;
                let scope = scope.build().context("valid scope must be provided")?;
                let entry = roles
                    .find_role(&role, &scope)
                    .with_context(|| format!("role not found ({role:?} {scope:?})"))?;
                client.activate_role_assignment(&entry, &justification, duration.into())?;

                if let Some(wait) = wait {
                    let assignments = [entry].into();
                    client.wait_for_role_activation(&assignments, wait.into())?;
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

        #[clap(flatten)]
        scope: ScopeBuilder,
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
                let scope = scope.build().context("valid scope must be provided")?;
                let roles = client
                    .list_active_role_assignments(None, Some(ListFilter::AsTarget))
                    .context("unable to list active assignments")?;
                let entry = roles.find_role(&role, &scope).context("role not found")?;
                client.deactivate_role_assignment(&entry)?;
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

    /// Commands related to resources in Azure
    Resources {
        #[clap(subcommand)]
        cmd: ResourcesSubCommand,
    },
}

#[derive(Subcommand)]
enum AssignmentSubCommand {
    /// List assignments
    List {
        #[clap(flatten)]
        scope: ScopeBuilder,
    },

    /// Delete an assignment
    Delete {
        /// Assignment name
        assignment_name: String,

        #[clap(flatten)]
        scope: ScopeBuilder,
    },

    /// Delete a set of assignments
    DeleteSet {
        #[clap(value_hint = ValueHint::FilePath)]
        /// Path to a JSON config file containing a set of assignments to delete
        config: PathBuf,
    },
}

impl AssignmentSubCommand {
    fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::List { scope } => {
                let scope = scope.build().context("valid scope must be provided")?;
                let objects = client
                    .role_assignments(&scope)
                    .context("unable to list active assignments")?;
                output(&objects)?;
            }
            Self::Delete {
                assignment_name,
                scope,
            } => {
                let scope = scope.build().context("valid scope must be provided")?;
                client
                    .delete_role_assignment(&scope, &assignment_name)
                    .context("unable to delete assignment")?;
            }
            Self::DeleteSet { config } => {
                let data = read(config)?;
                let entries = serde_json::from_slice::<Vec<Assignment>>(&data)
                    .context("unable to parse config file")?;
                for entry in entries {
                    client
                        .delete_role_assignment(&entry.properties.scope, &entry.name)
                        .context("unable to delete assignment")?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Subcommand)]
enum CleanupSubCommand {
    /// Delete orphaned role assignments and orphaned eligibile role assignments for all available scopes
    All {
        /// Always respond yes to confirmations
        #[arg(long)]
        yes: bool,
    },

    /// Delete orphaned role assignments and orphaned eligibile role assignments
    Auto {
        #[clap(flatten)]
        scope: ScopeBuilder,

        /// Do not check for nested assignments
        #[arg(long)]
        skip_nested: bool,

        /// Always respond yes to confirmations
        #[arg(long)]
        yes: bool,
    },

    /// Delete orphaned role assignments
    OrphanedAssignments {
        #[clap(flatten)]
        scope: ScopeBuilder,

        /// Do not check for nested assignments
        #[arg(long)]
        skip_nested: bool,

        /// Always respond yes to confirmations
        #[arg(long)]
        yes: bool,
    },

    /// Delete orphaned eligible role assignments
    OrphanedEligibleAssignments {
        #[clap(flatten)]
        scope: ScopeBuilder,

        /// Do not check for nested assignments
        #[arg(long)]
        skip_nested: bool,

        /// Always respond yes to confirmations
        #[arg(long)]
        yes: bool,
    },
}

impl CleanupSubCommand {
    fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::All { yes } => {
                let active =
                    client.list_active_role_assignments(None, Some(ListFilter::AsTarget))?;
                let mut total =
                    client.list_eligible_role_assignments(None, Some(ListFilter::AsTarget))?;
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
            }
            Self::Auto {
                scope,
                skip_nested,
                yes,
            } => {
                let scope = scope.build().context("valid scope must be provided")?;
                client.activate_role_admin(
                    &scope,
                    "cleaning up orphaned assignments",
                    Duration::from_secs(5 * 60),
                )?;
                client.delete_orphaned_role_assignments(&scope, yes, !skip_nested)?;
                client.delete_orphaned_eligible_role_assignments(&scope, yes, !skip_nested)?;
            }
            Self::OrphanedAssignments {
                scope,
                skip_nested,
                yes,
            } => {
                let scope = scope.build().context("valid scope must be provided")?;
                client.delete_orphaned_role_assignments(&scope, yes, !skip_nested)?;
            }
            Self::OrphanedEligibleAssignments {
                scope,
                skip_nested,
                yes,
            } => {
                let scope = scope.build().context("valid scope must be provided")?;
                client.delete_orphaned_eligible_role_assignments(&scope, yes, !skip_nested)?;
            }
        }
        Ok(())
    }
}

#[derive(Subcommand)]
enum DefinitionSubCommand {
    /// List the definitions for the specific scope
    List {
        #[clap(flatten)]
        scope: ScopeBuilder,
    },
}
impl DefinitionSubCommand {
    fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::List { scope } => {
                let scope = scope.build().context("valid scope must be provided")?;
                output(&client.role_definitions(&scope)?)?;
            }
        }
        Ok(())
    }
}

#[derive(Subcommand)]
enum ResourcesSubCommand {
    /// List the child resources of a resource which you have eligible access
    List {
        #[clap(flatten)]
        scope: ScopeBuilder,
    },
}

impl ResourcesSubCommand {
    fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::List { scope } => {
                let scope = scope.build().context("valid scope must be provided")?;
                output(&client.eligible_child_resources(&scope)?)?;
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
            filter,
            scope,
        } => {
            let scope = scope.build();
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
            RoleSubCommand::Resources { cmd } => cmd.run(&client),
        },
        SubCommand::Cleanup { cmd } => cmd.run(&client),
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
) -> Result<BTreeSet<RoleAssignment>> {
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
            .find_role(&role, &scope)
            .with_context(|| format!("role not found.  role:{role} scope:{scope}"))?;
        to_add.insert(entry);
    }

    Ok(to_add)
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
