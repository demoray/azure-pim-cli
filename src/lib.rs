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
pub mod assignments;
mod az_cli;
mod backend;
mod definitions;
mod graph;
pub mod interactive;
mod latest;
pub mod resources;
pub mod roles;
pub mod scope;

pub use crate::latest::check_latest_version;
use crate::{
    activate::check_error_response,
    assignments::{Assignment, Assignments},
    backend::Backend,
    definitions::{Definition, Definitions},
    graph::get_objects_by_ids,
    resources::ChildResource,
    roles::{RoleAssignment, RoleAssignments},
    scope::Scope,
};
use anyhow::{bail, ensure, Context, Result};
use backend::Operation;
use clap::ValueEnum;
use rayon::{prelude::*, ThreadPoolBuilder};
use reqwest::Method;
use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter, Result as FmtResult},
    sync::Once,
    thread::sleep,
    time::{Duration, Instant},
};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const WAIT_DELAY: Duration = Duration::from_secs(5);

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
}

impl PimClient {
    pub fn new() -> Result<Self> {
        let backend = Backend::new();
        Ok(Self { backend })
    }

    pub fn current_user(&self) -> Result<String> {
        self.backend.principal_id()
    }

    fn thread_builder(concurrency: usize) {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            if let Err(err) = ThreadPoolBuilder::new()
                .num_threads(concurrency)
                .build_global()
            {
                warn!("thread pool failed to build: {err}");
            }
        });
    }

    /// List the roles available to the current user
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub fn list_eligible_role_assignments(
        &self,
        scope: Option<Scope>,
        filter: Option<ListFilter>,
    ) -> Result<RoleAssignments> {
        let with_principal = filter.as_ref().map_or(true, |x| x != &ListFilter::AsTarget);
        info!("listing eligible assignments");
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
            .context("unable to list eligible assignments")?;
        let mut results = RoleAssignments::parse(&response, with_principal)
            .context("unable to parse eligible assignments")?
            .0;

        if with_principal {
            let ids = results
                .iter()
                .filter_map(|x| x.principal_id.as_deref())
                .collect::<BTreeSet<_>>();

            let objects = get_objects_by_ids(self, ids).context("getting objects by id")?;
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

        Ok(RoleAssignments(results))
    }

    /// List the roles active role assignments for the current user
    pub fn list_active_role_assignments(
        &self,
        scope: Option<Scope>,
        filter: Option<ListFilter>,
    ) -> Result<RoleAssignments> {
        let with_principal = filter.as_ref().map_or(true, |x| x != &ListFilter::AsTarget);

        info!("listing active assignments");
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
            .context("unable to list active assignments")?;
        let mut results = RoleAssignments::parse(&response, with_principal)
            .context("unable to parse active assignments")?
            .0;

        if with_principal {
            let ids = results
                .iter()
                .filter_map(|x| x.principal_id.as_deref())
                .collect::<BTreeSet<_>>();

            let objects = get_objects_by_ids(self, ids).context("getting objects by id")?;
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
        Ok(RoleAssignments(results))
    }

    /// Request extending the specified role eligibility
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub fn extend_role_assignment(
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
        info!("extending {role} in {scope_name} ({scope})");
        let request_id = Uuid::now_v7();
        let body = serde_json::json!({
            "properties": {
                "principalId": self.backend.principal_id()?,
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
            .send()?;
        Ok(())
    }

    /// Activates the specified role
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub fn activate_role_assignment(
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
        info!("activating {role} in {scope_name} ({scope})");
        let request_id = Uuid::now_v7();
        let body = serde_json::json!({
            "properties": {
                "principalId": self.backend.principal_id()?,
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
            .send()?;

        Ok(())
    }

    pub fn activate_role_assignment_set(
        &self,
        assignments: &RoleAssignments,
        justification: &str,
        duration: Duration,
        concurrency: usize,
    ) -> Result<()> {
        ensure!(!assignments.0.is_empty(), "no roles specified");

        Self::thread_builder(concurrency);

        let results = assignments
            .0
            .clone()
            .into_par_iter()
            .map(
                |entry| match self.activate_role_assignment(&entry, justification, duration) {
                    Ok(()) => ActivationResult::Success,
                    Err(error) => {
                        error!(
                            "scope: {} definition: {} error: {error:?}",
                            entry.scope, entry.role_definition_id
                        );
                        ActivationResult::Failed(entry)
                    }
                },
            )
            .collect::<Vec<_>>();

        let mut failed = RoleAssignments::default();

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
    pub fn deactivate_role_assignment(&self, assignment: &RoleAssignment) -> Result<()> {
        let RoleAssignment {
            scope,
            role_definition_id,
            role,
            scope_name,
            principal_id: _,
            principal_type: _,
            object: _,
        } = assignment;
        info!("deactivating {role} in {scope_name} ({scope})");
        let request_id = Uuid::now_v7();
        let body = serde_json::json!({
            "properties": {
                "principalId": self.backend.principal_id()?,
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
            .send()?;
        Ok(())
    }

    pub fn deactivate_role_assignment_set(
        &self,
        assignments: &RoleAssignments,
        concurrency: usize,
    ) -> Result<()> {
        ensure!(!assignments.0.is_empty(), "no roles specified");

        Self::thread_builder(concurrency);

        let results = assignments
            .0
            .clone()
            .into_par_iter()
            .map(|entry| match self.deactivate_role_assignment(&entry) {
                Ok(()) => ActivationResult::Success,
                Err(error) => {
                    error!(
                        "scope: {} definition: {} error: {error:?}",
                        entry.scope, entry.role_definition_id
                    );
                    ActivationResult::Failed(entry)
                }
            })
            .collect::<Vec<_>>();

        let mut failed = RoleAssignments::default();

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

    pub fn wait_for_role_activation(
        &self,
        assignments: &RoleAssignments,
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

            let active = self.list_active_role_assignments(None, Some(ListFilter::AsTarget))?;
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

    /// List all assignments (not just those managed by PIM)
    pub fn list_assignments(&self, scope: &Scope) -> Result<Vec<Assignment>> {
        info!("listing assignments for {scope}");
        let value = self
            .backend
            .request(Method::GET, Operation::RoleAssignments)
            .scope(scope.clone())
            .send()
            .context("unable to list assignments")?;
        let assignments: Assignments = serde_json::from_value(value)?;
        let mut assignments = assignments.value;
        let ids = assignments
            .iter()
            .map(|x| x.properties.principal_id.as_str())
            .collect();

        let objects = get_objects_by_ids(self, ids).context("getting objects by id")?;
        for x in &mut assignments {
            x.object = objects.get(&x.properties.principal_id).cloned();
        }
        Ok(assignments)
    }

    pub fn eligible_child_resources(&self, scope: &Scope) -> Result<BTreeSet<ChildResource>> {
        info!("listing eligible child resources for {scope}");
        let value = self
            .backend
            .request(Method::GET, Operation::EligibleChildResources)
            .scope(scope.clone())
            .send()
            .context("unable to list eligible child resources")?;
        ChildResource::parse(&value)
    }

    /// List all assignments (not just those managed by PIM)
    pub fn role_definitions(&self, scope: &Scope) -> Result<Vec<Definition>> {
        info!("listing role definitions for {scope}");
        let definitions = self
            .backend
            .request(Method::GET, Operation::RoleDefinitions)
            .scope(scope.clone())
            .send()
            .context("unable to list role definitions")?;
        let definitions: Definitions = serde_json::from_value(definitions)?;
        Ok(definitions.value)
    }

    pub fn delete_assignment(&self, scope: &Scope, assignment_name: &str) -> Result<()> {
        info!("deleting assignment {assignment_name} from {scope}");
        self.backend
            .request(Method::DELETE, Operation::RoleAssignments)
            .extra(format!("/{assignment_name}"))
            .scope(scope.clone())
            .send()
            .context("unable to delete assignment")?;
        Ok(())
    }

    /// Delete the specified role assignment
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub fn delete_role_assignment(&self, assignment: &RoleAssignment) -> Result<()> {
        let RoleAssignment {
            scope,
            role_definition_id,
            role,
            scope_name,
            status: _,
            name: _,
            principal_id,
            principal_type: _,
            object: _,
        } = assignment;

        let principal_id = principal_id.as_deref().context("missing principal id")?;
        info!("deleting {role} in {scope_name} ({scope})");
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
            .send()?;
        Ok(())
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
