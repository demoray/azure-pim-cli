use anyhow::{bail, Result};
use reqwest::StatusCode;
use serde_json::Value;
use tracing::info;

// NOTE: serde_json doesn't panic on failed index slicing, it returns a Value
// that allows further nested nulls
#[allow(clippy::indexing_slicing)]
pub(crate) fn check_error_response(status: StatusCode, body: &Value) -> Result<()> {
    if !status.is_success() {
        if status == StatusCode::BAD_REQUEST {
            if body["error"]["code"].as_str() == Some("RoleAssignmentExists") {
                info!("role already assigned");
                return Ok(());
            }
            if body["error"]["code"].as_str() == Some("RoleAssignmentRequestExists") {
                info!("role assignment request already exists");
                return Ok(());
            }
        }
        bail!(
            "request failed: status:{status:#?} body:{}",
            serde_json::to_string_pretty(body)?
        );
    }
    Ok(())
}
