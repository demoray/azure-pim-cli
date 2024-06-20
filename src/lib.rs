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
use anyhow::{anyhow, Context, Result};
use reqwest::{
    blocking::{Client, Request},
    IntoUrl, Method, StatusCode,
};
use retry::{
    delay::{jitter, Fixed},
    retry, OperationResult,
};
use roles::{ScopeEntry, ScopeEntryList};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tracing::debug;
use uuid::Uuid;

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

pub struct PimClient {
    client: Client,
    token: String,
}

impl PimClient {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::new(),
            token: get_token().context("unable to obtain access token")?,
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

        let retries = Fixed::from(Duration::from_secs(5)).map(jitter).take(5);
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
    pub fn list_eligible_assignments(&self) -> Result<ScopeEntryList> {
        let url = "https://management.azure.com/providers/Microsoft.Authorization/roleEligibilityScheduleInstances";
        let response = self
            .get(url, Some(&[("$filter", "asTarget()")]))
            .context("unable to list eligible assignments")?;
        ScopeEntry::parse(&response).context("unable to parse eligible assignments")
    }

    /// Activates the specified role
    ///
    /// # Errors
    /// Will return `Err` if the request fails or the response is not valid JSON
    pub fn activate_assignment(
        &self,
        principal_id: &str,
        assignment: &ScopeEntry,
        justification: &str,
        duration: u32,
    ) -> Result<Option<Uuid>> {
        let ScopeEntry {
            scope,
            role_definition_id,
            ..
        } = assignment;
        let request_id = Uuid::new_v4();
        let url = format!("https://management.azure.com{scope}/providers/Microsoft.Authorization/roleAssignmentScheduleRequests/{request_id}");
        let body = serde_json::json!({
            "properties": {
                "principalId": principal_id,
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
}
