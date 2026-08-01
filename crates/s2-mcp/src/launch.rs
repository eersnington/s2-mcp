use crate::{Error, Result, S2Configuration, error::ConfigError};
use tokio::sync::OnceCell;

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
    connection: RuntimeConnection,
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "The managed variant retains the S2 Lite RAII guard for the runtime lifetime"
)]
enum RuntimeConnection {
    Ready(S2Configuration),
    Managed(OnceCell<ManagedConnection>),
}

#[derive(Debug)]
struct ManagedConnection {
    configuration: S2Configuration,
    #[expect(
        dead_code,
        reason = "The guard owns container cleanup through its Drop implementation"
    )]
    container_guard: s2_testcontainers::S2Lite,
}

impl ResolvedRuntime {
    pub fn resolve(intent: LaunchIntent) -> Result<Self> {
        match intent {
            LaunchIntent::Cloud => Ok(Self::ready(S2Configuration::load_cloud()?)),
            LaunchIntent::Dev(DevSource::Managed) => Ok(Self {
                connection: RuntimeConnection::Managed(OnceCell::new()),
            }),
            LaunchIntent::Dev(DevSource::Endpoint(endpoint)) => Ok(Self::ready(
                S2Configuration::load_shared_endpoint(&endpoint)?,
            )),
            LaunchIntent::Dev(DevSource::Environment) => {
                Ok(Self::ready(S2Configuration::load_development_environment()?))
            }
        }
    }

    pub(crate) fn from_configuration(configuration: S2Configuration) -> Self {
        Self::ready(configuration)
    }

    pub(crate) async fn configuration(&self) -> Result<&S2Configuration> {
        match &self.connection {
            RuntimeConnection::Ready(configuration) => Ok(configuration),
            RuntimeConnection::Managed(cell) => {
                let managed = cell
                    .get_or_try_init(|| async {
                        let lite = s2_testcontainers::S2Lite::start().await.map_err(|source| {
                            Error::Config(ConfigError::StartManagedLite { source })
                        })?;
                        let configuration = S2Configuration::for_shared_endpoint(
                            lite.endpoint(),
                            LITE_ACCESS_TOKEN,
                            crate::config::ConnectionEnvironment::ManagedLite,
                        )?;
                        Ok::<ManagedConnection, Error>(ManagedConnection {
                            configuration,
                            container_guard: lite,
                        })
                    })
                    .await?;
                Ok(&managed.configuration)
            }
        }
    }

    fn ready(configuration: S2Configuration) -> Self {
        Self {
            connection: RuntimeConnection::Ready(configuration),
        }
    }
}
