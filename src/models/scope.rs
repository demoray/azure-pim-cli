use anyhow::Result;
use clap::Args;
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter, Result as FmtResult},
    str::FromStr,
};
use uuid::Uuid;

#[derive(thiserror::Error, Debug)]
pub enum ScopeError {
    #[error("scope must start with a /")]
    LeadingSlash,
}

#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Clone, Deserialize, Hash)]
pub struct Scope(pub(crate) String);
impl Scope {
    pub fn new<S: Into<String>>(value: S) -> Result<Self, ScopeError> {
        let value = value.into();
        if !value.starts_with('/') {
            return Err(ScopeError::LeadingSlash);
        }
        Ok(Self(value))
    }

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

    #[must_use]
    pub fn from_provider(subscription_id: &Uuid, resource_group: &str, provider: &str) -> Self {
        Self(format!(
            "/subscriptions/{subscription_id}/resourceGroups/{resource_group}/providers/{provider}"
        ))
    }

    #[must_use]
    pub fn is_subscription(&self) -> bool {
        self.0.starts_with("/subscriptions/") && !self.0.contains("/resourceGroups/")
    }

    #[must_use]
    pub fn subscription(&self) -> Option<Uuid> {
        let entries = self.0.split('/').collect::<Vec<_>>();
        let first = entries.get(1)?;
        if first != &"subscriptions" {
            return None;
        }
        let id = entries.get(2)?;
        Uuid::parse_str(id).ok()
    }

    #[must_use]
    pub fn contains(&self, other: &Self) -> bool {
        let first = self.0.split('/').collect::<Vec<_>>();
        let second = other.0.split('/').collect::<Vec<_>>();

        let left = Some(&first[..]);
        let right = second.get(0..first.len());

        left == right
    }
}

impl Display for Scope {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Scope {
    type Err = ScopeError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::new(s.to_string())
    }
}

#[derive(Args)]
#[command(about = None)]
pub struct ScopeBuilder {
    /// Specify scope at the subscription level
    #[arg(long)]
    subscription: Option<Uuid>,

    /// Specify scope at the Resource Group level
    ///
    /// This argument requires `subscription` to be set.
    #[arg(long, requires = "subscription")]
    resource_group: Option<String>,

    /// Specify scope at the Resource Provider level
    ///
    /// This argument requires `subscription` and `resource_group` to be set.
    #[arg(long, requires = "resource_group")]
    provider: Option<String>,

    /// Specify the full scope directly
    #[arg(long, conflicts_with = "subscription")]
    scope: Option<Scope>,
}

impl ScopeBuilder {
    #[must_use]
    pub fn build(self) -> Option<Scope> {
        let Self {
            subscription,
            resource_group,
            provider,
            scope,
        } = self;

        match (subscription, resource_group, provider, scope) {
            (Some(subscription), Some(group), Some(provider), None) => {
                Some(Scope::from_provider(&subscription, &group, &provider))
            }
            (Some(subscription), Some(group), None, None) => {
                Some(Scope::from_resource_group(&subscription, &group))
            }
            (Some(subscription), None, None, None) => Some(Scope::from_subscription(&subscription)),
            (None, None, None, Some(scope)) => Some(scope),
            (None, None, None, None) => None,
            _ => {
                unreachable!("invalid combination of arguments provided");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::models::scope::Scope;

    #[test]
    fn test_contains() {
        let with_provider = Scope("/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/rg/providers/provider".to_string());
        let with_rg1 = Scope(
            "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/rg".to_string(),
        );
        let with_rg2 = Scope(
            "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/r".to_string(),
        );
        let with_sub1 = Scope("/subscriptions/00000000-0000-0000-0000-000000000000".to_string());
        let with_sub2 = Scope("/subscriptions/00000000-0000-0000-0000-000000000001".to_string());

        assert!(with_rg1.contains(&with_provider));
        assert!(with_rg1.contains(&with_rg1));

        assert!(!with_provider.contains(&with_rg1));
        assert!(!with_rg2.contains(&with_provider));

        assert!(with_sub1.contains(&with_provider));
        assert!(with_sub1.contains(&with_rg1));
        assert!(with_sub1.contains(&with_rg2));
        assert!(with_sub1.contains(&with_sub1));
        assert!(!with_sub1.contains(&with_sub2));
    }
}
