use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter, Result as FmtResult},
    str::FromStr,
};
use uuid::Uuid;

#[derive(Debug, PartialEq, Eq)]
pub struct ParseError;

impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "unable to parse role or scope")
    }
}

impl std::error::Error for ParseError {}

#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Clone, Deserialize)]
pub struct Scope(pub String);
impl Scope {
    #[must_use]
    pub fn from_subscription(subscription_id: &Uuid) -> Self {
        Self(format!("/subscriptions/{subscription_id}"))
    }

    #[must_use]
    pub fn from_resource_group(subscription_id: &Uuid, resource_group: &str) -> Self {
        Self(format!(
            "/subscriptions/{subscription_id}/resourceGroups/{resource_group}"
        ))
    }
}

impl Display for Scope {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Scope {
    type Err = ParseError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
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
    type Err = ParseError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Default, Clone)]
pub struct Assignments(pub BTreeSet<Assignment>);

impl Assignments {
    #[must_use]
    pub fn find(&self, role: &Role, scope: &Scope) -> Option<&Assignment> {
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

    #[must_use]
    pub fn contains(&self, entry: &Assignment) -> bool {
        self.0.contains(entry)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn friendly(&self) -> String {
        self.0
            .iter()
            .map(|x| format!("* {}", x.friendly()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(crate) fn insert(&mut self, entry: Assignment) -> bool {
        self.0.insert(entry)
    }

    pub(crate) fn retain<F>(&mut self, f: F)
    where
        F: FnMut(&Assignment) -> bool,
    {
        self.0.retain(f);
    }

    // NOTE: serde_json doesn't panic on failed index slicing, it returns a Value
    // that allows further nested nulls
    #[allow(clippy::indexing_slicing)]
    pub(crate) fn parse(body: &Value) -> Result<Self> {
        let Some(values) = body["value"].as_array() else {
            bail!("unable to parse response: missing value array: {body:#?}");
        };

        let mut results = Self::default();
        for entry in values {
            let Some(role) = entry["properties"]["expandedProperties"]["roleDefinition"]
                ["displayName"]
                .as_str()
                .and_then(|x| Role::from_str(x).ok())
            else {
                bail!("no role name: {entry:#?}");
            };

            let Some(scope) = entry["properties"]["expandedProperties"]["scope"]["id"]
                .as_str()
                .and_then(|x| Scope::from_str(x).ok())
            else {
                bail!("no scope id: {entry:#?}");
            };

            let Some(scope_name) = entry["properties"]["expandedProperties"]["scope"]
                ["displayName"]
                .as_str()
                .map(ToString::to_string)
            else {
                bail!("no scope name: {entry:#?}");
            };

            let Some(role_definition_id) = entry["properties"]["roleDefinitionId"]
                .as_str()
                .map(ToString::to_string)
            else {
                bail!("no role definition id: {entry:#?}");
            };

            results.insert(Assignment {
                role,
                scope,
                scope_name,
                role_definition_id,
            });
        }

        Ok(results)
    }
}

#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Clone)]
pub struct Assignment {
    pub role: Role,
    pub scope: Scope,
    pub scope_name: String,
    #[serde(skip)]
    pub role_definition_id: String,
}

impl Assignment {
    pub(crate) fn friendly(&self) -> String {
        format!(
            "\"{}\" in \"{}\" ({})",
            self.role, self.scope_name, self.scope
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Assignments;
    use anyhow::Result;
    use insta::assert_json_snapshot;

    #[test]
    fn parse_active() -> Result<()> {
        const ASSIGNMENTS: &str = include_str!("../tests/data/role-assignments.json");
        let assignments = RoleAssignments::parse(&serde_json::from_str(ASSIGNMENTS)?)?;
        assert_json_snapshot!(&assignments);
        Ok(())
    }
}
