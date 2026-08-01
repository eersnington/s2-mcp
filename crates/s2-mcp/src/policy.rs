use serde::{Deserialize, Serialize};

use crate::error::{Error, PolicyError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    Read,
    Write,
    Destructive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Global,
    Account,
    Basin,
    Stream,
    Dynamic { applicable_under_basin: bool },
}

impl Scope {
    const fn applicable_under_basin(self) -> bool {
        match self {
            Self::Account => false,
            Self::Dynamic {
                applicable_under_basin,
            } => applicable_under_basin,
            Self::Global | Self::Basin | Self::Stream => true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Policy {
    pub readonly: bool,
    pub basin: Option<String>,
    pub allow_destructive: bool,
}

impl Policy {
    pub(crate) fn allows(&self, access: Access, scope: Scope) -> bool {
        if self.basin.is_some() && !scope.applicable_under_basin() {
            return false;
        }

        match access {
            Access::Read => true,
            Access::Write => !self.readonly,
            Access::Destructive => self.allows_destructive(),
        }
    }

    pub(crate) const fn allows_destructive(&self) -> bool {
        !self.readonly && self.allow_destructive
    }

    pub(crate) fn enforce_operation(&self, access: Access, scope: Scope) -> Result<()> {
        if !self.allows(access, scope) {
            return Err(Error::Policy(PolicyError::Forbidden));
        }
        Ok(())
    }

    pub(crate) fn enforce_basin(&self, requested: &str) -> Result<()> {
        if let Some(allowed) = &self.basin
            && requested != allowed
        {
            return Err(Error::Policy(PolicyError::BasinScope {
                requested: requested.to_owned(),
                allowed: allowed.clone(),
            }));
        }
        Ok(())
    }
}
