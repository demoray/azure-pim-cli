use anyhow::{bail, Context, Result};
use azure_core::credentials::TokenCredential;
use azure_identity::{new_executor, AzureCliCredential, AzureDeveloperCliCredential};
use azure_identity_helpers::{
    azureauth_cli_credentials::AzureauthCliCredential,
    chained_token_credential::ChainedTokenCredential, devicecode_credentials::DeviceCodeCredential,
};
use base64::prelude::{Engine, BASE64_STANDARD_NO_PAD};
use serde_json::Value;
use std::{env::home_dir, ffi::OsStr};
use tokio::fs::read;
use tracing::trace;

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub(crate) enum TokenScope {
    Management,
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

/// Read the default tenant from the azure CLI config
async fn read_default_tenant() -> Option<String> {
    let dir = home_dir()?;
    let profile = dir.join(".azure/azureProfile.json");
    let mut subscriptions = read(&profile).await.ok()?;
    if subscriptions.starts_with(&[0xEF, 0xBB, 0xBF]) {
        subscriptions.drain(..3);
    }
    let profile: Value = serde_json::from_slice(&subscriptions).ok()?;
    let subscriptions = profile.get("subscriptions")?;
    for subscription in subscriptions.as_array()? {
        if subscription.get("isDefault")?.as_bool().unwrap_or(false) {
            return subscription
                .get("tenantId")?
                .as_str()
                .map(ToString::to_string);
        }
    }
    None
}

/// Get an Oauth token for the current user
///
/// # Errors
/// Will return `Err` if the authentication fails
pub async fn get_token(scope: TokenScope) -> Result<String> {
    let mut provider = ChainedTokenCredential::new(None);
    provider.add_source(AzureCliCredential::new(None)?);
    provider.add_source(AzureDeveloperCliCredential::new(None)?);
    if let Some(tenant_id) = read_default_tenant().await {
        provider.add_source(AzureauthCliCredential::new(
            tenant_id,
            "04b07795-8ddb-461a-bbee-02f9e1bf7b46",
        )?);
    }
    provider.add_source(DeviceCodeCredential::new(
        "common",
        "04b07795-8ddb-461a-bbee-02f9e1bf7b46",
    )?);

    let token = provider
        .get_token(&[scope.to_scope_endpoint()], None)
        .await?;

    Ok(token.token.secret().to_string())
}

pub(crate) fn extract_oid(token: &str) -> Result<String> {
    trace!("identifying oid from token: {token}");
    let part = token
        .split('.')
        .nth(1)
        .context("unable to find token marker")?;
    trace!("extracted base64-header from token: {part}");
    let bytes = BASE64_STANDARD_NO_PAD
        .decode(part)
        .context("base64 decoding failed")?;
    let json: Value = serde_json::from_slice(&bytes).context("json parsing failed")?;
    trace!("parsed json from base64-decoded token: {json:?}");
    let oid = json.get("oid").context("no oid in token")?;
    trace!("extracted oid from token: {oid:?}");
    let as_str = oid.as_str().context("oid is not a string")?;
    Ok(as_str.to_string())
}

/// Find the az CLI executable
async fn find_az() -> Option<&'static OsStr> {
    #[cfg(target_os = "windows")]
    let which = "where";
    #[cfg(not(target_os = "windows"))]
    let which = "which";

    for &exe in &[OsStr::new("az.exe"), OsStr::new("az")] {
        if new_executor()
            .run(OsStr::new(which), &[exe])
            .await
            .map(|x| x.status.success())
            .unwrap_or(false)
        {
            return Some(exe);
        }
    }
    None
}

pub(crate) async fn get_signed_in_user_oid() -> Result<String> {
    let cmd = ["ad", "signed-in-user", "show", "--query", "id", "-o", "tsv"];
    let cmd = cmd.iter().map(AsRef::as_ref).collect::<Vec<&OsStr>>();
    let az_exe = find_az()
        .await
        .context("unable to find az CLI executable in PATH")?;
    let executor = new_executor();
    let result = executor
        .run(az_exe, &cmd)
        .await
        .context("failed to run az CLI")?;
    if !result.status.success() {
        bail!("az CLI returned non-zero exit code: {}", result.status);
    }
    let stdout = String::from_utf8(result.stdout).context("az CLI output was not valid UTF-8")?;
    let oid = stdout.trim();
    if oid.is_empty() {
        bail!("no signed-in user found in az CLI");
    }
    Ok(oid.to_string())
}
