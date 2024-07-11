use crate::models::scope::Scope;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Serialize, Deserialize, PartialOrd, Ord, PartialEq, Eq, Debug)]
pub struct ChildResource {
    pub id: Scope,
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
}

impl ChildResource {
    pub(crate) fn parse(data: &Value) -> Result<BTreeSet<Self>> {
        let mut results = BTreeSet::new();

        if let Some(value) = data.get("value") {
            if let Some(value) = value.as_array() {
                for entry in value {
                    let child_resource: ChildResource = serde_json::from_value(entry.clone())?;
                    results.insert(child_resource);
                }
            }
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::ChildResource;
    use anyhow::Result;
    use insta::assert_json_snapshot;
    use serde_json::{from_str, Value};

    #[test]
    fn test_child_resource_parse() -> Result<()> {
        let data: Value = from_str(include_str!("../../tests/data/child-resources.json"))?;
        let result = ChildResource::parse(&data)?;
        assert_json_snapshot!(result);
        Ok(())
    }
}
