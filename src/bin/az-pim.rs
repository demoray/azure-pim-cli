use anyhow::{ensure, Context, Result};
use azure_pim_cli::{
    check_latest_version,
    interactive::{interactive_ui, Selected},
    logging::{setup_logging, Verbosity},
    models::{
        assignments::Assignment,
        roles::{Role, RoleAssignment, RolesExt},
        scope::{Scope, ScopeBuilder},
    },
    ListFilter, PimClient,
};
use clap::{Command, CommandFactory, Parser, Subcommand, ValueHint};
use clap_complete::{generate, Shell};
use humantime::Duration as HumanDuration;
use serde::{Deserialize, Serialize};
use std::{
    cmp::min,
    collections::BTreeSet,
    error::Error,
    fmt::Write,
    fs::{read, File},
    io::stdout,
    path::PathBuf,
    str::FromStr,
    time::Duration,
};
use tracing::{debug, info};

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
    async fn run(self, client: &PimClient) -> Result<()> {
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
                    .await
                    .context("unable to list eligible assignments")?;
                let scope = scope.build().context("valid scope must be provided")?;
                let entry = roles
                    .find_role(&role, &scope)
                    .with_context(|| format!("role not found ({role:?} {scope:?})"))?;
                client
                    .activate_role_assignment(&entry, &justification, duration.into())
                    .await?;

                if let Some(wait) = wait {
                    let assignments = [entry].into();
                    client
                        .wait_for_role_activation(&assignments, wait.into())
                        .await?;
                }
            }
            Self::Set {
                config,
                role,
                justification,
                duration,
                wait,
            } => {
                let set = build_set(client, config, role, false).await?;
                ensure!(!set.is_empty(), "no roles to activate");
                client
                    .activate_role_assignment_set(&set, &justification, duration.into())
                    .await?;

                if let Some(wait) = wait {
                    client.wait_for_role_activation(&set, wait.into()).await?;
                }
            }
            Self::Interactive {
                justification,
                duration,
                wait,
            } => {
                let roles = client
                    .list_eligible_role_assignments(None, Some(ListFilter::AsTarget))
                    .await?;
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
                    client
                        .activate_role_assignment_set(&assignments, &justification, duration)
                        .await?;

                    if let Some(wait) = wait {
                        client
                            .wait_for_role_activation(&assignments, wait.into())
                            .await?;
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
    },
    /// Deactivate roles interactively
    Interactive {},
}

impl DeactivateSubCommand {
    async fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::Role { role, scope } => {
                let scope = scope.build().context("valid scope must be provided")?;
                let roles = client
                    .list_active_role_assignments(None, Some(ListFilter::AsTarget))
                    .await
                    .context("unable to list active assignments")?;
                let entry = roles.find_role(&role, &scope).context("role not found")?;
                client.deactivate_role_assignment(&entry).await?;
            }
            Self::Set { config, role } => {
                let set = build_set(client, config, role, true).await?;
                client.deactivate_role_assignment_set(&set).await?;
            }
            Self::Interactive {} => {
                let roles = client
                    .list_active_role_assignments(None, Some(ListFilter::AsTarget))
                    .await?;
                if let Some(Selected { assignments, .. }) = interactive_ui(roles, None, None)? {
                    client.deactivate_role_assignment_set(&assignments).await?;
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
    async fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::List { scope } => {
                let scope = scope.build().context("valid scope must be provided")?;
                let objects = client
                    .role_assignments(&scope)
                    .await
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
                    .await
                    .context("unable to delete assignment")?;
            }
            Self::DeleteSet { config } => {
                let data = read(config)?;
                let entries = serde_json::from_slice::<Vec<Assignment>>(&data)
                    .context("unable to parse config file")?;
                for entry in entries {
                    client
                        .delete_role_assignment(&entry.properties.scope, &entry.name)
                        .await
                        .context("unable to delete assignment")?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Subcommand)]
enum CleanupSubCommand {
    /// Delete orphaned role assignments and orphaned eligible role assignments for all available scopes
    All {
        /// Always respond yes to confirmations
        #[arg(long)]
        yes: bool,
    },

    /// Delete orphaned role assignments and orphaned eligible role assignments
    Auto {
        #[clap(flatten)]
        scope: ScopeBuilder,

        #[arg(long)]
        /// Do not check for nested assignments
        skip_nested: bool,

        #[arg(long)]
        /// Always respond yes to confirmations
        yes: bool,
    },

    /// Delete orphaned role assignments
    OrphanedAssignments {
        #[clap(flatten)]
        scope: ScopeBuilder,

        #[arg(long)]
        /// Do not check for nested assignments
        skip_nested: bool,

        #[arg(long)]
        /// Always respond yes to confirmations
        yes: bool,
    },

    /// Delete orphaned eligible role assignments
    OrphanedEligibleAssignments {
        #[clap(flatten)]
        scope: ScopeBuilder,

        #[arg(long)]
        /// Do not check for nested assignments
        skip_nested: bool,

        #[arg(long)]
        /// Always respond yes to confirmations
        yes: bool,
    },
}

impl CleanupSubCommand {
    async fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::All { yes } => {
                let active = client
                    .list_active_role_assignments(None, Some(ListFilter::AsTarget))
                    .await?;
                let mut total = client
                    .list_eligible_role_assignments(None, Some(ListFilter::AsTarget))
                    .await?;
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

                    if let Some(scope_name) = role_assignment.scope_name.as_ref() {
                        info!("checking {scope_name}");
                    } else {
                        info!("checking {}", role_assignment.scope.to_string());
                    }

                    if !active.contains(&role_assignment) {
                        to_activate.insert(role_assignment.clone());
                    }

                    scopes.insert(role_assignment.scope);
                }

                if !to_activate.is_empty() {
                    client
                        .activate_role_assignment_set(
                            &to_activate,
                            "cleaning up orphaned resources",
                            Duration::from_secs(60 * 60 * 8),
                        )
                        .await?;
                    client
                        .wait_for_role_activation(&to_activate, Duration::from_secs(60 * 5))
                        .await?;
                }

                for scope in scopes {
                    info!("deleting orphaned role assignments for {scope}");
                    client
                        .delete_orphaned_role_assignments(&scope, yes, true)
                        .await?;
                    info!("deleting orphaned eligible role assignments for {scope}");
                    client
                        .delete_orphaned_eligible_role_assignments(&scope, yes, true)
                        .await?;
                }
            }
            Self::Auto {
                scope,
                skip_nested,
                yes,
            } => {
                let scope = scope.build().context("valid scope must be provided")?;
                client
                    .activate_role_admin(
                        &scope,
                        "cleaning up orphaned assignments",
                        Duration::from_secs(5 * 60),
                    )
                    .await?;
                client
                    .delete_orphaned_role_assignments(&scope, yes, !skip_nested)
                    .await?;
                client
                    .delete_orphaned_eligible_role_assignments(&scope, yes, !skip_nested)
                    .await?;
            }
            Self::OrphanedAssignments {
                scope,
                skip_nested,
                yes,
            } => {
                let scope = scope.build().context("valid scope must be provided")?;
                client
                    .delete_orphaned_role_assignments(&scope, yes, !skip_nested)
                    .await?;
            }
            Self::OrphanedEligibleAssignments {
                scope,
                skip_nested,
                yes,
            } => {
                let scope = scope.build().context("valid scope must be provided")?;
                client
                    .delete_orphaned_eligible_role_assignments(&scope, yes, !skip_nested)
                    .await?;
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
    async fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::List { scope } => {
                let scope = scope.build().context("valid scope must be provided")?;
                output(&client.role_definitions(&scope).await?)?;
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

        #[arg(long)]
        /// Do not check for nested assignments
        skip_nested: bool,
    },
}

impl ResourcesSubCommand {
    async fn run(self, client: &PimClient) -> Result<()> {
        match self {
            Self::List { scope, skip_nested } => {
                let scope = scope.build().context("valid scope must be provided")?;
                output(
                    &client
                        .eligible_child_resources(&scope, !skip_nested)
                        .await?,
                )?;
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

fn build_readme_entry(cmd: &mut Command, mut names: Vec<String>) -> Result<String> {
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
    write!(readme, " {name}\n\n```\n{long_help}\n```\n")?;

    if let Some(example) = Cmd::example(&name) {
        for _ in 0..=depth {
            readme.push('#');
        }
        write!(readme, " Example Usage\n\n```\n{}\n```\n\n", example.trim())?;
    }

    for cmd in cmd.get_subcommands_mut() {
        if cmd.get_name() == "readme" {
            continue;
        }
        readme.push_str(&build_readme_entry(cmd, names.clone())?);
    }
    Ok(readme)
}

fn build_readme() -> Result<()> {
    let mut cmd = Cmd::command();
    let readme = build_readme_entry(&mut cmd, Vec::new())?
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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cmd::parse();

    setup_logging(&args.verbose)?;

    if let Err(err) = check_latest_version().await {
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
                client
                    .list_active_role_assignments(scope, Some(filter))
                    .await?
            } else {
                client
                    .list_eligible_role_assignments(scope, Some(filter))
                    .await?
            };
            output(&roles)
        }
        SubCommand::Activate { cmd } => cmd.run(&client).await,
        SubCommand::Deactivate { cmd } => cmd.run(&client).await,
        SubCommand::Role { cmd } => match cmd {
            RoleSubCommand::Assignment { cmd } => cmd.run(&client).await,
            RoleSubCommand::Definition { cmd } => cmd.run(&client).await,
            RoleSubCommand::Resources { cmd } => cmd.run(&client).await,
        },
        SubCommand::Cleanup { cmd } => cmd.run(&client).await,
        SubCommand::Readme => build_readme(),
        SubCommand::Init { shell } => {
            Cmd::shell_completion(shell);
            Ok(())
        }
    }
}

async fn build_set(
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
            .await
            .context("unable to list active assignments in PIM")?
    } else {
        client
            .list_eligible_role_assignments(None, Some(ListFilter::AsTarget))
            .await
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
