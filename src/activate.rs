use crate::az_cli::get_userid;
use anyhow::{bail, Result};
use reqwest::{blocking::Client, StatusCode};
use serde_json::Value;
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

/// Activates the specified role
///
/// # Errors
/// Will return `Err` if the request fails or the response is not valid JSON
pub fn activate_role(
    token: &str,
    scope: &str,
    role_definition_id: &str,
    justification: &str,
    duration: u32,
) -> Result<()> {
    let principal_id = get_userid()?;

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
                    "duration": format!("PT{}M", duration),
                    "type": "AfterDuration",
                }
            }
        }
    });

    let response = Client::new()
        .put(url)
        .query(&[("api-version", "2020-10-01")])
        .bearer_auth(token)
        .json(&body)
        .send()?;

    let status = response.status();

    if status == StatusCode::BAD_REQUEST {
        return check_error_response(&response.json()?);
    }

    let body: Value = response.error_for_status()?.json()?;

    debug!("body: {status:#?} - {body:#?}");
    info!("submitted request: {request_id}");

    Ok(())
}
