use std::time::Duration;

use s2_sdk::types::{
    BasinConfig as SdkBasinConfig, BasinReconfiguration as SdkBasinReconfiguration,
    DeleteOnEmptyConfig as SdkDeleteOnEmptyConfig,
    DeleteOnEmptyReconfiguration as SdkDeleteOnEmptyReconfiguration, DeleteStreamInput,
    EncryptionAlgorithm as SdkEncryptionAlgorithm, EnsureOutput, EnsureStreamInput,
    ReconfigureStreamInput, RetentionPolicy as SdkRetentionPolicy, StorageClass as SdkStorageClass,
    StreamConfig as SdkStreamConfig, StreamInfo as SdkStreamInfo,
    StreamReconfiguration as SdkStreamReconfiguration, TimestampingConfig as SdkTimestampingConfig,
    TimestampingMode as SdkTimestampingMode,
    TimestampingReconfiguration as SdkTimestampingReconfiguration,
};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::{Operations, bounded, date_time, parse, serialize};
use crate::{
    error::{Error, Result},
    policy::{Access, Scope},
};

const MAX_LIST_LIMIT: usize = 100;
const MAX_DIFF_RESOURCES: usize = 8;
const MAX_DIFF_ITEMS: usize = 64;
const DEFAULT_RETENTION_AGE_SECS: u64 = 7 * 24 * 60 * 60;

impl Operations {
    pub(super) async fn list_streams(&self, arguments: Value) -> Result<Value> {
        let request: ListStreamsRequest = parse(arguments)?;
        self.policy.enforce_operation(Access::Read, Scope::Basin)?;
        self.policy.enforce_basin(&request.basin)?;
        let limit = bounded(
            request.limit.unwrap_or(MAX_LIST_LIMIT),
            MAX_LIST_LIMIT,
            "limit",
        )?;
        let basin = self.s2.basin(request.basin.parse()?);
        let mut input = s2_sdk::types::ListStreamsInput::new().with_limit(limit);
        if let Some(prefix) = request.prefix {
            input = input.with_prefix(prefix.parse()?);
        }
        if let Some(start_after) = request.start_after {
            input = input.with_start_after(start_after.parse()?);
        }

        let page = basin.list_streams(input).await?;
        let next_cursor = page
            .has_more
            .then(|| page.values.last().map(|stream| stream.name.to_string()))
            .flatten();
        serialize(ListStreamsOutput {
            streams: page
                .values
                .into_iter()
                .map(StreamInfoOutput::from)
                .collect(),
            next_cursor,
        })
    }

    pub(crate) async fn get_stream_config(&self, arguments: Value) -> Result<Value> {
        let request: GetStreamConfigRequest = parse(arguments)?;
        self.policy.enforce_operation(Access::Read, Scope::Stream)?;
        self.policy.enforce_basin(&request.basin)?;
        let basin = self.s2.basin(request.basin.parse()?);
        let config = basin.get_stream_config(request.stream.parse()?).await?;
        serialize(GetStreamConfigOutput {
            basin: request.basin,
            stream: request.stream,
            config: config.into(),
        })
    }

    pub(crate) async fn ensure_stream(&self, arguments: Value) -> Result<Value> {
        let request: EnsureStreamRequest = parse(arguments)?;
        self.policy
            .enforce_operation(Access::Write, Scope::Stream)?;
        self.policy.enforce_basin(&request.basin)?;
        let basin = self.s2.basin(request.basin.parse()?);
        let mut input = EnsureStreamInput::new(request.stream.parse()?);
        if let Some(config) = request.config {
            input = input.with_config(config.try_into()?);
        }

        let (outcome, stream) = match basin.ensure_stream(input).await? {
            EnsureOutput::Created(stream) => (EnsureOutcomeOutput::Created, stream),
            EnsureOutput::ConfigUpdated(stream) => (EnsureOutcomeOutput::ConfigUpdated, stream),
            EnsureOutput::ConfigUnchanged(stream) => (EnsureOutcomeOutput::ConfigUnchanged, stream),
        };
        serialize(EnsureStreamOutput {
            outcome,
            stream: stream.into(),
        })
    }

    pub(crate) async fn reconfigure_stream(&self, arguments: Value) -> Result<Value> {
        let request: ReconfigureStreamRequest = parse(arguments)?;
        self.policy
            .enforce_operation(Access::Write, Scope::Stream)?;
        self.policy.enforce_basin(&request.basin)?;
        if request.config.is_empty() {
            return Err(Error::InvalidArguments(
                "config must specify at least one field".to_owned(),
            ));
        }
        let basin = self.s2.basin(request.basin.parse()?);
        let input =
            ReconfigureStreamInput::new(request.stream.parse()?, request.config.try_into_sdk()?);
        let config = basin.reconfigure_stream(input).await?;
        serialize(GetStreamConfigOutput {
            basin: request.basin,
            stream: request.stream,
            config: config.into(),
        })
    }

    pub(crate) async fn delete_stream(&self, arguments: Value) -> Result<Value> {
        let request: DeleteStreamRequest = parse(arguments)?;
        self.policy
            .enforce_operation(Access::Destructive, Scope::Stream)?;
        self.policy.enforce_basin(&request.basin)?;
        let basin = self.s2.basin(request.basin.parse()?);
        let input = DeleteStreamInput::new(request.stream.parse()?)
            .with_ignore_not_found(request.ignore_not_found);
        basin.delete_stream(input).await?;
        serialize(DeleteResourceOutput { accepted: true })
    }

    pub(crate) async fn diff_resources(&self, arguments: Value) -> Result<Value> {
        let request: DiffResourcesRequest = parse(arguments)?;
        self.policy.enforce_operation(
            Access::Read,
            Scope::Dynamic {
                applicable_under_basin: true,
            },
        )?;
        bounded(
            request.resources.len(),
            MAX_DIFF_RESOURCES,
            "resources length",
        )?;
        let item_limit = bounded(
            request.max_items.unwrap_or(MAX_DIFF_ITEMS),
            MAX_DIFF_ITEMS,
            "max_items",
        )?;

        let mut remaining_items = item_limit;
        let mut difference_count = 0;
        let mut resources = Vec::with_capacity(request.resources.len());
        for resource in request.resources {
            let (identity, differences) = match resource {
                DesiredResource::Basin { basin, desired } => {
                    self.policy.enforce_operation(Access::Read, Scope::Basin)?;
                    self.policy.enforce_basin(&basin)?;
                    desired.validate()?;
                    let actual = self.s2.get_basin_config(basin.parse()?).await?;
                    let differences = basin_config_differences(&actual.into(), &desired)?;
                    (ResourceIdentityOutput::Basin { basin }, differences)
                }
                DesiredResource::Stream {
                    basin,
                    stream,
                    desired,
                } => {
                    self.policy.enforce_operation(Access::Read, Scope::Stream)?;
                    self.policy.enforce_basin(&basin)?;
                    desired.validate()?;
                    let actual = self
                        .s2
                        .basin(basin.parse()?)
                        .get_stream_config(stream.parse()?)
                        .await?;
                    let differences = stream_config_differences(&actual.into(), &desired, "")?;
                    (
                        ResourceIdentityOutput::Stream { basin, stream },
                        differences,
                    )
                }
            };

            let resource_difference_count = differences.len();
            difference_count += resource_difference_count;
            let returned_count = resource_difference_count.min(remaining_items);
            remaining_items -= returned_count;
            resources.push(ResourceDiffOutput {
                resource: identity,
                matches: resource_difference_count == 0,
                difference_count: resource_difference_count,
                differences: differences.into_iter().take(returned_count).collect(),
                truncated: returned_count < resource_difference_count,
            });
        }

        let returned_items = item_limit - remaining_items;
        serialize(DiffResourcesOutput {
            matches: difference_count == 0,
            difference_count,
            returned_items,
            truncated: returned_items < difference_count,
            resources,
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListStreamsRequest {
    basin: String,
    prefix: Option<String>,
    start_after: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ListStreamsOutput {
    streams: Vec<StreamInfoOutput>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct StreamInfoOutput {
    name: String,
    created_at: String,
    deleted_at: Option<String>,
    encrypted: bool,
}

impl From<SdkStreamInfo> for StreamInfoOutput {
    fn from(stream: SdkStreamInfo) -> Self {
        Self {
            name: stream.name.to_string(),
            created_at: date_time(stream.created_at),
            deleted_at: stream.deleted_at.map(date_time),
            encrypted: stream.cipher.is_some(),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetStreamConfigRequest {
    basin: String,
    stream: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct GetStreamConfigOutput {
    basin: String,
    stream: String,
    config: StreamConfigDto,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnsureStreamRequest {
    basin: String,
    stream: String,
    config: Option<StreamConfigDto>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct EnsureStreamOutput {
    outcome: EnsureOutcomeOutput,
    stream: EnsuredStreamInfoOutput,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EnsureOutcomeOutput {
    Created,
    ConfigUpdated,
    ConfigUnchanged,
}

#[derive(Debug, Serialize, JsonSchema)]
struct EnsuredStreamInfoOutput {
    name: String,
    created_at: String,
    deleted_at: Option<String>,
    cipher: Option<EncryptionAlgorithmDto>,
}

impl From<SdkStreamInfo> for EnsuredStreamInfoOutput {
    fn from(stream: SdkStreamInfo) -> Self {
        Self {
            name: stream.name.to_string(),
            created_at: date_time(stream.created_at),
            deleted_at: stream.deleted_at.map(date_time),
            cipher: stream.cipher.map(Into::into),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReconfigureStreamRequest {
    basin: String,
    stream: String,
    config: StreamReconfigurationDto,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteStreamRequest {
    basin: String,
    stream: String,
    #[serde(default = "default_true")]
    ignore_not_found: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct DeleteResourceOutput {
    pub(super) accepted: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StreamConfigDto {
    storage_class: Option<StorageClassDto>,
    retention_policy: Option<RetentionPolicyDto>,
    timestamping: Option<TimestampingConfigDto>,
    delete_on_empty: Option<DeleteOnEmptyConfigDto>,
}

impl StreamConfigDto {
    fn validate(&self) -> Result<()> {
        if matches!(
            self.retention_policy,
            Some(RetentionPolicyDto::Age { seconds: 0 })
        ) {
            return Err(Error::InvalidArguments(
                "retention_policy.age seconds must be greater than zero".to_owned(),
            ));
        }
        Ok(())
    }
}

impl TryFrom<StreamConfigDto> for SdkStreamConfig {
    type Error = Error;

    fn try_from(config: StreamConfigDto) -> Result<Self> {
        config.validate()?;
        let mut sdk = SdkStreamConfig::new();
        if let Some(storage_class) = config.storage_class {
            sdk = sdk.with_storage_class(storage_class.into());
        }
        if let Some(retention_policy) = config.retention_policy {
            sdk = sdk.with_retention_policy(retention_policy.try_into_sdk()?);
        }
        if let Some(timestamping) = config.timestamping {
            sdk = sdk.with_timestamping(timestamping.into_sdk());
        }
        if let Some(delete_on_empty) = config.delete_on_empty {
            sdk = sdk.with_delete_on_empty(delete_on_empty.into_sdk());
        }
        Ok(sdk)
    }
}

impl From<SdkStreamConfig> for StreamConfigDto {
    fn from(config: SdkStreamConfig) -> Self {
        Self {
            storage_class: config.storage_class.map(Into::into),
            retention_policy: config.retention_policy.map(Into::into),
            timestamping: config.timestamping.map(Into::into),
            delete_on_empty: config.delete_on_empty.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BasinConfigDto {
    default_stream_config: Option<StreamConfigDto>,
    stream_cipher: Option<EncryptionAlgorithmDto>,
    #[serde(default)]
    create_stream_on_append: bool,
    #[serde(default)]
    create_stream_on_read: bool,
}

impl BasinConfigDto {
    pub(super) fn validate(&self) -> Result<()> {
        if let Some(config) = &self.default_stream_config {
            config.validate()?;
        }
        Ok(())
    }
}

impl TryFrom<BasinConfigDto> for SdkBasinConfig {
    type Error = Error;

    fn try_from(config: BasinConfigDto) -> Result<Self> {
        config.validate()?;
        let mut sdk = SdkBasinConfig::new()
            .with_create_stream_on_append(config.create_stream_on_append)
            .with_create_stream_on_read(config.create_stream_on_read);
        if let Some(default_stream_config) = config.default_stream_config {
            sdk = sdk.with_default_stream_config(default_stream_config.try_into()?);
        }
        if let Some(stream_cipher) = config.stream_cipher {
            sdk = sdk.with_stream_cipher(stream_cipher.into());
        }
        Ok(sdk)
    }
}

impl From<SdkBasinConfig> for BasinConfigDto {
    fn from(config: SdkBasinConfig) -> Self {
        Self {
            default_stream_config: config.default_stream_config.map(Into::into),
            stream_cipher: config.stream_cipher.map(Into::into),
            create_stream_on_append: config.create_stream_on_append,
            create_stream_on_read: config.create_stream_on_read,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StorageClassDto {
    Standard,
    Express,
}

impl From<StorageClassDto> for SdkStorageClass {
    fn from(storage_class: StorageClassDto) -> Self {
        match storage_class {
            StorageClassDto::Standard => Self::Standard,
            StorageClassDto::Express => Self::Express,
        }
    }
}

impl From<SdkStorageClass> for StorageClassDto {
    fn from(storage_class: SdkStorageClass) -> Self {
        match storage_class {
            SdkStorageClass::Standard => Self::Standard,
            SdkStorageClass::Express => Self::Express,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RetentionPolicyDto {
    Age { seconds: u64 },
    Infinite,
}

impl RetentionPolicyDto {
    fn try_into_sdk(self) -> Result<SdkRetentionPolicy> {
        match self {
            Self::Age { seconds: 0 } => Err(Error::InvalidArguments(
                "retention_policy.age seconds must be greater than zero".to_owned(),
            )),
            Self::Age { seconds } => Ok(SdkRetentionPolicy::Age(seconds)),
            Self::Infinite => Ok(SdkRetentionPolicy::Infinite),
        }
    }
}

impl From<SdkRetentionPolicy> for RetentionPolicyDto {
    fn from(retention_policy: SdkRetentionPolicy) -> Self {
        match retention_policy {
            SdkRetentionPolicy::Age(seconds) => Self::Age { seconds },
            SdkRetentionPolicy::Infinite => Self::Infinite,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TimestampingModeDto {
    ClientPrefer,
    ClientRequire,
    Arrival,
}

impl From<TimestampingModeDto> for SdkTimestampingMode {
    fn from(mode: TimestampingModeDto) -> Self {
        match mode {
            TimestampingModeDto::ClientPrefer => Self::ClientPrefer,
            TimestampingModeDto::ClientRequire => Self::ClientRequire,
            TimestampingModeDto::Arrival => Self::Arrival,
        }
    }
}

impl From<SdkTimestampingMode> for TimestampingModeDto {
    fn from(mode: SdkTimestampingMode) -> Self {
        match mode {
            SdkTimestampingMode::ClientPrefer => Self::ClientPrefer,
            SdkTimestampingMode::ClientRequire => Self::ClientRequire,
            SdkTimestampingMode::Arrival => Self::Arrival,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TimestampingConfigDto {
    mode: Option<TimestampingModeDto>,
    uncapped: Option<bool>,
}

impl TimestampingConfigDto {
    fn into_sdk(self) -> SdkTimestampingConfig {
        let mut sdk = SdkTimestampingConfig::new();
        if let Some(mode) = self.mode {
            sdk = sdk.with_mode(mode.into());
        }
        if let Some(uncapped) = self.uncapped {
            sdk = sdk.with_uncapped(uncapped);
        }
        sdk
    }
}

impl From<SdkTimestampingConfig> for TimestampingConfigDto {
    fn from(config: SdkTimestampingConfig) -> Self {
        Self {
            mode: config.mode.map(Into::into),
            uncapped: config.uncapped,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteOnEmptyConfigDto {
    #[serde(default)]
    min_age_seconds: u64,
}

impl DeleteOnEmptyConfigDto {
    fn into_sdk(self) -> SdkDeleteOnEmptyConfig {
        SdkDeleteOnEmptyConfig::new().with_min_age(Duration::from_secs(self.min_age_seconds))
    }
}

impl From<SdkDeleteOnEmptyConfig> for DeleteOnEmptyConfigDto {
    fn from(config: SdkDeleteOnEmptyConfig) -> Self {
        Self {
            min_age_seconds: config.min_age_secs,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EncryptionAlgorithmDto {
    Aegis256,
    Aes256Gcm,
}

impl From<EncryptionAlgorithmDto> for SdkEncryptionAlgorithm {
    fn from(algorithm: EncryptionAlgorithmDto) -> Self {
        match algorithm {
            EncryptionAlgorithmDto::Aegis256 => Self::Aegis256,
            EncryptionAlgorithmDto::Aes256Gcm => Self::Aes256Gcm,
        }
    }
}

impl From<SdkEncryptionAlgorithm> for EncryptionAlgorithmDto {
    fn from(algorithm: SdkEncryptionAlgorithm) -> Self {
        match algorithm {
            SdkEncryptionAlgorithm::Aegis256 => Self::Aegis256,
            SdkEncryptionAlgorithm::Aes256Gcm => Self::Aes256Gcm,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum NullableUpdate<T> {
    #[default]
    Omitted,
    Clear,
    Value(T),
}

impl<'de, T> Deserialize<'de> for NullableUpdate<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Clear,
        })
    }
}

impl<T> NullableUpdate<T> {
    fn is_omitted(&self) -> bool {
        matches!(self, Self::Omitted)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum ValueUpdate<T> {
    #[default]
    Omitted,
    Value(T),
}

impl<'de, T> Deserialize<'de> for ValueUpdate<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Self::Value)
    }
}

impl<T> ValueUpdate<T> {
    fn is_omitted(&self) -> bool {
        matches!(self, Self::Omitted)
    }
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TimestampingReconfigurationDto {
    #[serde(default)]
    #[schemars(with = "Option<TimestampingModeDto>")]
    mode: NullableUpdate<TimestampingModeDto>,
    #[serde(default)]
    #[schemars(with = "Option<bool>")]
    uncapped: NullableUpdate<bool>,
}

impl TimestampingReconfigurationDto {
    fn is_empty(&self) -> bool {
        self.mode.is_omitted() && self.uncapped.is_omitted()
    }

    fn into_sdk(self) -> SdkTimestampingReconfiguration {
        let mut sdk = SdkTimestampingReconfiguration::new();
        match self.mode {
            NullableUpdate::Omitted => {}
            NullableUpdate::Clear => sdk.mode = None.into(),
            NullableUpdate::Value(mode) => sdk.mode = Some(mode.into()).into(),
        }
        match self.uncapped {
            NullableUpdate::Omitted => {}
            NullableUpdate::Clear => sdk.uncapped = None.into(),
            NullableUpdate::Value(uncapped) => sdk.uncapped = Some(uncapped).into(),
        }
        sdk
    }
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteOnEmptyReconfigurationDto {
    #[serde(default)]
    #[schemars(with = "Option<u64>")]
    min_age_seconds: NullableUpdate<u64>,
}

impl DeleteOnEmptyReconfigurationDto {
    fn is_empty(&self) -> bool {
        self.min_age_seconds.is_omitted()
    }

    fn into_sdk(self) -> SdkDeleteOnEmptyReconfiguration {
        let mut sdk = SdkDeleteOnEmptyReconfiguration::new();
        match self.min_age_seconds {
            NullableUpdate::Omitted => {}
            NullableUpdate::Clear => sdk.min_age_secs = None.into(),
            NullableUpdate::Value(seconds) => sdk.min_age_secs = Some(seconds).into(),
        }
        sdk
    }
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StreamReconfigurationDto {
    #[serde(default)]
    #[schemars(with = "Option<StorageClassDto>")]
    storage_class: NullableUpdate<StorageClassDto>,
    #[serde(default)]
    #[schemars(with = "Option<RetentionPolicyDto>")]
    retention_policy: NullableUpdate<RetentionPolicyDto>,
    #[serde(default)]
    #[schemars(with = "Option<TimestampingReconfigurationDto>")]
    timestamping: NullableUpdate<TimestampingReconfigurationDto>,
    #[serde(default)]
    #[schemars(with = "Option<DeleteOnEmptyReconfigurationDto>")]
    delete_on_empty: NullableUpdate<DeleteOnEmptyReconfigurationDto>,
}

impl StreamReconfigurationDto {
    fn is_empty(&self) -> bool {
        self.storage_class.is_omitted()
            && self.retention_policy.is_omitted()
            && self.timestamping.is_omitted()
            && self.delete_on_empty.is_omitted()
    }

    fn try_into_sdk(self) -> Result<SdkStreamReconfiguration> {
        let mut sdk = SdkStreamReconfiguration::new();
        match self.storage_class {
            NullableUpdate::Omitted => {}
            NullableUpdate::Clear => sdk.storage_class = None.into(),
            NullableUpdate::Value(storage_class) => {
                sdk.storage_class = Some(storage_class.into()).into();
            }
        }
        match self.retention_policy {
            NullableUpdate::Omitted => {}
            NullableUpdate::Clear => sdk.retention_policy = None.into(),
            NullableUpdate::Value(retention_policy) => {
                sdk.retention_policy = Some(retention_policy.try_into_sdk()?).into();
            }
        }
        match self.timestamping {
            NullableUpdate::Omitted => {}
            NullableUpdate::Clear => sdk.timestamping = None.into(),
            NullableUpdate::Value(timestamping) => {
                if timestamping.is_empty() {
                    return Err(Error::InvalidArguments(
                        "timestamping must specify at least one field".to_owned(),
                    ));
                }
                sdk.timestamping = Some(timestamping.into_sdk()).into();
            }
        }
        match self.delete_on_empty {
            NullableUpdate::Omitted => {}
            NullableUpdate::Clear => sdk.delete_on_empty = None.into(),
            NullableUpdate::Value(delete_on_empty) => {
                if delete_on_empty.is_empty() {
                    return Err(Error::InvalidArguments(
                        "delete_on_empty must specify min_age_seconds".to_owned(),
                    ));
                }
                sdk.delete_on_empty = Some(delete_on_empty.into_sdk()).into();
            }
        }
        Ok(sdk)
    }
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BasinReconfigurationDto {
    #[serde(default)]
    #[schemars(with = "Option<StreamReconfigurationDto>")]
    default_stream_config: NullableUpdate<StreamReconfigurationDto>,
    #[serde(default)]
    #[schemars(with = "Option<EncryptionAlgorithmDto>")]
    stream_cipher: NullableUpdate<EncryptionAlgorithmDto>,
    #[serde(default)]
    #[schemars(with = "bool")]
    create_stream_on_append: ValueUpdate<bool>,
    #[serde(default)]
    #[schemars(with = "bool")]
    create_stream_on_read: ValueUpdate<bool>,
}

impl BasinReconfigurationDto {
    pub(super) fn is_empty(&self) -> bool {
        self.default_stream_config.is_omitted()
            && self.stream_cipher.is_omitted()
            && self.create_stream_on_append.is_omitted()
            && self.create_stream_on_read.is_omitted()
    }

    pub(super) fn try_into_sdk(self) -> Result<SdkBasinReconfiguration> {
        let mut sdk = SdkBasinReconfiguration::new();
        match self.default_stream_config {
            NullableUpdate::Omitted => {}
            NullableUpdate::Clear => sdk.default_stream_config = None.into(),
            NullableUpdate::Value(default_stream_config) => {
                if default_stream_config.is_empty() {
                    return Err(Error::InvalidArguments(
                        "default_stream_config must specify at least one field".to_owned(),
                    ));
                }
                sdk.default_stream_config = Some(default_stream_config.try_into_sdk()?).into();
            }
        }
        match self.stream_cipher {
            NullableUpdate::Omitted => {}
            NullableUpdate::Clear => sdk.stream_cipher = None.into(),
            NullableUpdate::Value(stream_cipher) => {
                sdk.stream_cipher = Some(stream_cipher.into()).into();
            }
        }
        match self.create_stream_on_append {
            ValueUpdate::Omitted => {}
            ValueUpdate::Value(value) => sdk.create_stream_on_append = value.into(),
        }
        match self.create_stream_on_read {
            ValueUpdate::Omitted => {}
            ValueUpdate::Value(value) => sdk.create_stream_on_read = value.into(),
        }
        Ok(sdk)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiffResourcesRequest {
    resources: Vec<DesiredResource>,
    max_items: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DesiredResource {
    Basin {
        basin: String,
        desired: BasinConfigDto,
    },
    Stream {
        basin: String,
        stream: String,
        desired: StreamConfigDto,
    },
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct DiffResourcesOutput {
    matches: bool,
    difference_count: usize,
    returned_items: usize,
    truncated: bool,
    resources: Vec<ResourceDiffOutput>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ResourceDiffOutput {
    resource: ResourceIdentityOutput,
    matches: bool,
    difference_count: usize,
    differences: Vec<DiffItemOutput>,
    truncated: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ResourceIdentityOutput {
    Basin { basin: String },
    Stream { basin: String, stream: String },
}

#[derive(Debug, Serialize, JsonSchema)]
struct DiffItemOutput {
    path: String,
    actual: Value,
    desired: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EffectiveStreamConfig {
    storage_class: StorageClassDto,
    retention_policy: RetentionPolicyDto,
    timestamping_mode: TimestampingModeDto,
    timestamping_uncapped: bool,
    delete_on_empty_min_age_seconds: u64,
}

impl From<&StreamConfigDto> for EffectiveStreamConfig {
    fn from(config: &StreamConfigDto) -> Self {
        Self {
            storage_class: config.storage_class.unwrap_or(StorageClassDto::Express),
            retention_policy: config.retention_policy.unwrap_or(RetentionPolicyDto::Age {
                seconds: DEFAULT_RETENTION_AGE_SECS,
            }),
            timestamping_mode: config
                .timestamping
                .as_ref()
                .and_then(|timestamping| timestamping.mode)
                .unwrap_or(TimestampingModeDto::ClientPrefer),
            timestamping_uncapped: config
                .timestamping
                .as_ref()
                .and_then(|timestamping| timestamping.uncapped)
                .unwrap_or(false),
            delete_on_empty_min_age_seconds: config
                .delete_on_empty
                .as_ref()
                .map(|delete_on_empty| delete_on_empty.min_age_seconds)
                .unwrap_or(0),
        }
    }
}

fn stream_config_differences(
    actual: &StreamConfigDto,
    desired: &StreamConfigDto,
    path_prefix: &str,
) -> Result<Vec<DiffItemOutput>> {
    let actual = EffectiveStreamConfig::from(actual);
    let desired = EffectiveStreamConfig::from(desired);
    let mut differences = Vec::new();
    push_difference(
        &mut differences,
        &format!("{path_prefix}storage_class"),
        &actual.storage_class,
        &desired.storage_class,
    )?;
    push_difference(
        &mut differences,
        &format!("{path_prefix}retention_policy"),
        &actual.retention_policy,
        &desired.retention_policy,
    )?;
    push_difference(
        &mut differences,
        &format!("{path_prefix}timestamping.mode"),
        &actual.timestamping_mode,
        &desired.timestamping_mode,
    )?;
    push_difference(
        &mut differences,
        &format!("{path_prefix}timestamping.uncapped"),
        &actual.timestamping_uncapped,
        &desired.timestamping_uncapped,
    )?;
    push_difference(
        &mut differences,
        &format!("{path_prefix}delete_on_empty.min_age_seconds"),
        &actual.delete_on_empty_min_age_seconds,
        &desired.delete_on_empty_min_age_seconds,
    )?;
    Ok(differences)
}

fn basin_config_differences(
    actual: &BasinConfigDto,
    desired: &BasinConfigDto,
) -> Result<Vec<DiffItemOutput>> {
    let default_stream = StreamConfigDto::default();
    let mut differences = stream_config_differences(
        actual
            .default_stream_config
            .as_ref()
            .unwrap_or(&default_stream),
        desired
            .default_stream_config
            .as_ref()
            .unwrap_or(&default_stream),
        "default_stream_config.",
    )?;
    push_difference(
        &mut differences,
        "stream_cipher",
        &actual.stream_cipher,
        &desired.stream_cipher,
    )?;
    push_difference(
        &mut differences,
        "create_stream_on_append",
        &actual.create_stream_on_append,
        &desired.create_stream_on_append,
    )?;
    push_difference(
        &mut differences,
        "create_stream_on_read",
        &actual.create_stream_on_read,
        &desired.create_stream_on_read,
    )?;
    Ok(differences)
}

fn push_difference<T>(
    differences: &mut Vec<DiffItemOutput>,
    path: &str,
    actual: &T,
    desired: &T,
) -> Result<()>
where
    T: PartialEq + Serialize,
{
    if actual != desired {
        differences.push(DiffItemOutput {
            path: path.to_owned(),
            actual: serde_json::to_value(actual)?,
            desired: serde_json::to_value(desired)?,
        });
    }
    Ok(())
}

const fn default_true() -> bool {
    true
}
