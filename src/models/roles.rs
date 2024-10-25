use crate::{
    graph::Object,
    models::scope::{Scope, ScopeError},
};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fmt::{Display, Formatter, Result as FmtResult},
    str::FromStr,
};

#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Clone, Deserialize)]
pub struct Role(pub String);
impl Display for Role {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Role {
    type Err = ScopeError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

pub trait RolesExt {
    fn find_role(&self, role: &Role, scope: &Scope) -> Option<RoleAssignment>;
    fn friendly(&self) -> String;
}

impl RolesExt for &BTreeSet<RoleAssignment> {
    #[must_use]
    fn find_role(&self, role: &Role, scope: &Scope) -> Option<RoleAssignment> {
        let role = role.0.to_lowercase();
        self.iter()
            .find(|v| v.role.0.to_lowercase() == role && &v.scope == scope)
            .cloned()
    }

    fn friendly(&self) -> String {
        self.iter()
            .map(|x| format!("* {}", x.friendly()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl RolesExt for BTreeSet<RoleAssignment> {
    #[must_use]
    fn find_role(&self, role: &Role, scope: &Scope) -> Option<RoleAssignment> {
        (&self).find_role(role, scope)
    }

    fn friendly(&self) -> String {
        (&self).friendly()
    }
}

#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Clone)]
pub struct RoleAssignment {
    pub role: Role,
    pub scope: Scope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_name: Option<String>,
    #[serde(skip)]
    pub role_definition_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub principal_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<Object>,
}

impl RoleAssignment {
    pub(crate) fn friendly(&self) -> String {
        if let Some(scope_name) = self.scope_name.as_ref() {
            format!("\"{}\" in \"{}\" ({})", self.role, scope_name, self.scope)
        } else {
            format!("\"{}\" in {}", self.role, self.scope)
        }
    }

    // NOTE: serde_json doesn't panic on failed index slicing, it returns a Value
    // that allows further nested nulls
    #[allow(clippy::indexing_slicing)]
    pub(crate) fn parse(body: &Value, with_principal: bool) -> Result<BTreeSet<Self>> {
        let Some(values) = body["value"].as_array() else {
            bail!("unable to parse response: missing value array: {body:#?}");
        };

        let mut results = BTreeSet::new();
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

            let scope_name = entry["properties"]["expandedProperties"]["scope"]["displayName"]
                .as_str()
                .map(ToString::to_string);

            let Some(role_definition_id) = entry["properties"]["roleDefinitionId"]
                .as_str()
                .map(ToString::to_string)
            else {
                bail!("no role definition id: {entry:#?}");
            };

            let (principal_id, principal_type) = if with_principal {
                let principal_id = entry["properties"]["principalId"]
                    .as_str()
                    .map(ToString::to_string);

                let principal_type = entry["properties"]["principalType"]
                    .as_str()
                    .map(ToString::to_string);
                (principal_id, principal_type)
            } else {
                (None, None)
            };

            results.insert(Self {
                role,
                scope,
                scope_name,
                role_definition_id,
                principal_id,
                principal_type,
                object: None,
            });
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::{RoleAssignment, Scope};
    use anyhow::Result;
    use insta::assert_json_snapshot;
    use uuid::Uuid;

    #[test]
    fn parse_active() -> Result<()> {
        const ASSIGNMENTS: &str = include_str!("../../tests/data/role-assignments.json");
        let assignments = RoleAssignment::parse(&serde_json::from_str(ASSIGNMENTS)?, false)?;
        assert_json_snapshot!(&assignments);
        let assignments = RoleAssignment::parse(&serde_json::from_str(ASSIGNMENTS)?, true)?;
        assert_json_snapshot!(&assignments);
        Ok(())
    }

    #[test]
    fn test_scope() {
        let uuid = Uuid::now_v7();
        let scope = Scope::from_subscription(&uuid);
        assert!(scope.is_subscription());
        assert_eq!(scope.subscription(), Some(uuid));
        let scope = Scope::from_resource_group(&uuid, "rg");
        assert!(!scope.is_subscription());
        assert_eq!(scope.subscription(), Some(uuid));
    }
}
