use anyhow::{bail, Result};
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
pub struct ScopeEntry {
    pub role: String,
    pub scope: String,
    pub scope_name: String,
    #[serde(skip)]
    pub role_definition_id: String,
}

impl ScopeEntry {
    // NOTE: serde_json doesn't panic on failed index slicing, it returns a Value
    // that allows further nested nulls
    #[allow(clippy::indexing_slicing)]
    fn parse(body: &Value) -> Result<Vec<Self>> {
        let Some(values) = body["value"].as_array() else {
            bail!("unable to parse response: missing value array: {body:#?}");
        };

        let mut results = Vec::new();
        for entry in values {
            let Some(role) =
                entry["properties"]["expandedProperties"]["roleDefinition"]["displayName"].as_str()
            else {
                bail!("no role name: {entry:#?}");
            };

            let Some(scope) = entry["properties"]["expandedProperties"]["scope"]["id"].as_str()
            else {
                bail!("no scope id: {entry:#?}");
            };

            let Some(scope_name) =
                entry["properties"]["expandedProperties"]["scope"]["displayName"].as_str()
            else {
                bail!("no scope name: {entry:#?}");
            };

            let Some(role_definition_id) = entry["properties"]["roleDefinitionId"].as_str() else {
                bail!("no role definition id: {entry:#?}");
            };

            results.push(Self {
                role: role.to_string(),
                scope: scope.to_string(),
                scope_name: scope_name.to_string(),
                role_definition_id: role_definition_id.to_string(),
            });
        }
        Ok(results)
    }
}

pub fn list_roles(token: &str) -> Result<Vec<ScopeEntry>> {
    let url = "https://management.azure.com/providers/Microsoft.Authorization/roleEligibilityScheduleInstances";
    let response = Client::new()
        .get(url)
        .query(&[("$filter", "asTarget()"), ("api-version", "2020-10-01")])
        .bearer_auth(token)
        .send()?
        .error_for_status()?;
    ScopeEntry::parse(&response.json()?)
}
