use std::{env::VarError, path::PathBuf, time::Duration};

use s2_sdk::types::{
    AccountEndpoint, BasinEndpoint, Compression, EncryptionKey, S2Config, S2Endpoints,
};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const USER_AGENT: &str = concat!("s2-mcp/", env!("CARGO_PKG_VERSION"));

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
        let path = config_path()?;
        let mut config = if path.exists() {
            let contents = std::fs::read_to_string(&path).map_err(|source| Error::ReadConfig {
                path: path.clone(),
                source,
            })?;
            toml::from_str(&contents).map_err(|source| Error::ParseConfig {
                path: path.clone(),
                source,
            })?
        } else {
            Self::default()
        };

        override_string("S2_ACCESS_TOKEN", &mut config.access_token)?;
        override_string("S2_ACCOUNT_ENDPOINT", &mut config.account_endpoint)?;
        override_string("S2_BASIN_ENDPOINT", &mut config.basin_endpoint)?;
        override_string("S2_ENCRYPTION_KEY", &mut config.encryption_key)?;
        override_bool("S2_SSL_NO_VERIFY", &mut config.ssl_no_verify)?;
        if let Some(compression) = environment_value("S2_COMPRESSION")? {
            config.compression = Some(match compression.to_ascii_lowercase().as_str() {
                "none" => S2Compression::None,
                "gzip" => S2Compression::Gzip,
                "zstd" => S2Compression::Zstd,
                _ => {
                    return Err(Error::InvalidConfig(format!(
                        "S2_COMPRESSION must be one of none, gzip, or zstd; received `{compression}`"
                    )));
                }
            });
        }

        Ok(config)
    }

    pub fn sdk_config(&self) -> Result<S2Config> {
        let access_token = self
            .access_token
            .as_deref()
            .ok_or(Error::MissingAccessToken)?;
        let mut config = S2Config::new(access_token)
            .with_user_agent(USER_AGENT)?
            .with_request_timeout(Duration::from_secs(30))
            .with_compression(self.compression.unwrap_or_default().into());

        match (&self.account_endpoint, &self.basin_endpoint) {
            (Some(account), Some(basin)) => {
                let account = AccountEndpoint::new(account)
                    .map_err(|error| Error::InvalidConfig(error.to_string()))?;
                let basin = BasinEndpoint::new(basin)
                    .map_err(|error| Error::InvalidConfig(error.to_string()))?;
                let endpoints = S2Endpoints::new(account, basin)
                    .map_err(|error| Error::InvalidConfig(error.to_string()))?;
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
                    Error::InvalidConfig(format!("S2_ENCRYPTION_KEY is invalid: {error}"))
                })
            })
            .transpose()
    }
}

fn environment_value(name: &'static str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(Error::InvalidEnvironment { name }),
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
            Error::InvalidConfig(format!(
                "{name} must be either true or false; received `{value}`"
            ))
        })?);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn config_path() -> Result<PathBuf> {
    let mut path = dirs::config_dir().ok_or(Error::ConfigDirectoryNotFound)?;
    path.push("s2");
    path.push("config.toml");
    Ok(path)
}

#[cfg(not(target_os = "windows"))]
fn config_path() -> Result<PathBuf> {
    let mut path = dirs::home_dir().ok_or(Error::ConfigDirectoryNotFound)?;
    path.push(".config");
    path.push("s2");
    path.push("config.toml");
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_labels_only_report_active_custom_endpoints() {
        let account_only = S2Configuration {
            account_endpoint: Some("http://localhost:8080".to_owned()),
            ..S2Configuration::default()
        };
        assert_eq!(account_only.account_endpoint_label(), "S2 Cloud default");
        assert_eq!(account_only.basin_endpoint_label(), "S2 Cloud default");

        let endpoints = account_only.with_endpoints(
            "http://account.localhost:8080",
            "http://basin.localhost:8080",
        );
        assert_eq!(
            endpoints.account_endpoint_label(),
            "http://account.localhost:8080"
        );
        assert_eq!(
            endpoints.basin_endpoint_label(),
            "http://basin.localhost:8080"
        );
    }

    #[test]
    fn empty_encryption_key_is_rejected() {
        let configuration = S2Configuration::default().with_encryption_key("  ");
        let result = configuration.encryption_key();
        assert!(result.is_err(), "empty encryption key was accepted");
        if let Err(error) = result {
            assert!(error.to_string().contains("S2_ENCRYPTION_KEY is invalid"));
        }
    }
}
