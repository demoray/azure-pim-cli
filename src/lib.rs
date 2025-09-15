#![forbid(unsafe_code)]
#![deny(
    clippy::indexing_slicing,
    clippy::manual_assert,
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used
)]
#![allow(clippy::module_name_repetitions)]

mod activate;
mod az_cli;
mod backend;
mod expiring;
pub mod graph;
pub mod interactive;
mod latest;
pub mod models;

pub use crate::latest::check_latest_version;
use crate::{
    activate::check_error_response,
    backend::Backend,
    expiring::ExpiringMap,
    graph::{get_objects_by_ids, group_members, Object, PrincipalType},
    models::{
        assignments::{Assignment, Assignments},
        definitions::{Definition, Definitions},
        resources::ChildResource,
        roles::{RoleAssignment, RolesExt},
        scope::Scope,
    },
};
use anyhow::{bail, ensure, Context, Result};
use backend::Operation;
use clap::ValueEnum;
use reqwest::Method;
use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter, Result as FmtResult},
    io::stdin,
    thread::sleep,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const WAIT_DELAY: Duration = Duration::from_secs(5);
const RBAC_ADMIN_ROLES: &[&str] = &["Owner", "Role Based Access Control Administrator"];

#[allow(clippy::large_enum_variant)]
pub enum ActivationResult {
    Success,
    Failed(RoleAssignment),
}

#[allow(clippy::manual_assert, clippy::panic)]
#[derive(Clone, ValueEnum, PartialEq, Eq, PartialOrd, Ord)]
pub enum ListFilter {
    AtScope,
    AsTarget,
}

impl Display for ListFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        match self {
            Self::AtScope => write!(f, "at-scope"),
            Self::AsTarget => write!(f, "as-target"),
        }
    }
}

impl ListFilter {
    fn as_str(&self) -> &'static str {
        match self {
            Self::AtScope => "atScope()",
            Self::AsTarget => "asTarget()",
        }
    }
}

pub struct PimClient {
    backend: Backend,
    object_cache: Mutex<ExpiringMap<String, Option<Object>>>,
    group_cache: Mutex<ExpiringMap<String, BTreeSet<Object>>>,
    role_definitions_cache: Mutex<ExpiringMap<Scope, Vec<Definition>>>,
}

impl PimClient {
    pub fn new() -> Result<Self> {
        let backend = Backend::new();
        let object_cache = Mutex::new(ExpiringMap::new(Duration::from_secs(60 * 10)));
        let group_cache = Mutex::new(ExpiringMap::new(Duration::from_secs(60 * 10)));
        let role_definitions_cache = Mutex::new(ExpiringMap::new(Duration::from_secs(60 * 10)));
        Ok(Self {
            backend,
            object_cache,
            group_cache,
            role_definitions_cache,
        })
    }

    pub async fn clear_cache(&self) {
        self.object_cache.lock().await.clear();
        self.role_definitions_cache.lock().await.clear();
    }

    pub async fn current_user(&self) -> Result<String> {
        self.backend.principal_id().await
    }

    /// List the roles available to the current user
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub async fn list_eligible_role_assignments(
        &self,
        scope: Option<Scope>,
        filter: Option<ListFilter>,
    ) -> Result<BTreeSet<RoleAssignment>> {
        let with_principal = filter.as_ref() != Some(&ListFilter::AsTarget);
        if let Some(scope) = &scope {
            info!("listing eligible assignments for {scope}");
        } else {
            info!("listing eligible assignments");
        }
        let mut builder = self
            .backend
            .request(Method::GET, Operation::RoleEligibilityScheduleInstances);

        if let Some(scope) = scope {
            builder = builder.scope(scope);
        }

        if let Some(filter) = filter {
            builder = builder.query("$filter", filter.as_str());
        }

        let response = builder
            .send()
            .await
            .context("unable to list eligible assignments")?;
        let mut results = RoleAssignment::parse(&response, with_principal)
            .context("unable to parse eligible assignments")?;

        if with_principal {
            let ids = results
                .iter()
                .filter_map(|x| x.principal_id.as_deref())
                .collect::<BTreeSet<_>>();

            let objects = get_objects_by_ids(self, ids)
                .await
                .context("getting objects by id")?;
            results = results
                .into_iter()
                .map(|mut x| {
                    if let Some(principal_id) = x.principal_id.as_ref() {
                        x.object = objects.get(principal_id).cloned();
                    }
                    x
                })
                .collect();
        }

        Ok(results)
    }

    /// List the roles active role assignments for the current user
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub async fn list_active_role_assignments(
        &self,
        scope: Option<Scope>,
        filter: Option<ListFilter>,
    ) -> Result<BTreeSet<RoleAssignment>> {
        let with_principal = filter.as_ref() != Some(&ListFilter::AsTarget);

        if let Some(scope) = &scope {
            info!("listing active role assignments in {scope}");
        } else {
            info!("listing active role assignments");
        }

        let mut builder = self
            .backend
            .request(Method::GET, Operation::RoleAssignmentScheduleInstances);

        if let Some(scope) = scope {
            builder = builder.scope(scope);
        }

        if let Some(filter) = filter {
            builder = builder.query("$filter", filter.as_str());
        }

        let response = builder
            .send()
            .await
            .context("unable to list active role assignments")?;
        let mut results = RoleAssignment::parse(&response, with_principal)
            .context("unable to parse active role assignments")?;

        if with_principal {
            let ids = results
                .iter()
                .filter_map(|x| x.principal_id.as_deref())
                .collect::<BTreeSet<_>>();

            let objects = get_objects_by_ids(self, ids)
                .await
                .context("getting objects by id")?;
            results = results
                .into_iter()
                .map(|mut x| {
                    if let Some(principal_id) = x.principal_id.as_ref() {
                        x.object = objects.get(principal_id).cloned();
                    }
                    x
                })
                .collect();
        }
        Ok(results)
    }

    /// Request extending the specified role eligibility
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub async fn extend_role_assignment(
        &self,
        assignment: &RoleAssignment,
        justification: &str,
        duration: Duration,
    ) -> Result<()> {
        let RoleAssignment {
            scope,
            role_definition_id,
            role,
            scope_name,
            principal_id: _,
            principal_type: _,
            object: _,
        } = assignment;
        if let Some(scope_name) = scope_name {
            info!("extending {role} in {scope_name} ({scope})");
        } else {
            info!("extending {role} in {scope}");
        }
        let request_id = Uuid::now_v7();
        let body = serde_json::json!({
            "properties": {
                "principalId": self.backend.principal_id().await?,
                "roleDefinitionId": role_definition_id,
                "requestType": "SelfExtend",
                "justification": justification,
                "scheduleInfo": {
                    "expiration": {
                        "duration": format_duration(duration)?,
                        "type": "AfterDuration",
                    }
                }
            }
        });

        self.backend
            .request(Method::PUT, Operation::RoleAssignmentScheduleRequests)
            .extra(format!("/{request_id}"))
            .scope(scope.clone())
            .json(body)
            .validate(check_error_response)
            .send()
            .await?;
        Ok(())
    }

    /// Activates the specified role
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub async fn activate_role_assignment(
        &self,
        assignment: &RoleAssignment,
        justification: &str,
        duration: Duration,
    ) -> Result<()> {
        let RoleAssignment {
            scope,
            role_definition_id,
            role,
            scope_name,
            principal_id: _,
            principal_type: _,
            object: _,
        } = assignment;
        if let Some(scope_name) = scope_name {
            info!("activating {role} in {scope_name} ({scope})");
        } else {
            info!("activating {role} in {scope}");
        }
        let request_id = Uuid::now_v7();
        let body = serde_json::json!({
            "properties": {
                "principalId": self.backend.principal_id().await?,
                "roleDefinitionId": role_definition_id,
                "requestType": "SelfActivate",
                "justification": justification,
                "scheduleInfo": {
                    "expiration": {
                        "duration": format_duration(duration)?,
                        "type": "AfterDuration",
                    }
                }
            }
        });

        self.backend
            .request(Method::PUT, Operation::RoleAssignmentScheduleRequests)
            .extra(format!("/{request_id}"))
            .scope(scope.clone())
            .json(body)
            .validate(check_error_response)
            .send()
            .await?;

        Ok(())
    }

    pub async fn activate_role_assignment_set(
        &self,
        assignments: &BTreeSet<RoleAssignment>,
        justification: &str,
        duration: Duration,
    ) -> Result<()> {
        ensure!(!assignments.is_empty(), "no roles specified");

        let results = assignments.iter().map(|x| async {
            let result = self
                .activate_role_assignment(x, justification, duration)
                .await;
            match result {
                Ok(()) => ActivationResult::Success,
                Err(error) => {
                    error!(
                        "scope: {} definition: {} error: {error:?}",
                        x.scope, x.role_definition_id
                    );
                    ActivationResult::Failed(x.clone())
                }
            }
        });

        let results = futures::future::join_all(results).await;

        let mut failed = BTreeSet::new();

        for result in results {
            match result {
                ActivationResult::Failed(entry) => {
                    failed.insert(entry);
                }
                ActivationResult::Success => {}
            }
        }

        if !failed.is_empty() {
            bail!(
                "failed to activate the following roles:\n{}",
                failed.friendly()
            );
        }

        Ok(())
    }

    /// Deactivate the specified role
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub async fn deactivate_role_assignment(&self, assignment: &RoleAssignment) -> Result<()> {
        let RoleAssignment {
            scope,
            role_definition_id,
            role,
            scope_name,
            principal_id: _,
            principal_type: _,
            object: _,
        } = assignment;
        if let Some(scope_name) = scope_name {
            info!("deactivating {role} in {scope_name} ({scope})");
        } else {
            info!("deactivating {role} in {scope}");
        }
        let request_id = Uuid::now_v7();
        let body = serde_json::json!({
            "properties": {
                "principalId": self.backend.principal_id().await?,
                "roleDefinitionId": role_definition_id,
                "requestType": "SelfDeactivate",
                "justification": "Deactivation request",
            }
        });

        self.backend
            .request(Method::PUT, Operation::RoleAssignmentScheduleRequests)
            .extra(format!("/{request_id}"))
            .scope(scope.clone())
            .json(body)
            .validate(check_error_response)
            .send()
            .await?;
        Ok(())
    }

    pub async fn deactivate_role_assignment_set(
        &self,
        assignments: &BTreeSet<RoleAssignment>,
    ) -> Result<()> {
        ensure!(!assignments.is_empty(), "no roles specified");

        let results = assignments.iter().map(|entry| async {
            match self.deactivate_role_assignment(entry).await {
                Ok(()) => ActivationResult::Success,
                Err(error) => {
                    error!(
                        "scope: {} definition: {} error: {error:?}",
                        entry.scope, entry.role_definition_id
                    );
                    ActivationResult::Failed(entry.clone())
                }
            }
        });
        let results = futures::future::join_all(results).await;

        let mut failed = BTreeSet::new();

        for result in results {
            match result {
                ActivationResult::Failed(entry) => {
                    failed.insert(entry);
                }
                ActivationResult::Success => {}
            }
        }

        if !failed.is_empty() {
            bail!(
                "failed to deactivate the following roles:\n{}",
                failed.friendly()
            );
        }

        Ok(())
    }

    pub async fn wait_for_role_activation(
        &self,
        assignments: &BTreeSet<RoleAssignment>,
        wait_timeout: Duration,
    ) -> Result<()> {
        if assignments.is_empty() {
            return Ok(());
        }

        let start = Instant::now();
        let mut last = None::<Instant>;

        let mut waiting = assignments.clone();
        while !waiting.is_empty() {
            if start.elapsed() > wait_timeout {
                break;
            }

            // only check active assignments every `wait_timeout` seconds.
            //
            // While the list active assignments endpoint takes ~10-30 seconds
            // today, it could go faster in the future and this should avoid
            // spamming said API
            let current = Instant::now();
            if let Some(last) = last {
                let to_wait = last.duration_since(current).saturating_sub(WAIT_DELAY);
                if !to_wait.is_zero() {
                    debug!("sleeping {to_wait:?} before checking active assignments");
                    sleep(to_wait);
                }
            }
            last = Some(current);

            let active = self
                .list_active_role_assignments(None, Some(ListFilter::AsTarget))
                .await?;
            debug!("active assignments: {active:#?}");
            waiting.retain(|entry| !active.contains(entry));
            debug!("still waiting: {waiting:#?}");
        }

        if !waiting.is_empty() {
            bail!(
                "timed out waiting for the following roles to activate:\n{}",
                waiting.friendly()
            );
        }

        Ok(())
    }

    /// List role assignments
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub async fn role_assignments(&self, scope: &Scope) -> Result<Vec<Assignment>> {
        info!("listing assignments for {scope}");
        let value = self
            .backend
            .request(Method::GET, Operation::RoleAssignments)
            .scope(scope.clone())
            .send()
            .await
            .with_context(|| format!("unable to list role assignments at {scope}"))?;
        let assignments: Assignments = serde_json::from_value(value)
            .with_context(|| format!("unable to parse role assignment response at {scope}"))?;
        let mut assignments = assignments.value;
        let ids = assignments
            .iter()
            .map(|x| x.properties.principal_id.as_str())
            .collect();

        let objects = get_objects_by_ids(self, ids)
            .await
            .context("getting objects by id")?;
        for x in &mut assignments {
            x.object = objects.get(&x.properties.principal_id).cloned();
        }
        Ok(assignments)
    }

    /// List eligible child resources for the specified scope
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub async fn eligible_child_resources(
        &self,
        scope: &Scope,
        nested: bool,
    ) -> Result<BTreeSet<ChildResource>> {
        let mut todo = [scope.clone()].into_iter().collect::<BTreeSet<_>>();
        let mut seen = BTreeSet::new();
        let mut result = BTreeSet::new();

        while !todo.is_empty() {
            seen.extend(todo.clone());
            // let iteration: Vec<Result<Result<BTreeSet<ChildResource>>>> = todo
            let iteration = todo.iter().map(|scope| async {
                let scope = scope.clone();
                info!("listing eligible child resources for {scope}");
                self.backend
                    .request(Method::GET, Operation::EligibleChildResources)
                    .scope(scope.clone())
                    .send()
                    .await
                    .with_context(|| format!("unable to list eligible child resources for {scope}"))
                    .map(|x| {
                        ChildResource::parse(&x).with_context(|| {
                            format!("unable to parse eligible child resources for {scope}")
                        })
                    })
            });
            let iteration = futures::future::join_all(iteration).await;

            todo = BTreeSet::new();
            for entry in iteration {
                for child in entry?? {
                    if nested && !seen.contains(&child.id) {
                        todo.insert(child.id.clone());
                    }
                    result.insert(child);
                }
            }
        }

        Ok(result)
    }

    /// List role definitions available at the target scope
    ///
    /// Note, this will cache the results for 10 minutes.
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub async fn role_definitions(&self, scope: &Scope) -> Result<Vec<Definition>> {
        let mut cache = self.role_definitions_cache.lock().await;

        if let Some(cached) = cache.get(scope) {
            return Ok(cached.clone());
        }

        info!("listing role definitions for {scope}");
        let definitions = self
            .backend
            .request(Method::GET, Operation::RoleDefinitions)
            .scope(scope.clone())
            .send()
            .await
            .with_context(|| format!("unable to list role definitions at {scope}"))?;
        let definitions: Definitions = serde_json::from_value(definitions)
            .with_context(|| format!("unable to parse role definitions at {scope}"))?;
        cache.insert(scope.clone(), definitions.value.clone());

        Ok(definitions.value)
    }

    /// Delete a role assignment
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub async fn delete_role_assignment(&self, scope: &Scope, assignment_name: &str) -> Result<()> {
        info!("deleting assignment {assignment_name} from {scope}");
        self.backend
            .request(Method::DELETE, Operation::RoleAssignments)
            .extra(format!("/{assignment_name}"))
            .scope(scope.clone())
            .send()
            .await
            .with_context(|| format!("unable to delete assignment {assignment_name} at {scope}"))?;
        Ok(())
    }

    /// Delete eligibile role assignment
    ///
    /// This removes role assignments that are available via PIM.
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub async fn delete_eligible_role_assignment(&self, assignment: &RoleAssignment) -> Result<()> {
        let RoleAssignment {
            scope,
            role_definition_id,
            role,
            scope_name,
            principal_id,
            principal_type: _,
            object: _,
        } = assignment;

        let principal_id = principal_id.as_deref().context("missing principal id")?;
        info!("deleting {role} in {scope_name:?} ({scope})");
        let request_id = Uuid::now_v7();
        let body = serde_json::json!({
            "properties": {
                "principalId": principal_id,
                "roleDefinitionId": role_definition_id,
                "requestType": "AdminRemove",
                "ScheduleInfo": {
                    "Expiration": {
                        "Type": "NoExpiration",
                    }
                }
            }
        });

        self.backend
            .request(Method::PUT, Operation::RoleEligibilityScheduleRequests)
            .extra(format!("/{request_id}"))
            .scope(scope.clone())
            .json(body)
            .validate(check_error_response)
            .send()
            .await
            .with_context(|| {
                format!("unable to delete role definition {role_definition_id} for {principal_id}")
            })?;
        Ok(())
    }

    pub async fn delete_orphaned_role_assignments(
        &self,
        scope: &Scope,
        answer_yes: bool,
        nested: bool,
    ) -> Result<()> {
        let scopes = if nested {
            self.eligible_child_resources(scope, nested)
                .await?
                .into_iter()
                .map(|x| x.id)
                .collect::<BTreeSet<_>>()
        } else {
            [scope.clone()].into_iter().collect()
        };

        for scope in scopes {
            let definitions = self.role_definitions(&scope).await?;

            let mut objects = self
                .role_assignments(&scope)
                .await
                .with_context(|| format!("unable to list role assignments at {scope}"))?;
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
                if !answer_yes && !confirm(&format!("delete {value}")) {
                    info!("skipping {value}");
                    continue;
                }

                self.delete_role_assignment(&entry.properties.scope, &entry.name)
                    .await
                    .context("unable to delete assignment")?;
            }
        }
        Ok(())
    }

    pub async fn delete_orphaned_eligible_role_assignments(
        &self,
        scope: &Scope,
        answer_yes: bool,
        nested: bool,
    ) -> Result<()> {
        let scopes = if nested {
            self.eligible_child_resources(scope, nested)
                .await?
                .into_iter()
                .map(|x| x.id)
                .collect::<BTreeSet<_>>()
        } else {
            [scope.clone()].into_iter().collect()
        };
        for scope in scopes {
            let definitions = self.role_definitions(&scope).await?;
            for entry in self
                .list_eligible_role_assignments(Some(scope), None)
                .await?
            {
                if entry.object.is_some() {
                    continue;
                }

                let definition = definitions
                    .iter()
                    .find(|x| x.id == entry.role_definition_id);

                let value = format!(
                    "role:\"{}\" principal:{} (type: {}) scope:{}",
                    definition.map_or(entry.role_definition_id.as_str(), |x| x
                        .properties
                        .role_name
                        .as_str()),
                    entry.principal_id.clone().unwrap_or_default(),
                    entry.principal_type.clone().unwrap_or_default(),
                    entry
                        .scope_name
                        .clone()
                        .unwrap_or_else(|| entry.scope.to_string())
                );
                if !answer_yes && !confirm(&format!("delete {value}")) {
                    info!("skipping {value}");
                    continue;
                }
                info!("deleting {value}");

                self.delete_eligible_role_assignment(&entry).await?;
            }
        }

        Ok(())
    }

    pub async fn activate_role_admin(
        &self,
        scope: &Scope,
        justification: &str,
        duration: Duration,
    ) -> Result<()> {
        let active = self
            .list_active_role_assignments(None, Some(ListFilter::AsTarget))
            .await?;

        for entry in active {
            if entry.scope.contains(scope) && RBAC_ADMIN_ROLES.contains(&entry.role.0.as_str()) {
                info!("role already active: {entry:?}");
                return Ok(());
            }
        }

        let eligible = self
            .list_eligible_role_assignments(None, Some(ListFilter::AsTarget))
            .await?;
        for entry in eligible {
            if entry.scope.contains(scope) && RBAC_ADMIN_ROLES.contains(&entry.role.0.as_str()) {
                return self
                    .activate_role_assignment(&entry, justification, duration)
                    .await;
            }
        }

        bail!("unable to find role to administrate RBAC for {scope}");
    }

    pub async fn group_members(&self, id: &str, nested: bool) -> Result<BTreeSet<Object>> {
        if !nested {
            return group_members(self, id).await;
        }

        let mut results = BTreeSet::new();
        let mut todo = [id.to_string()].into_iter().collect::<BTreeSet<_>>();
        let mut done = BTreeSet::new();

        while let Some(id) = todo.pop_first() {
            if done.contains(&id) {
                continue;
            }
            done.insert(id.clone());

            let group_results = group_members(self, &id).await?;
            todo.extend(
                group_results
                    .iter()
                    .filter(|x| matches!(x.object_type, PrincipalType::Group))
                    .map(|x| x.id.clone()),
            );
            results.extend(group_results);
        }
        Ok(results)
    }
}

fn format_duration(duration: Duration) -> Result<String> {
    let mut as_secs = duration.as_secs();

    let hours = as_secs / 3600;
    as_secs %= 3600;

    let minutes = as_secs / 60;
    let seconds = as_secs % 60;

    let mut data = vec![];
    if hours > 0 {
        data.push(format!("{hours}H"));
    }
    if minutes > 0 {
        data.push(format!("{minutes}M"));
    }
    if seconds > 0 {
        data.push(format!("{seconds}S"));
    }

    ensure!(!data.is_empty(), "duration must be at least 1 second");
    Ok(format!("PT{}", data.join("")))
}

pub fn confirm(msg: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() -> Result<()> {
        assert!(format_duration(Duration::from_secs(0)).is_err());

        for (secs, parsed) in [
            (1, "PT1S"),
            (60, "PT1M"),
            (61, "PT1M1S"),
            (3600, "PT1H"),
            (86400, "PT24H"),
            (86401, "PT24H1S"),
            (86460, "PT24H1M"),
            (86520, "PT24H2M"),
            (90061, "PT25H1M1S"),
        ] {
            assert_eq!(format_duration(Duration::from_secs(secs))?, parsed);
        }

        Ok(())
    }
}
