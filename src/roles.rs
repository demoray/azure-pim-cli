use anyhow::{bail, Result};
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
pub struct ScopeEntry {
    pub name: String,
    pub scope: String,
    scope_name: String,
    #[serde(skip)]
    pub role_definition_id: String,
}

pub fn list(token: &str) -> Result<Vec<ScopeEntry>> {
    let url = "https://management.azure.com/providers/Microsoft.Authorization/roleEligibilityScheduleInstances";
    let response = Client::new()
        .get(url)
        .query(&[("$filter", "asTarget()"), ("api-version", "2020-10-01")])
        .bearer_auth(token)
        .send()?
        .error_for_status()?;
    let body: Value = response.json()?;
    let Some(values) = body["value"].as_array() else {
        bail!("unable to parse response: missing value array: {body:#?}");
    };

    let mut results = Vec::new();
    for role in values {
        let Some(name) =
            role["properties"]["expandedProperties"]["roleDefinition"]["displayName"].as_str()
        else {
            bail!("no role name: {role:#?}");
        };

        let Some(scope) = role["properties"]["expandedProperties"]["scope"]["id"].as_str() else {
            bail!("no scope id: {role:#?}");
        };

        let Some(scope_name) =
            role["properties"]["expandedProperties"]["scope"]["displayName"].as_str()
        else {
            bail!("no scope name: {role:#?}");
        };

        let Some(role_definition_id) = role["properties"]["roleDefinitionId"].as_str() else {
            bail!("no role definition id: {role:#?}");
        };

        results.push(ScopeEntry {
            name: name.to_string(),
            scope: scope.to_string(),
            scope_name: scope_name.to_string(),
            role_definition_id: role_definition_id.to_string(),
        });
    }

    Ok(results)
}
