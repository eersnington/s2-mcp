use crate::{Error, Result, S2Configuration};

const LITE_ACCESS_TOKEN: &str = "ignored";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchIntent {
    Cloud,
    Dev(DevSource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevSource {
    Managed,
    Endpoint(String),
    Environment,
}

#[derive(Debug)]
pub struct ResolvedRuntime {
    configuration: S2Configuration,
    managed_lite: Option<s2_testcontainers::S2Lite>,
}

impl ResolvedRuntime {
    pub async fn resolve(intent: LaunchIntent) -> Result<Self> {
        match intent {
            LaunchIntent::Cloud => Ok(Self {
                configuration: S2Configuration::load_cloud()?,
                managed_lite: None,
            }),
            LaunchIntent::Dev(DevSource::Managed) => {
                let lite = s2_testcontainers::S2Lite::start()
                    .await
                    .map_err(|source| Error::StartManagedLite { source })?;
                let configuration = S2Configuration::for_shared_endpoint(
                    lite.endpoint(),
                    LITE_ACCESS_TOKEN,
                    crate::config::ConnectionEnvironment::ManagedLite,
                )?;
                Ok(Self {
                    configuration,
                    managed_lite: Some(lite),
                })
            }
            LaunchIntent::Dev(DevSource::Endpoint(endpoint)) => Ok(Self {
                configuration: S2Configuration::load_shared_endpoint(&endpoint)?,
                managed_lite: None,
            }),
            LaunchIntent::Dev(DevSource::Environment) => Ok(Self {
                configuration: S2Configuration::load_development_environment()?,
                managed_lite: None,
            }),
        }
    }

    pub fn configuration(&self) -> &S2Configuration {
        &self.configuration
    }

    pub fn is_managed_lite(&self) -> bool {
        self.managed_lite.is_some()
    }
}
