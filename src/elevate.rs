use crate::{az_cli::get_userid, roles::ScopeEntry};
use anyhow::{bail, Context, Result};
use clap::Parser;
use reqwest::{blocking::Client, StatusCode};
use serde_json::Value;
use tracing::{debug, info};
use uuid::Uuid;

#[derive(Parser)]
pub struct ElevateConfig {
    /// Name of the role to elevate
    role: String,
    /// Scope to elevate
    scope: String,
    /// Justification for the request
    justification: String,
    /// Duration in minutes
    #[clap(long, default_value_t = 480)]
    duration: u32,
}

pub fn elevate_role(token: &str, cfg: &ElevateConfig, roles: &[ScopeEntry]) -> Result<()> {
    let scope = roles
        .iter()
        .find(|v| v.name == cfg.role && v.scope == cfg.scope)
        .context("role not found")?;

    let principal_id = get_userid()?;

    let request_id = Uuid::new_v4();
    let url = format!("https://management.azure.com{}/providers/Microsoft.Authorization/roleAssignmentScheduleRequests/{request_id}", scope.scope);
    let body = serde_json::json!({
        "properties": {
            "principalId": principal_id.to_string(),
            "roleDefinitionId": scope.role_definition_id,
            "requestType": "SelfActivate",
            "justification": cfg.justification,
            "scheduleInfo": {
                "expiration": {
                    "duration": format!("PT{}M", cfg.duration),
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
        let body: Value = response.json()?;
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

    let body: Value = response.error_for_status()?.json()?;

    debug!("body: {status:#?} - {body:#?}");
    info!("submitted request: {request_id}");

    Ok(())
}
