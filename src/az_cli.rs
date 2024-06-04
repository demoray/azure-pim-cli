use anyhow::Result;
use std::process::Command;

/// Execute an Azure CLI command
///
/// # Errors
/// Will return `Err` if the Azure CLI fails
fn az_cmd(args: &[&str]) -> Result<String> {
    let output = Command::new("az").args(args).output()?;
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
