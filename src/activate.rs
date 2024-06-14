use crate::roles::ScopeEntry;
use anyhow::{anyhow, bail, Result};
use reqwest::{blocking::Client, StatusCode};
use retry::{
    delay::{jitter, Fixed},
    retry_with_index, OperationResult,
};
use serde_json::Value;
use std::time::Duration;
use tracing::{debug, info};
use uuid::Uuid;

// NOTE: serde_json doesn't panic on failed index slicing, it returns a Value
// that allows further nested nulls
#[allow(clippy::indexing_slicing)]
fn check_error_response(body: &Value) -> Result<()> {
    if body["error"]["code"].as_str() == Some("RoleAssignmentExists") {
        info!("role already assigned");
        return Ok(());
    }
    if body["error"]["code"].as_str() == Some("RoleAssignmentRequestExists") {
        info!("role assignment request already exists");
        return Ok(());
    }
    bail!("unable to elevate: {body:#?}");
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

/// Activates the specified role
///
/// # Errors
/// Will return `Err` if the request fails or the response is not valid JSON
pub fn activate_role(
    principal_id: &str,
    token: &str,
    entry: &ScopeEntry,
    justification: &str,
    duration: u32,
) -> Result<Option<Uuid>> {
    let ScopeEntry {
        scope,
        role_definition_id,
        ..
    } = entry;
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

    let func = |_: u64| {
        let response = try_or_stop!(Client::new()
            .put(&url)
            .query(&[("api-version", "2020-10-01")])
            .bearer_auth(token)
            .json(&body)
            .send());

        let status = response.status();

        if status == StatusCode::TOO_MANY_REQUESTS {
            return OperationResult::Retry(anyhow!("rate limited"));
        }

        if status == StatusCode::BAD_REQUEST {
            let body = try_or_stop!(response.json());
            try_or_stop!(check_error_response(&body));
            return OperationResult::Ok(None);
        }

        let response = try_or_stop!(response.error_for_status());
        let body: Value = try_or_stop!(response.json());

        debug!("body: {status:#?} - {body:#?}");
        OperationResult::Ok(Some(request_id))
    };

    let retries = Fixed::from(Duration::from_secs(5)).map(jitter).take(5);
    retry_with_index(retries, func).map_err(|e| e.error)
}
