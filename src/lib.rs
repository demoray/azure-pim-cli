#![forbid(
    unsafe_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::manual_assert
)]
#![deny(clippy::indexing_slicing)]
#![allow(clippy::module_name_repetitions)]

pub mod activate;
pub mod az_cli;
pub mod interactive;
pub mod roles;

use crate::{
    activate::check_error_response,
    az_cli::{extract_oid, get_token},
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use rayon::{prelude::*, ThreadPoolBuilder};
use reqwest::{
    blocking::{Client, Request},
    IntoUrl, Method, StatusCode,
};
use retry::{
    delay::{jitter, Fixed},
    retry, OperationResult,
};
use roles::{Assignment, Assignments};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tracing::{debug, error, info};
use uuid::Uuid;

const RETRY_COUNT: usize = 10;

macro_rules! try_or_stop {
    ($e:expr) => {
        match $e {
            Ok(x) => x,
            Err(e) => {
                return OperationResult::Err(anyhow::Error::from(e));
            }
        }
    };
}

pub enum ActivationResult {
    Existing,
    Submitted(Assignment),
    Failed(Assignment),
}

pub struct PimClient {
    client: Client,
    token: String,
    principal_id: String,
}

impl PimClient {
    pub fn new() -> Result<Self> {
        let token = get_token().context("unable to obtain access token")?;
        let principal_id = extract_oid(&token).context("unable to obtain the current user")?;
        Ok(Self {
            client: Client::new(),
            token,
            principal_id,
        })
    }

    fn try_request(
        client: &Client,
        request: Request,
        validate: Option<for<'a> fn(StatusCode, &'a Value) -> Result<()>>,
    ) -> OperationResult<Value, anyhow::Error> {
        debug!("sending request: {request:?}");
        let response = try_or_stop!(client.execute(request));
        let status = response.status();

        debug!("got status sending request: {status:?}");
        if status == StatusCode::TOO_MANY_REQUESTS {
            return OperationResult::Retry(anyhow!("rate limited"));
        }

        debug!("getting response json");
        let body = try_or_stop!(response.text());
        debug!("response body: {body:#?}");
        let body = try_or_stop!(serde_json::from_str(&body));

        if let Some(validate) = validate {
            try_or_stop!(validate(status, &body));
            return OperationResult::Ok(body);
        }

        if !status.is_success() {
            return OperationResult::Err(anyhow!("request failed: status: {status} {body:#?}"));
        }

        OperationResult::Ok(body)
    }

    pub(crate) fn request<U: IntoUrl, Q: Serialize + Sized, B: Serialize + Sized>(
        &self,
        method: Method,
        url: U,
        query: Option<Q>,
        json: Option<B>,
        validate: Option<for<'a> fn(StatusCode, &'a Value) -> Result<()>>,
    ) -> Result<Value> {
        let mut builder = self
            .client
            .request(method, url)
            .query(&[("api-version", "2020-10-01")])
            .header("X-Ms-Command-Name", "Microsoft_Azure_PIMCommon.")
            .bearer_auth(&self.token);

        if let Some(query) = query {
            builder = builder.query(&query);
        }
        if let Some(json) = json {
            builder = builder.json(&json);
        }

        let request = builder.build()?;

        let retries = Fixed::from(Duration::from_secs(5))
            .map(jitter)
            .take(RETRY_COUNT);
        retry(retries, || {
            let Some(request) = request.try_clone() else {
                return OperationResult::Err(anyhow!("unable to clone request"));
            };
            Self::try_request(&self.client, request, validate)
        })
        .map_err(|e| e.error)
    }

    pub(crate) fn get<U: IntoUrl, Q: Serialize + Sized>(
        &self,
        url: U,
        query: Option<Q>,
    ) -> Result<Value> {
        self.request(
            Method::GET,
            url,
            query,
            None::<Value>,
            None::<fn(StatusCode, &Value) -> Result<()>>,
        )
    }

    pub(crate) fn put<U: IntoUrl, Q: Serialize + Sized, B: Serialize + Sized>(
        &self,
        url: U,
        body: B,
        query: Option<Q>,
        validate: Option<for<'a> fn(StatusCode, &'a Value) -> Result<()>>,
    ) -> Result<Value> {
        self.request(Method::PUT, url, query, Some(body), validate)
    }

    /// List the roles available to the current user
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub fn list_eligible_assignments(&self) -> Result<Assignments> {
        let url = "https://management.azure.com/providers/Microsoft.Authorization/roleEligibilityScheduleInstances";
        let response = self
            .get(url, Some(&[("$filter", "asTarget()")]))
            .context("unable to list eligible assignments")?;
        Assignment::parse(&response).context("unable to parse eligible assignments")
    }

    /// List the roles active role assignments for the current user
    pub fn list_active_assignments(&self) -> Result<Assignments> {
        info!("listing active assignments");
        let url = "https://management.azure.com/providers/Microsoft.Authorization/roleAssignmentScheduleInstances";
        let response = self
            .get(url, Some(&[("$filter", "asTarget()")]))
            .context("unable to list active assignments")?;
        Assignment::parse(&response).context("unable to parse active assignments")
    }

    /// Activates the specified role
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub fn activate_assignment(
        &self,
        assignment: &Assignment,
        justification: &str,
        duration: u32,
    ) -> Result<Option<Uuid>> {
        let Assignment {
            scope,
            role_definition_id,
            ..
        } = assignment;
        let request_id = Uuid::now_v7();
        let url = format!("https://management.azure.com{scope}/providers/Microsoft.Authorization/roleAssignmentScheduleRequests/{request_id}");
        let body = serde_json::json!({
            "properties": {
                "principalId": self.principal_id,
                "roleDefinitionId": role_definition_id,
                "requestType": "SelfActivate",
                "justification": justification,
                "scheduleInfo": {
                    "expiration": {
                        "duration": format!("PT{duration}M"),
                        "type": "AfterDuration",
                    }
                }
            }
        });

        self.put(url, body, None::<Value>, Some(check_error_response))?;
        Ok(Some(request_id))
    }

    pub fn activate_assignment_set(
        &self,
        assignments: &Assignments,
        justification: &str,
        duration: u32,
        concurrency: usize,
    ) -> Result<Assignments> {
        ensure!(!assignments.0.is_empty(), "no roles specified");

        ThreadPoolBuilder::new()
            .num_threads(concurrency)
            .build_global()?;

        let results = assignments
            .0
            .clone()
            .into_par_iter()
            .map(|entry| {
                info!(
                    "activating {} in {} ({})",
                    entry.role, entry.scope_name, entry.scope
                );
                match self.activate_assignment(&entry, justification, duration) {
                    Ok(Some(request_id)) => {
                        info!("submitted request: {request_id}");
                        ActivationResult::Submitted(entry)
                    }
                    Ok(None) => ActivationResult::Existing,
                    Err(error) => {
                        error!(
                            "scope: {} definition: {} error: {error:?}",
                            entry.scope, entry.role_definition_id
                        );
                        ActivationResult::Failed(entry)
                    }
                }
            })
            .collect::<Vec<_>>();

        let mut failed = vec![];
        let mut submitted = vec![];

        for result in results {
            match result {
                ActivationResult::Failed(entry) => {
                    failed.push(format!("* {} in {}", entry.role, entry.scope_name));
                }
                ActivationResult::Submitted(entry) => submitted.push(entry),
                ActivationResult::Existing => {}
            }
        }

        if !failed.is_empty() {
            bail!(
                "failed to activate the following roles:\n{}",
                failed.join("\n")
            );
        }

        Ok(Assignments(submitted))
    }

    /// Deactivate the specified role
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub fn deactivate_assignment(&self, assignment: &Assignment) -> Result<Uuid> {
        let Assignment {
            scope,
            role_definition_id,
            ..
        } = assignment;
        let request_id = Uuid::now_v7();
        let url = format!("https://management.azure.com{scope}/providers/Microsoft.Authorization/roleAssignmentScheduleRequests/{request_id}");
        let body = serde_json::json!({
            "properties": {
                "principalId": self.principal_id,
                "roleDefinitionId": role_definition_id,
                "requestType": "SelfDeactivate",
                "justification": "Deactivation request",
            }
        });

        self.put(url, body, None::<Value>, Some(check_error_response))?;
        Ok(request_id)
    }

    pub fn deactivate_assignment_set(
        &self,
        assignments: &Assignments,
        concurrency: usize,
    ) -> Result<Assignments> {
        ensure!(!assignments.0.is_empty(), "no roles specified");

        ThreadPoolBuilder::new()
            .num_threads(concurrency)
            .build_global()?;

        let results = assignments
            .0
            .clone()
            .into_par_iter()
            .map(|entry| {
                info!(
                    "deactivating {} in {} ({})",
                    entry.role, entry.scope_name, entry.scope
                );
                match self.deactivate_assignment(&entry) {
                    Ok(request_id) => {
                        info!("submitted request: {request_id}");
                        ActivationResult::Submitted(entry)
                    }
                    Err(error) => {
                        error!(
                            "scope: {} definition: {} error: {error:?}",
                            entry.scope, entry.role_definition_id
                        );
                        ActivationResult::Failed(entry)
                    }
                }
            })
            .collect::<Vec<_>>();

        let mut failed = vec![];
        let mut submitted = vec![];

        for result in results {
            match result {
                ActivationResult::Failed(entry) => {
                    failed.push(format!("* {} in {}", entry.role, entry.scope_name));
                }
                ActivationResult::Submitted(entry) => submitted.push(entry),
                ActivationResult::Existing => {}
            }
        }

        if !failed.is_empty() {
            bail!(
                "failed to deactivate the following roles:\n{}",
                failed.join("\n")
            );
        }

        Ok(Assignments(submitted))
    }
}
