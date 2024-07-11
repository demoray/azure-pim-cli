use crate::{
    az_cli::{extract_oid, get_token, TokenScope},
    scope::Scope,
};
use anyhow::{anyhow, Context, Result};
use derive_setters::Setters;
use parking_lot::Mutex;
use reqwest::{
    blocking::{Client, Request},
    Method, StatusCode,
};
use retry::{
    delay::{jitter, Fixed},
    retry, OperationResult,
};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};
use tracing::{debug, trace};

const RETRY_COUNT: usize = 10;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[allow(clippy::enum_variant_names, dead_code)]
pub(crate) enum Operation {
    RoleAssignments,
    RoleAssignmentScheduleInstances,
    RoleDefinitions,
    RoleEligibilityScheduleInstances,
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
            | Self::RoleAssignmentScheduleRequests
            | Self::EligibleChildResources => TokenScope::Management,
        }
    }

    fn api_version(&self) -> &str {
        match self {
            Self::RoleAssignments | Self::RoleDefinitions => "2022-04-01",
            Self::RoleAssignmentScheduleInstances
            | Self::RoleEligibilityScheduleInstances
            | Self::RoleAssignmentScheduleRequests
            | Self::EligibleChildResources => "2020-10-01",
        }
    }
}

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

macro_rules! try_or_retry {
    ($e:expr) => {
        match $e {
            Ok(x) => x,
            Err(e) => {
                return OperationResult::Retry(anyhow::Error::from(e));
            }
        }
    };
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

    pub(crate) fn principal_id(&self) -> Result<String> {
        let mgmt_token = self.get_token(TokenScope::Management)?;
        extract_oid(&mgmt_token).context("unable to obtain the current user")
    }

    pub(crate) fn get_token(&self, scope: TokenScope) -> Result<String> {
        let mut tokens = self.tokens.lock();
        if let Some(token) = tokens.get(&scope) {
            return Ok(token.clone());
        }

        let token = get_token(scope)?;
        tokens.insert(scope, token.clone());
        Ok(token)
    }

    fn try_request(
        client: &Client,
        request: Request,
        validate: Option<for<'a> fn(StatusCode, &'a Value) -> Result<()>>,
    ) -> OperationResult<Value, anyhow::Error> {
        debug!("sending request: {request:?}");
        let response = try_or_retry!(client.execute(request));
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

    pub(crate) fn retry_request(
        &self,
        request: &Request,
        validate: Option<for<'a> fn(StatusCode, &'a Value) -> Result<()>>,
    ) -> Result<Value> {
        let retries = Fixed::from(Duration::from_secs(5))
            .map(jitter)
            .take(RETRY_COUNT);
        let operation = || {
            let Some(request) = request.try_clone() else {
                return OperationResult::Err(anyhow!("unable to clone request"));
            };
            Self::try_request(&self.client, request, validate)
        };
        retry(retries, operation).map_err(|e| e.error)
    }

    pub(crate) fn request(&self, method: Method, operation: Operation) -> RequestBuilder {
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

    pub(crate) fn send(self) -> Result<Value> {
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
            .bearer_auth(backend.get_token(operation.token_scope())?);

        if let Some(query) = query {
            builder = builder.query(&query);
        }
        if let Some(json) = json {
            builder = builder.json(&json);
        }

        let request = builder.build()?;
        backend.retry_request(&request, validate)
    }
}
