#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::manual_assert)]
#![deny(clippy::indexing_slicing)]
#![allow(clippy::module_name_repetitions)]

pub mod activate;
pub mod az_cli;
pub mod interactive;
pub mod roles;

use crate::{activate::check_error_response, az_cli::get_token};
use anyhow::{anyhow, bail, ensure, Context, Result};
use az_cli::get_userid;
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
use std::{sync::Once, time::Duration};
use tracing::{debug, error, info, warn};
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
        Ok(Self {
            client: Client::new(),
            token: get_token().context("unable to obtain access token")?,
            principal_id: get_userid().context("unable to obtain the current user")?,
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
        let body = try_or_stop!(response.json());
        debug!("response json: {body:#?}");

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
        static RETRY_WARNING: Once = Once::new();

        let mut entries = Assignments(Vec::new());
        for i in 0..=RETRY_COUNT {
            debug!("attempt {i}/{RETRY_COUNT}");
            if i > 0 {
                RETRY_WARNING.call_once(|| {
                    warn!(
                        "Listing active assignments has known reliability issues. \
                        This request will retry up to {RETRY_COUNT} times until results are returned. \
                        If you continue to see no results, please try again later."
                    );
                });
            }

            let url = "https://management.azure.com/providers/Microsoft.Authorization/roleAssignmentScheduleInstances";
            let response = self
                .get(url, Some(&[("$filter", "asTarget()")]))
                .context("unable to list active assignments")?;
            entries = Assignment::parse(&response).context("unable to parse active assignments")?;

            if !entries.0.is_empty() {
                break;
            }
        }
        Ok(entries)
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
        let request_id = Uuid::new_v4();
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
                info!("activating {} in {}", entry.role, entry.scope_name);
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
}
