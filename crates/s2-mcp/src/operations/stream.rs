use s2_sdk::{
    S2Stream,
    types::{
        AppendInput, AppendRecord, AppendRecordBatch, CommandRecord, FencingToken, StreamPosition,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Operations, parse, serialize};
use crate::error::Result;

impl Operations {
    pub(crate) async fn check_tail(&self, arguments: Value) -> Result<Value> {
        let request: StreamRequest = parse(arguments)?;
        let stream = self.stream(&request.basin, &request.stream)?;
        serialize(PositionOutput::from(stream.check_tail().await?))
    }

    pub(crate) async fn fence_stream(&self, arguments: Value) -> Result<Value> {
        let request: FenceStreamRequest = parse(arguments)?;
        let stream = self.stream(&request.basin, &request.stream)?;
        let fencing_token: FencingToken = request.fencing_token.parse()?;
        let command = CommandRecord::fence(fencing_token);
        append_command(
            stream,
            command,
            request.match_seq_num,
            request.current_fencing_token,
        )
        .await
    }

    pub(crate) async fn trim_stream(&self, arguments: Value) -> Result<Value> {
        let request: TrimStreamRequest = parse(arguments)?;
        let stream = self.stream(&request.basin, &request.stream)?;
        let command = CommandRecord::trim(request.trim_point);
        append_command(
            stream,
            command,
            request.match_seq_num,
            request.fencing_token,
        )
        .await
    }

    pub(super) fn stream(&self, basin: &str, stream: &str) -> Result<S2Stream> {
        self.policy.enforce_basin(basin)?;
        let stream = self.s2.basin(basin.parse()?).stream(stream.parse()?);
        Ok(match &self.encryption_key {
            Some(encryption_key) => stream.with_encryption_key(encryption_key.clone()),
            None => stream,
        })
    }
}

async fn append_command(
    stream: S2Stream,
    command: CommandRecord,
    match_seq_num: Option<u64>,
    fencing_token: Option<String>,
) -> Result<Value> {
    let records = AppendRecordBatch::try_from_iter([AppendRecord::from(command)])?;
    let mut input = AppendInput::new(records);
    if let Some(match_seq_num) = match_seq_num {
        input = input.with_match_seq_num(match_seq_num);
    }
    if let Some(fencing_token) = fencing_token {
        input = input.with_fencing_token(fencing_token.parse()?);
    }

    let acknowledgement = stream.append(input).await?;
    serialize(StreamCommandOutput {
        start: acknowledgement.start.into(),
        end: acknowledgement.end.into(),
        tail: acknowledgement.tail.into(),
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StreamRequest {
    basin: String,
    stream: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FenceStreamRequest {
    basin: String,
    stream: String,
    fencing_token: String,
    match_seq_num: Option<u64>,
    current_fencing_token: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TrimStreamRequest {
    basin: String,
    stream: String,
    trim_point: u64,
    match_seq_num: Option<u64>,
    fencing_token: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
pub(crate) struct PositionOutput {
    seq_num: u64,
    timestamp: u64,
}

impl From<StreamPosition> for PositionOutput {
    fn from(value: StreamPosition) -> Self {
        Self {
            seq_num: value.seq_num,
            timestamp: value.timestamp,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct StreamCommandOutput {
    start: PositionOutput,
    end: PositionOutput,
    tail: PositionOutput,
}
