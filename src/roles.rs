use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fmt::{Display, Formatter, Result as FmtResult},
    str::FromStr,
};

#[derive(Debug, PartialEq, Eq)]
pub struct ParseError;

impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "unable to parse role or scope")
    }
}

impl std::error::Error for ParseError {
}


#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Clone, Deserialize)]
pub struct Scope(pub String);
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

#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Default)]
pub struct Assignments(pub Vec<Assignment>);

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
    // NOTE: serde_json doesn't panic on failed index slicing, it returns a Value
    // that allows further nested nulls
    #[allow(clippy::indexing_slicing)]
    pub(crate) fn parse(body: &Value) -> Result<Assignments> {
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
        Ok(Assignments(results))
    }
}

#[cfg(test)]
mod tests {
    use super::Assignment;
    use anyhow::Result;
    use insta::assert_json_snapshot;
    use serde_json::json;

    #[test]
    fn parse_active() -> Result<()> {
        let value = json!({
          "value": [
            {
              "id": "/subscriptions/00000000-0000-0000-0000-000000000001/providers/Microsoft.Authorization/roleAssignmentScheduleInstances/00000000-0000-0000-0000-000000000003",
              "name": "00000000-0000-0000-0000-000000000003",
              "properties": {
                "assignmentType": "Activated",
                "createdOn": "2024-06-19T15:53:15.98Z",
                "endDateTime": "2024-06-19T23:53:12.377Z",
                "expandedProperties": {
                  "principal": {
                    "displayName": "USERNAME",
                    "email": "user@contoso.com",
                    "id": "00000000-0000-0000-0000-000000000002",
                    "type": "User"
                  },
                  "roleDefinition": {
                    "displayName": "Custom Role Name",
                    "id": "/subscriptions/00000000-0000-0000-0000-000000000001/providers/Microsoft.Authorization/roleDefinitions/00000000-0000-0000-0000-000000000004",
                    "type": "CustomRole"
                  },
                  "scope": {
                    "displayName": "azure-sub-name",
                    "id": "/subscriptions/00000000-0000-0000-0000-000000000001",
                    "type": "subscription"
                  }
                },
                "linkedRoleEligibilityScheduleId": "/subscriptions/00000000-0000-0000-0000-000000000001/providers/Microsoft.Authorization/roleEligibilitySchedules/00000000-0000-0000-0000-000000000005",
                "linkedRoleEligibilityScheduleInstanceId": "/subscriptions/00000000-0000-0000-0000-000000000001/providers/Microsoft.Authorization/roleEligibilityScheduleInstances/00000000-0000-0000-0000-000000000006",
                "memberType": "Group",
                "originRoleAssignmentId": "/subscriptions/00000000-0000-0000-0000-000000000001/providers/Microsoft.Authorization/roleAssignments/00000000-0000-0000-0000-000000000003",
                "principalId": "00000000-0000-0000-0000-000000000002",
                "principalType": "User",
                "roleAssignmentScheduleId": "/subscriptions/00000000-0000-0000-0000-000000000001/providers/Microsoft.Authorization/roleAssignmentSchedules/00000000-0000-0000-0000-000000000007",
                "roleDefinitionId": "/subscriptions/00000000-0000-0000-0000-000000000001/providers/Microsoft.Authorization/roleDefinitions/00000000-0000-0000-0000-000000000004",
                "scope": "/subscriptions/00000000-0000-0000-0000-000000000001",
                "startDateTime": "2024-06-19T15:53:15.98Z",
                "status": "Provisioned"
              },
              "type": "Microsoft.Authorization/roleAssignmentScheduleInstances"
            }
          ]
        });

        let assignments = Assignment::parse(&value)?;
        assert_json_snapshot!(&assignments);
        Ok(())
    }
}
