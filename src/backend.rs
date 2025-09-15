use crate::{
    az_cli::{extract_oid, get_token, TokenScope},
    models::scope::Scope,
};
use anyhow::{bail, Context, Result};
use derive_setters::Setters;
use exponential_backoff::Backoff;
use reqwest::{Client, Method, Request, StatusCode};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};
use tokio::sync::Mutex;
use tracing::{debug, trace};

const RETRY_COUNT: u32 = 10;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[allow(clippy::enum_variant_names, dead_code)]
pub(crate) enum Operation {
    RoleAssignments,
    RoleAssignmentScheduleInstances,
    RoleDefinitions,
    RoleEligibilityScheduleInstances,
    RoleEligibilityScheduleRequests,
    RoleAssignmentScheduleRequests,
    EligibleChildResources,
}

impl Operation {
    fn as_str(&self) -> &str {
        match self {
            Self::RoleAssignments => "roleAssignments",
            Self::RoleAssignmentScheduleInstances => "roleAssignmentScheduleInstances",
            Self::RoleDefinitions => "roleDefinitions",
            Self::RoleEligibilityScheduleInstances => "roleEligibilityScheduleInstances",
            Self::RoleEligibilityScheduleRequests => "roleEligibilityScheduleRequests",
            Self::RoleAssignmentScheduleRequests => "roleAssignmentScheduleRequests",
            Self::EligibleChildResources => "eligibleChildResources",
        }
    }

    fn token_scope(self) -> TokenScope {
        match self {
            Self::RoleAssignments
            | Self::RoleAssignmentScheduleInstances
            | Self::RoleDefinitions
            | Self::RoleEligibilityScheduleInstances
            | Self::RoleEligibilityScheduleRequests
            | Self::RoleAssignmentScheduleRequests
            | Self::EligibleChildResources => TokenScope::Management,
        }
    }

    fn api_version(&self) -> &str {
        match self {
            Self::RoleAssignments | Self::RoleDefinitions => "2022-04-01",
            Self::RoleAssignmentScheduleInstances
            | Self::RoleEligibilityScheduleInstances
            | Self::RoleEligibilityScheduleRequests
            | Self::RoleAssignmentScheduleRequests
            | Self::EligibleChildResources => "2020-10-01",
        }
    }
}

pub(crate) struct Backend {
    pub(crate) client: Client,
    tokens: Mutex<BTreeMap<TokenScope, String>>,
}

impl Backend {
    pub(crate) fn new() -> Self {
        Self {
            client: Client::new(),
            tokens: Mutex::new(BTreeMap::new()),
        }
    }

    pub(crate) async fn principal_id(&self) -> Result<String> {
        let mgmt_token = self.get_token(TokenScope::Management).await?;
        extract_oid(&mgmt_token).context("unable to obtain the current user")
    }

    pub(crate) async fn get_token(&self, scope: TokenScope) -> Result<String> {
        let mut tokens = self.tokens.lock().await;
        if let Some(token) = tokens.get(&scope) {
            return Ok(token.clone());
        }

        let token = get_token(scope).await?;
        tokens.insert(scope, token.clone());
        Ok(token)
    }

    pub(crate) async fn retry_request(
        &self,
        request: &Request,
        validate: Option<for<'a> fn(StatusCode, &'a Value) -> Result<()>>,
    ) -> Result<Value> {
        let backoff = Backoff::new(RETRY_COUNT, Duration::from_secs(1), None);
        for duration in backoff {
            let Some(request) = request.try_clone() else {
                bail!("unable to clone request");
            };

            let response = self.client.execute(request).await;
            if let Ok(response) = response {
                let status = response.status();

                debug!("got status sending request: {status:?}");
                if status == StatusCode::TOO_MANY_REQUESTS {
                    bail!("rate limited");
                }

                let body = response.text().await?;
                trace!("response body: {body:#?}");
                let body = serde_json::from_str(&body)?;

                if let Some(validate) = validate {
                    validate(status, &body)?;
                    return Ok(body);
                }

                if status.is_success() {
                    return Ok(body);
                }
            }

            if let Some(duration) = duration {
                debug!("waiting {duration:?} before retrying");
                tokio::time::sleep(duration).await;
            } else {
                debug!("no more retries left");
            }
        }
        bail!("exhausted retries");
    }

    pub(crate) fn request(&self, method: Method, operation: Operation) -> RequestBuilder<'_> {
        RequestBuilder::new(self, method, operation)
    }
}

#[derive(Setters)]
#[setters(strip_option)]
pub(crate) struct RequestBuilder<'a> {
    backend: &'a Backend,
    method: Method,
    operation: Operation,
    extra: Option<String>,
    scope: Option<Scope>,
    #[setters(skip)]
    query: Option<Vec<(String, String)>>,
    json: Option<Value>,
    validate: Option<fn(StatusCode, &Value) -> Result<()>>,
}

impl<'a> RequestBuilder<'a> {
    pub(crate) fn new(backend: &'a Backend, method: Method, operation: Operation) -> Self {
        Self {
            backend,
            method,
            operation,
            extra: None,
            scope: None,
            query: None,
            json: None,
            validate: None,
        }
    }

    pub(crate) fn query<K, V>(mut self, key: K, value: V) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.query
            .get_or_insert_with(Vec::new)
            .push((key.into(), value.into()));
        self
    }

    pub(crate) async fn send(self) -> Result<Value> {
        let Self {
            backend,
            method,
            operation,
            extra,
            scope,
            query,
            json,
            validate,
        } = self;

        let scope = scope.map(|x| x.0).unwrap_or_default();
        let extra = extra.unwrap_or_default();
        let url = format!(
            "https://management.azure.com{scope}/providers/Microsoft.Authorization/{}{extra}",
            operation.as_str()
        );

        let mut builder = backend
            .client
            .request(method, url)
            .query(&[("api-version", operation.api_version())])
            .header("X-Ms-Command-Name", "Microsoft_Azure_PIMCommon.")
            .bearer_auth(backend.get_token(operation.token_scope()).await?);

        if let Some(query) = query {
            builder = builder.query(&query);
        }
        if let Some(json) = json {
            builder = builder.json(&json);
        }

        let request = builder.build()?;
        backend.retry_request(&request, validate).await
    }
}
