use anyhow::{Context, Result};
use std::process::Command;

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
    let output = String::from_utf8(output.stdout)?;
    Ok(output.trim().to_string())
}

/// Get an Oauth token from Azure CLI for the current user
///
/// # Errors
/// Will return `Err` if the Azure CLI fails
pub fn get_token() -> Result<String> {
    az_cmd(&[
        "account",
        "get-access-token",
        "--scope=https://management.core.windows.net/.default",
        "--query",
        "accessToken",
        "--output",
        "tsv",
    ])
}

/// Get the user id for the currently logged in user from the Azure CLI
///
/// # Errors
/// Will return `Err` if the Azure CLI fails
pub fn get_userid() -> Result<String> {
    az_cmd(&[
        "ad",
        "signed-in-user",
        "show",
        "--query",
        "id",
        "--output",
        "tsv",
    ])
}
