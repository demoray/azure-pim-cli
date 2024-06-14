use anyhow::{bail, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fmt::{Display, Formatter, Result as FmtResult},
    str::FromStr,
};

#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Clone, Deserialize)]
pub struct Scope(pub String);
impl Display for Scope {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Scope {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(Self(s.to_string()))
    }
}

#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Clone, Deserialize)]
pub struct Role(pub String);
impl Display for Role {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Role {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(Self(s.to_string()))
    }
}

#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq)]
pub struct ScopeEntryList(pub Vec<ScopeEntry>);

impl ScopeEntryList {
    #[must_use]
    pub fn find(&self, role: &Role, scope: &Scope) -> Option<&ScopeEntry> {
        let scope = scope.0.to_lowercase();
        let role = role.0.to_lowercase();
        self.0
            .iter()
            .find(|v| v.role.0.to_lowercase() == role && v.scope.0.to_lowercase() == scope)
            .or_else(|| {
                self.0.iter().find(|v| {
                    v.role.0.to_lowercase() == role && v.scope_name.to_lowercase() == scope
                })
            })
    }
}

#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq)]
pub struct ScopeEntry {
    pub role: Role,
    pub scope: Scope,
    pub scope_name: String,
    #[serde(skip)]
    pub role_definition_id: String,
}

impl ScopeEntry {
    // NOTE: serde_json doesn't panic on failed index slicing, it returns a Value
    // that allows further nested nulls
    #[allow(clippy::indexing_slicing)]
    fn parse(body: &Value) -> Result<ScopeEntryList> {
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
                role: Role(role.to_string()),
                scope: Scope(scope.to_string()),
                scope_name: scope_name.to_string(),
                role_definition_id: role_definition_id.to_string(),
            });
        }
        results.sort();
        Ok(ScopeEntryList(results))
    }
}

/// List the roles available to the current user
///
/// # Errors
/// Will return `Err` if the request fails or the response is not valid JSON
pub fn list_roles(token: &str) -> Result<ScopeEntryList> {
    let url = "https://management.azure.com/providers/Microsoft.Authorization/roleEligibilityScheduleInstances";
    let response = Client::new()
        .get(url)
        .query(&[("$filter", "asTarget()"), ("api-version", "2020-10-01")])
        .bearer_auth(token)
        .send()?
        .error_for_status()?;
    ScopeEntry::parse(&response.json()?)
}
