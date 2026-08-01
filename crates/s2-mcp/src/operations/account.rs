use s2_sdk::types::{
    AccountMetricSet, BasinInfo, BasinMetricSet, DeleteBasinInput, EnsureBasinInput, EnsureOutput,
    GetAccountMetricsInput, GetBasinMetricsInput, GetStreamMetricsInput, ListBasinsInput, Metric,
    MetricUnit, ReconfigureBasinInput, StreamMetricSet, TimeRange, TimeRangeAndInterval,
    TimeseriesInterval,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    Operations,
    basin::{BasinConfigDto, BasinReconfigurationDto, DeleteResourceOutput, EnsureOutcomeOutput},
    bounded, date_time, parse, serialize,
};
use crate::{
    error::{Error, Result},
    policy::{Access, Scope},
};

const MAX_LIST_LIMIT: usize = 100;
const MAX_METRIC_RANGE_SECONDS: u32 = 30 * 24 * 60 * 60;
const MAX_METRIC_POINTS_PER_SERIES: u32 = 1_440;
const MAX_METRICS: usize = 64;
const MAX_VALUES_PER_METRIC: usize = 1_024;
const MAX_METRIC_OUTPUT_ITEMS: usize = 4_096;

impl Operations {
    pub(crate) async fn connection_info(&self, arguments: Value) -> Result<Value> {
        let _: ConnectionInfoInput = parse(arguments)?;
        serialize(ConnectionInfoOutput {
            environment: self.connection.environment_label().to_owned(),
            account_endpoint: self.connection.account_endpoint_label().to_owned(),
            basin_endpoint: self.connection.basin_endpoint_label().to_owned(),
            readonly: self.policy.readonly,
            basin_scope: self.policy.basin.clone(),
            destructive_operations: self.policy.allows_destructive(),
        })
    }

    pub(crate) async fn list_basins(&self, arguments: Value) -> Result<Value> {
        let request: ListBasinsRequest = parse(arguments)?;
        let limit = bounded(
            request.limit.unwrap_or(MAX_LIST_LIMIT),
            MAX_LIST_LIMIT,
            "limit",
        )?;
        let mut input = ListBasinsInput::new().with_limit(limit);
        if let Some(prefix) = request.prefix {
            input = input.with_prefix(prefix.parse()?);
        }
        if let Some(start_after) = request.start_after {
            input = input.with_start_after(start_after.parse()?);
        }

        let page = self.s2.list_basins(input).await?;
        let next_cursor = page
            .has_more
            .then(|| page.values.last().map(|basin| basin.name.to_string()))
            .flatten();
        serialize(ListBasinsOutput {
            basins: page.values.into_iter().map(Into::into).collect(),
            next_cursor,
        })
    }

    pub(crate) async fn get_basin_config(&self, arguments: Value) -> Result<Value> {
        let request: GetBasinConfigRequest = parse(arguments)?;
        self.policy.enforce_basin(&request.basin)?;
        let config = self.s2.get_basin_config(request.basin.parse()?).await?;
        serialize(GetBasinConfigOutput {
            basin: request.basin,
            config: config.into(),
        })
    }

    pub(crate) async fn ensure_basin(&self, arguments: Value) -> Result<Value> {
        let request: EnsureBasinRequest = parse(arguments)?;
        self.policy.enforce_basin(&request.basin)?;
        let mut input = EnsureBasinInput::new(request.basin.parse()?);
        if let Some(config) = request.config {
            input = input.with_config(config.try_into()?);
        }
        if let Some(location) = request.location {
            input = input.with_location(location)?;
        }

        let (outcome, basin) = match self.s2.ensure_basin(input).await? {
            EnsureOutput::Created(basin) => (EnsureOutcomeOutput::Created, basin),
            EnsureOutput::ConfigUpdated(basin) => (EnsureOutcomeOutput::ConfigUpdated, basin),
            EnsureOutput::ConfigUnchanged(basin) => (EnsureOutcomeOutput::ConfigUnchanged, basin),
        };
        serialize(EnsureBasinOutput {
            outcome,
            basin: basin.into(),
        })
    }

    pub(crate) async fn reconfigure_basin(&self, arguments: Value) -> Result<Value> {
        let request: ReconfigureBasinRequest = parse(arguments)?;
        self.policy.enforce_basin(&request.basin)?;
        if request.config.is_empty() {
            return Err(Error::InvalidArguments(
                "config must specify at least one field".to_owned(),
            ));
        }
        let input =
            ReconfigureBasinInput::new(request.basin.parse()?, request.config.try_into_sdk()?);
        let config = self.s2.reconfigure_basin(input).await?;
        serialize(GetBasinConfigOutput {
            basin: request.basin,
            config: config.into(),
        })
    }

    pub(crate) async fn delete_basin(&self, arguments: Value) -> Result<Value> {
        let request: DeleteBasinRequest = parse(arguments)?;
        self.policy.enforce_basin(&request.basin)?;
        let input = DeleteBasinInput::new(request.basin.parse()?)
            .with_ignore_not_found(request.ignore_not_found);
        self.s2.delete_basin(input).await?;
        serialize(DeleteResourceOutput { accepted: true })
    }

    pub(crate) async fn revoke_access_token(&self, arguments: Value) -> Result<Value> {
        let request: RevokeAccessTokenRequest = parse(arguments)?;
        self.s2.revoke_access_token(request.id.parse()?).await?;
        serialize(RevokeAccessTokenOutput { revoked: true })
    }

    pub(crate) async fn get_metrics(&self, arguments: Value) -> Result<Value> {
        let request: GetMetricsRequest = parse(arguments)?;
        let metrics = match request {
            GetMetricsRequest::Account { set, query } => {
                self.policy
                    .enforce_operation(Access::Read, Scope::Account)?;
                let set = account_metric_set(set, &query)?;
                self.s2
                    .get_account_metrics(GetAccountMetricsInput::new(set))
                    .await?
            }
            GetMetricsRequest::Basin { basin, set, query } => {
                self.policy.enforce_operation(Access::Read, Scope::Basin)?;
                self.policy.enforce_basin(&basin)?;
                let set = basin_metric_set(set, &query)?;
                self.s2
                    .get_basin_metrics(GetBasinMetricsInput::new(basin.parse()?, set))
                    .await?
            }
            GetMetricsRequest::Stream {
                basin,
                stream,
                set,
                query,
            } => {
                self.policy.enforce_operation(Access::Read, Scope::Stream)?;
                self.policy.enforce_basin(&basin)?;
                let set = stream_metric_set(set, &query)?;
                self.s2
                    .get_stream_metrics(GetStreamMetricsInput::new(
                        basin.parse()?,
                        stream.parse()?,
                        set,
                    ))
                    .await?
            }
        };
        serialize(MetricsOutput::from_sdk(metrics))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConnectionInfoInput {}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ConnectionInfoOutput {
    environment: String,
    account_endpoint: String,
    basin_endpoint: String,
    readonly: bool,
    basin_scope: Option<String>,
    destructive_operations: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListBasinsRequest {
    prefix: Option<String>,
    start_after: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ListBasinsOutput {
    basins: Vec<BasinInfoOutput>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct BasinInfoOutput {
    name: String,
    location: Option<String>,
    created_at: String,
    deleted_at: Option<String>,
}

impl From<BasinInfo> for BasinInfoOutput {
    fn from(basin: BasinInfo) -> Self {
        Self {
            name: basin.name.to_string(),
            location: basin.location.map(|location| location.to_string()),
            created_at: date_time(basin.created_at),
            deleted_at: basin.deleted_at.map(date_time),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetBasinConfigRequest {
    basin: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct GetBasinConfigOutput {
    basin: String,
    config: BasinConfigDto,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnsureBasinRequest {
    basin: String,
    config: Option<BasinConfigDto>,
    location: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct EnsureBasinOutput {
    outcome: EnsureOutcomeOutput,
    basin: BasinInfoOutput,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReconfigureBasinRequest {
    basin: String,
    config: BasinReconfigurationDto,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteBasinRequest {
    basin: String,
    #[serde(default = "default_true")]
    ignore_not_found: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevokeAccessTokenRequest {
    id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct RevokeAccessTokenOutput {
    revoked: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum GetMetricsRequest {
    Account {
        set: AccountMetricSetDto,
        query: MetricQueryDto,
    },
    Basin {
        basin: String,
        set: BasinMetricSetDto,
        query: MetricQueryDto,
    },
    Stream {
        basin: String,
        stream: String,
        set: StreamMetricSetDto,
        query: MetricQueryDto,
    },
}

#[derive(Debug, JsonSchema)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
#[expect(dead_code, reason = "this type is used only to generate JSON Schema")]
pub(crate) enum BasinScopedGetMetricsSchema {
    Basin {
        basin: String,
        set: BasinMetricSetDto,
        query: MetricQueryDto,
    },
    Stream {
        basin: String,
        stream: String,
        set: StreamMetricSetDto,
        query: MetricQueryDto,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AccountMetricSetDto {
    ActiveBasins,
    AccountOps,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BasinMetricSetDto {
    Storage,
    AppendOps,
    ReadOps,
    ReadThroughput,
    AppendThroughput,
    BasinOps,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StreamMetricSetDto {
    Storage,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MetricQueryDto {
    start: u32,
    end: u32,
    interval: Option<MetricIntervalDto>,
}

impl MetricQueryDto {
    fn time_range_without_interval(&self, sampling_seconds: Option<u32>) -> Result<TimeRange> {
        if self.interval.is_some() {
            return Err(Error::InvalidArguments(
                "interval is not supported for this metric set".to_owned(),
            ));
        }
        self.validate(sampling_seconds)?;
        Ok(TimeRange::new(self.start, self.end))
    }

    fn time_range_and_interval(
        &self,
        default_interval: MetricIntervalDto,
    ) -> Result<TimeRangeAndInterval> {
        let effective_interval = self.interval.unwrap_or(default_interval);
        self.validate(Some(effective_interval.seconds()))?;
        let mut range = TimeRangeAndInterval::new(self.start, self.end);
        if let Some(interval) = self.interval {
            range = range.with_interval(interval.into());
        }
        Ok(range)
    }

    fn validate(&self, sampling_seconds: Option<u32>) -> Result<()> {
        let Some(duration) = self
            .end
            .checked_sub(self.start)
            .filter(|duration| *duration > 0)
        else {
            return Err(Error::InvalidArguments(
                "metric query end must be greater than start".to_owned(),
            ));
        };
        if duration > MAX_METRIC_RANGE_SECONDS {
            return Err(Error::InvalidArguments(format!(
                "metric query range cannot exceed {MAX_METRIC_RANGE_SECONDS} seconds"
            )));
        }
        if let Some(sampling_seconds) = sampling_seconds
            && duration.div_ceil(sampling_seconds) > MAX_METRIC_POINTS_PER_SERIES
        {
            return Err(Error::InvalidArguments(format!(
                "metric query cannot request more than {MAX_METRIC_POINTS_PER_SERIES} points per series"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MetricIntervalDto {
    Minute,
    Hour,
    Day,
}

impl MetricIntervalDto {
    const fn seconds(self) -> u32 {
        match self {
            Self::Minute => 60,
            Self::Hour => 60 * 60,
            Self::Day => 24 * 60 * 60,
        }
    }
}

impl From<MetricIntervalDto> for TimeseriesInterval {
    fn from(interval: MetricIntervalDto) -> Self {
        match interval {
            MetricIntervalDto::Minute => Self::Minute,
            MetricIntervalDto::Hour => Self::Hour,
            MetricIntervalDto::Day => Self::Day,
        }
    }
}

impl From<TimeseriesInterval> for MetricIntervalDto {
    fn from(interval: TimeseriesInterval) -> Self {
        match interval {
            TimeseriesInterval::Minute => Self::Minute,
            TimeseriesInterval::Hour => Self::Hour,
            TimeseriesInterval::Day => Self::Day,
        }
    }
}

fn account_metric_set(
    set: AccountMetricSetDto,
    query: &MetricQueryDto,
) -> Result<AccountMetricSet> {
    match set {
        AccountMetricSetDto::ActiveBasins => Ok(AccountMetricSet::ActiveBasins(
            query.time_range_without_interval(None)?,
        )),
        AccountMetricSetDto::AccountOps => Ok(AccountMetricSet::AccountOps(
            query.time_range_and_interval(MetricIntervalDto::Hour)?,
        )),
    }
}

fn basin_metric_set(set: BasinMetricSetDto, query: &MetricQueryDto) -> Result<BasinMetricSet> {
    match set {
        BasinMetricSetDto::Storage => Ok(BasinMetricSet::Storage(
            query.time_range_without_interval(Some(MetricIntervalDto::Hour.seconds()))?,
        )),
        BasinMetricSetDto::AppendOps => Ok(BasinMetricSet::AppendOps(
            query.time_range_and_interval(MetricIntervalDto::Minute)?,
        )),
        BasinMetricSetDto::ReadOps => Ok(BasinMetricSet::ReadOps(
            query.time_range_and_interval(MetricIntervalDto::Minute)?,
        )),
        BasinMetricSetDto::ReadThroughput => Ok(BasinMetricSet::ReadThroughput(
            query.time_range_and_interval(MetricIntervalDto::Minute)?,
        )),
        BasinMetricSetDto::AppendThroughput => Ok(BasinMetricSet::AppendThroughput(
            query.time_range_and_interval(MetricIntervalDto::Minute)?,
        )),
        BasinMetricSetDto::BasinOps => Ok(BasinMetricSet::BasinOps(
            query.time_range_and_interval(MetricIntervalDto::Hour)?,
        )),
    }
}

fn stream_metric_set(set: StreamMetricSetDto, query: &MetricQueryDto) -> Result<StreamMetricSet> {
    match set {
        StreamMetricSetDto::Storage => Ok(StreamMetricSet::Storage(
            query.time_range_without_interval(Some(MetricIntervalDto::Minute.seconds()))?,
        )),
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct MetricsOutput {
    metrics: Vec<MetricOutput>,
    truncated: bool,
}

impl MetricsOutput {
    fn from_sdk(metrics: Vec<Metric>) -> Self {
        let mut remaining_items = MAX_METRIC_OUTPUT_ITEMS;
        let mut truncated = metrics.len() > MAX_METRICS;
        let metrics = metrics
            .into_iter()
            .take(MAX_METRICS)
            .map(|metric| MetricOutput::from_sdk(metric, &mut remaining_items, &mut truncated))
            .collect();
        Self { metrics, truncated }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
enum MetricOutput {
    Scalar {
        name: String,
        unit: MetricUnitDto,
        value: f64,
    },
    Accumulation {
        name: String,
        unit: MetricUnitDto,
        interval: MetricIntervalDto,
        values: Vec<MetricPointOutput>,
        truncated: bool,
    },
    Gauge {
        name: String,
        unit: MetricUnitDto,
        values: Vec<MetricPointOutput>,
        truncated: bool,
    },
    Label {
        name: String,
        values: Vec<String>,
        truncated: bool,
    },
}

impl MetricOutput {
    fn from_sdk(metric: Metric, remaining_items: &mut usize, output_truncated: &mut bool) -> Self {
        match metric {
            Metric::Scalar(metric) => Self::Scalar {
                name: metric.name,
                unit: metric.unit.into(),
                value: metric.value,
            },
            Metric::Accumulation(metric) => {
                let (values, truncated) = take_bounded(metric.values, remaining_items);
                *output_truncated |= truncated;
                Self::Accumulation {
                    name: metric.name,
                    unit: metric.unit.into(),
                    interval: metric.interval.into(),
                    values: values.into_iter().map(MetricPointOutput::from).collect(),
                    truncated,
                }
            }
            Metric::Gauge(metric) => {
                let (values, truncated) = take_bounded(metric.values, remaining_items);
                *output_truncated |= truncated;
                Self::Gauge {
                    name: metric.name,
                    unit: metric.unit.into(),
                    values: values.into_iter().map(MetricPointOutput::from).collect(),
                    truncated,
                }
            }
            Metric::Label(metric) => {
                let (values, truncated) = take_bounded(metric.values, remaining_items);
                *output_truncated |= truncated;
                Self::Label {
                    name: metric.name,
                    values,
                    truncated,
                }
            }
        }
    }
}

fn take_bounded<T>(values: Vec<T>, remaining_items: &mut usize) -> (Vec<T>, bool) {
    let source_length = values.len();
    let limit = source_length
        .min(MAX_VALUES_PER_METRIC)
        .min(*remaining_items);
    *remaining_items -= limit;
    (
        values.into_iter().take(limit).collect(),
        limit < source_length,
    )
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MetricUnitDto {
    Bytes,
    Operations,
}

impl From<MetricUnit> for MetricUnitDto {
    fn from(unit: MetricUnit) -> Self {
        match unit {
            MetricUnit::Bytes => Self::Bytes,
            MetricUnit::Operations => Self::Operations,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
struct MetricPointOutput {
    timestamp: u32,
    value: f64,
}

impl From<(u32, f64)> for MetricPointOutput {
    fn from((timestamp, value): (u32, f64)) -> Self {
        Self { timestamp, value }
    }
}

const fn default_true() -> bool {
    true
}
