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
mod assignments;
pub mod az_cli;
mod definitions;
pub mod interactive;
mod latest;
pub mod roles;

pub use crate::latest::check_latest_version;
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
use std::{
    sync::Once,
    thread::sleep,
    time::{Duration, Instant},
};
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

const RETRY_COUNT: usize = 10;
const WAIT_DELAY: Duration = Duration::from_secs(5);

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
    Success,
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
        trace!("response body: {body:#?}");
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
        info!("listing eligible assignments");
        let url = "https://management.azure.com/providers/Microsoft.Authorization/roleEligibilityScheduleInstances";
        let response = self
            .get(url, Some(&[("$filter", "asTarget()")]))
            .context("unable to list eligible assignments")?;
        Assignments::parse(&response).context("unable to parse eligible assignments")
    }

    /// List the roles active role assignments for the current user
    pub fn list_active_assignments(&self) -> Result<Assignments> {
        info!("listing active assignments");
        let url = "https://management.azure.com/providers/Microsoft.Authorization/roleAssignmentScheduleInstances";
        let response = self
            .get(url, Some(&[("$filter", "asTarget()")]))
            .context("unable to list active assignments")?;
        Assignments::parse(&response).context("unable to parse active assignments")
    }

    /// Request extending the specified role eligibility
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub fn extend_assignment(
        &self,
        assignment: &Assignment,
        justification: &str,
        duration: Duration,
    ) -> Result<()> {
        let Assignment {
            scope,
            role_definition_id,
            role,
            scope_name,
        } = assignment;
        info!("extending {role} in {scope_name} ({scope})");
        let request_id = Uuid::now_v7();
        let url = format!("https://management.azure.com{scope}/providers/Microsoft.Authorization/roleAssignmentScheduleRequests/{request_id}");
        let body = serde_json::json!({
            "properties": {
                "principalId": self.principal_id,
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

        self.put(url, body, None::<Value>, Some(check_error_response))?;
        Ok(())
    }

    /// Activates the specified role
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub fn activate_assignment(
        &self,
        assignment: &Assignment,
        justification: &str,
        duration: Duration,
    ) -> Result<()> {
        let Assignment {
            scope,
            role_definition_id,
            role,
            scope_name,
        } = assignment;
        info!("activating {role} in {scope_name} ({scope})");
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
                        "duration": format_duration(duration)?,
                        "type": "AfterDuration",
                    }
                }
            }
        });

        self.put(url, body, None::<Value>, Some(check_error_response))?;
        Ok(())
    }

    pub fn activate_assignment_set(
        &self,
        assignments: &Assignments,
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
                |entry| match self.activate_assignment(&entry, justification, duration) {
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

        let mut failed = Assignments::default();

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
    pub fn deactivate_assignment(&self, assignment: &Assignment) -> Result<()> {
        let Assignment {
            scope,
            role_definition_id,
            role,
            scope_name,
        } = assignment;
        info!("deactivating {role} in {scope_name} ({scope})");
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
        Ok(())
    }

    pub fn deactivate_assignment_set(
        &self,
        assignments: &Assignments,
        concurrency: usize,
    ) -> Result<()> {
        ensure!(!assignments.0.is_empty(), "no roles specified");

        Self::thread_builder(concurrency);

        let results = assignments
            .0
            .clone()
            .into_par_iter()
            .map(|entry| match self.deactivate_assignment(&entry) {
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

        let mut failed = Assignments::default();

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

    pub fn wait_for_activation(
        &self,
        assignments: &Assignments,
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

            let active = self.list_active_assignments()?;
            info!("active assignments: {active:#?}");
            waiting.retain(|entry| !active.contains(entry));
            info!("still waiting: {waiting:#?}");
        }

        if !waiting.is_empty() {
            bail!(
                "timed out waiting for the following roles to activate:\n{}",
                waiting.friendly()
            );
        }

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
