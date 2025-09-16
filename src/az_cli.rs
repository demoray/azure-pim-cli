use anyhow::{Context, Result};
use azure_core::credentials::TokenCredential;
use azure_identity::{AzureCliCredential, AzureDeveloperCliCredential};
use azure_identity_helpers::{
    azureauth_cli_credentials::AzureauthCliCredential,
    chained_token_credential::ChainedTokenCredential, devicecode_credentials::DeviceCodeCredential,
};
use base64::prelude::{Engine, BASE64_STANDARD_NO_PAD};
use home::home_dir;
use serde_json::Value;
use tokio::fs::read;

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
    let token = BASE64_STANDARD_NO_PAD.decode(token.split('.').nth(1).context("invalid token")?)?;
    let token: Value = serde_json::from_slice(&token)?;
    Ok(token
        .get("oid")
        .context("no oid in token")?
        .as_str()
        .context("token is not string")?
        .to_string())
}
