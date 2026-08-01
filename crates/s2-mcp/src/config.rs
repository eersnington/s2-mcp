use std::{
    env::{self, VarError},
    fs,
    path::PathBuf,
    time::Duration,
};

use s2_sdk::types::{
    AccountEndpoint, BasinEndpoint, Compression, EncryptionKey, S2Config, S2Endpoints,
};
use serde::{Deserialize, Serialize};

use crate::error::{ConfigError, Error, Result};

const USER_AGENT: &str = concat!("s2-mcp/", env!("CARGO_PKG_VERSION"));
const DEVELOPMENT_ACCESS_TOKEN: &str = "ignored";

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConnectionEnvironment {
    #[default]
    Cloud,
    ManagedLite,
    CustomEndpoint,
}

impl ConnectionEnvironment {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Cloud => "cloud",
            Self::ManagedLite => "managed_lite",
            Self::CustomEndpoint => "custom_endpoint",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum S2Compression {
    #[default]
    None,
    Gzip,
    Zstd,
}

impl From<S2Compression> for Compression {
    fn from(value: S2Compression) -> Self {
        match value {
            S2Compression::None => Self::None,
            S2Compression::Gzip => Self::Gzip,
            S2Compression::Zstd => Self::Zstd,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct S2Configuration {
    access_token: Option<String>,
    account_endpoint: Option<String>,
    basin_endpoint: Option<String>,
    encryption_key: Option<String>,
    compression: Option<S2Compression>,
    ssl_no_verify: Option<bool>,
    environment: ConnectionEnvironment,
}

pub(crate) type ConnectionConfig = S2Configuration;

impl S2Configuration {
    pub fn new(access_token: impl Into<String>) -> Self {
        Self {
            access_token: Some(access_token.into()),
            ..Self::default()
        }
    }

    pub fn with_endpoints(
        self,
        account_endpoint: impl Into<String>,
        basin_endpoint: impl Into<String>,
    ) -> Self {
        Self {
            account_endpoint: Some(account_endpoint.into()),
            basin_endpoint: Some(basin_endpoint.into()),
            environment: ConnectionEnvironment::CustomEndpoint,
            ..self
        }
    }

    pub fn with_compression(self, compression: S2Compression) -> Self {
        Self {
            compression: Some(compression),
            ..self
        }
    }

    pub fn with_encryption_key(self, encryption_key: impl Into<String>) -> Self {
        Self {
            encryption_key: Some(encryption_key.into()),
            ..self
        }
    }

    pub fn with_ssl_no_verify(self, ssl_no_verify: bool) -> Self {
        Self {
            ssl_no_verify: Some(ssl_no_verify),
            ..self
        }
    }

    pub fn load() -> Result<Self> {
        Self::load_cloud()
    }

    pub fn load_cloud() -> Result<Self> {
        let path = config_path()?;
        let mut config = if path.exists() {
            let contents = fs::read_to_string(&path).map_err(|source| {
                Error::Config(ConfigError::Read {
                    path: path.clone(),
                    source,
                })
            })?;
            toml::from_str(&contents).map_err(|source| {
                Error::Config(ConfigError::Parse {
                    path: path.clone(),
                    source,
                })
            })?
        } else {
            Self::default()
        };

        override_string("S2_ACCESS_TOKEN", &mut config.access_token)?;
        config.account_endpoint = None;
        config.basin_endpoint = None;
        config.environment = ConnectionEnvironment::Cloud;
        override_string("S2_ENCRYPTION_KEY", &mut config.encryption_key)?;
        override_bool("S2_SSL_NO_VERIFY", &mut config.ssl_no_verify)?;
        if let Some(compression) = environment_value("S2_COMPRESSION")? {
            config.compression = Some(match compression.to_ascii_lowercase().as_str() {
                "none" => S2Compression::None,
                "gzip" => S2Compression::Gzip,
                "zstd" => S2Compression::Zstd,
                _ => {
                    return Err(Error::Config(ConfigError::Invalid(format!(
                        "S2_COMPRESSION must be one of none, gzip, or zstd; received `{compression}`"
                    ))));
                }
            });
        }

        Ok(config)
    }

    pub fn load_shared_endpoint(endpoint: &str) -> Result<Self> {
        let access_token = environment_value("S2_ACCESS_TOKEN")?
            .unwrap_or_else(|| DEVELOPMENT_ACCESS_TOKEN.to_owned());
        Self::for_shared_endpoint(
            endpoint,
            access_token,
            ConnectionEnvironment::CustomEndpoint,
        )
    }

    pub fn load_development_environment() -> Result<Self> {
        let account = environment_value("S2_ACCOUNT_ENDPOINT")?.ok_or_else(|| {
            Error::Config(ConfigError::Invalid(
                "--from-env requires S2_ACCOUNT_ENDPOINT and S2_BASIN_ENDPOINT".to_owned(),
            ))
        })?;
        let basin = environment_value("S2_BASIN_ENDPOINT")?.ok_or_else(|| {
            Error::Config(ConfigError::Invalid(
                "--from-env requires S2_ACCOUNT_ENDPOINT and S2_BASIN_ENDPOINT".to_owned(),
            ))
        })?;
        let access_token = environment_value("S2_ACCESS_TOKEN")?
            .unwrap_or_else(|| DEVELOPMENT_ACCESS_TOKEN.to_owned());
        let configuration = Self::new(access_token)
            .with_endpoints(account, basin)
            .with_environment(ConnectionEnvironment::CustomEndpoint);
        configuration.sdk_config()?;
        Ok(configuration)
    }

    pub(crate) fn for_shared_endpoint(
        endpoint: &str,
        access_token: impl Into<String>,
        environment: ConnectionEnvironment,
    ) -> Result<Self> {
        S2Endpoints::for_endpoint(endpoint)
            .map_err(|error| Error::Config(ConfigError::Invalid(error.to_string())))?;
        Ok(Self::new(access_token)
            .with_endpoints(endpoint, endpoint)
            .with_environment(environment))
    }

    fn with_environment(self, environment: ConnectionEnvironment) -> Self {
        Self {
            environment,
            ..self
        }
    }

    pub fn sdk_config(&self) -> Result<S2Config> {
        let access_token = self
            .access_token
            .as_deref()
            .ok_or(Error::Config(ConfigError::MissingAccessToken))?;
        let mut config = S2Config::new(access_token)
            .with_user_agent(USER_AGENT)?
            .with_request_timeout(Duration::from_secs(30))
            .with_compression(self.compression.unwrap_or_default().into());

        match (&self.account_endpoint, &self.basin_endpoint) {
            (Some(account), Some(basin)) => {
                let account = AccountEndpoint::new(account)
                    .map_err(|error| Error::Config(ConfigError::Invalid(error.to_string())))?;
                let basin = BasinEndpoint::new(basin)
                    .map_err(|error| Error::Config(ConfigError::Invalid(error.to_string())))?;
                let endpoints = S2Endpoints::new(account, basin)
                    .map_err(|error| Error::Config(ConfigError::Invalid(error.to_string())))?;
                config = config.with_endpoints(endpoints);
            }
            (Some(_), None) => {
                tracing::warn!(
                    "account endpoint is set but basin endpoint is not; both are required for custom endpoints, using S2 Cloud defaults"
                );
            }
            (None, Some(_)) => {
                tracing::warn!(
                    "basin endpoint is set but account endpoint is not; both are required for custom endpoints, using S2 Cloud defaults"
                );
            }
            (None, None) => {}
        }

        if self.ssl_no_verify == Some(true) {
            tracing::warn!("TLS certificate verification is disabled");
            config = config.with_insecure_skip_cert_verification(true);
        }

        Ok(config)
    }

    pub(crate) fn account_endpoint_label(&self) -> &str {
        match (&self.account_endpoint, &self.basin_endpoint) {
            (Some(account), Some(_)) => account,
            _ => "S2 Cloud default",
        }
    }

    pub(crate) const fn environment_label(&self) -> &'static str {
        self.environment.label()
    }

    pub(crate) fn basin_endpoint_label(&self) -> &str {
        match (&self.account_endpoint, &self.basin_endpoint) {
            (Some(_), Some(basin)) => basin,
            _ => "S2 Cloud default",
        }
    }

    pub(crate) fn encryption_key(&self) -> Result<Option<EncryptionKey>> {
        self.encryption_key
            .as_deref()
            .map(|key| {
                key.parse().map_err(|error| {
                    Error::Config(ConfigError::Invalid(format!(
                        "S2_ENCRYPTION_KEY is invalid: {error}"
                    )))
                })
            })
            .transpose()
    }
}

fn environment_value(name: &'static str) -> Result<Option<String>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => {
            Err(Error::Config(ConfigError::InvalidEnvironment { name }))
        }
    }
}

fn override_string(name: &'static str, target: &mut Option<String>) -> Result<()> {
    if let Some(value) = environment_value(name)? {
        *target = Some(value);
    }
    Ok(())
}

fn override_bool(name: &'static str, target: &mut Option<bool>) -> Result<()> {
    if let Some(value) = environment_value(name)? {
        *target = Some(value.parse().map_err(|_| {
            Error::Config(ConfigError::Invalid(format!(
                "{name} must be either true or false; received `{value}`"
            )))
        })?);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn config_path() -> Result<PathBuf> {
    let mut path = dirs::config_dir().ok_or(Error::Config(ConfigError::DirectoryNotFound))?;
    path.push("s2");
    path.push("config.toml");
    Ok(path)
}

#[cfg(not(target_os = "windows"))]
fn config_path() -> Result<PathBuf> {
    let mut path = dirs::home_dir().ok_or(Error::Config(ConfigError::DirectoryNotFound))?;
    path.push(".config");
    path.push("s2");
    path.push("config.toml");
    Ok(path)
}
