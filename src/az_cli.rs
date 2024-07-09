use anyhow::{ensure, Context, Result};
use base64::prelude::{Engine, BASE64_STANDARD_NO_PAD};
use serde_json::Value;
use std::process::Command;

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub(crate) enum TokenScope {
    Management,
    #[allow(dead_code)]
    Graph,
}

impl TokenScope {
    fn to_scope_endpoint(self) -> &'static str {
        match self {
            Self::Management => "https://management.core.windows.net/.default",
            Self::Graph => "https://graph.microsoft.com/.default",
        }
    }
}

#[cfg(target_os = "windows")]
const AZ_CMD: &str = "az.cmd";
#[cfg(not(target_os = "windows"))]
const AZ_CMD: &str = "az";

/// Execute an Azure CLI command
///
/// # Errors
/// Will return `Err` if the Azure CLI fails
fn az_cmd(args: &[&str]) -> Result<String> {
    let output = Command::new(AZ_CMD)
        .args(args)
        .output()
        .with_context(|| format!("unable to launch {AZ_CMD}"))?;
    ensure!(
        output.status.success(),
        "az command failed {}",
        String::from_utf8(output.stderr)?
    );
    let output = String::from_utf8(output.stdout)?;
    Ok(output.trim().to_string())
}

/// Get an Oauth token from Azure CLI for the current user
///
/// # Errors
/// Will return `Err` if the Azure CLI fails
pub(crate) fn get_token(scope: TokenScope) -> Result<String> {
    az_cmd(&[
        "account",
        "get-access-token",
        "--scope",
        scope.to_scope_endpoint(),
        "--query",
        "accessToken",
        "--output",
        "tsv",
    ])
    .with_context(|| format!("unable to obtain token to {}", scope.to_scope_endpoint()))
}

pub(crate) fn extract_oid(token: &str) -> Result<String> {
    let token = BASE64_STANDARD_NO_PAD.decode(token.split('.').nth(1).context("invalid token")?)?;
    let token: Value = serde_json::from_slice(&token)?;
    Ok(token
        .get("oid")
        .context("no oid in token")?
        .as_str()
        .context("token is not string")?
        .to_string())
}
