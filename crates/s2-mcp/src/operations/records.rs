use std::{
    io::{self, Write},
    ops::RangeTo,
    str,
    time::Duration,
};

use base64ct::{Base64, Encoding};
use s2_sdk::types::{
    AppendInput, AppendRecord, AppendRecordBatch, FencingToken, Header, MeteredBytes, ReadBatch,
    ReadFrom, ReadInput, ReadLimits, ReadStart, ReadStop, SequencedRecord, StreamPosition,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::timeout;

use super::{Operations, bounded, parse, serialize, stream::PositionOutput};
use crate::error::{Error, Result};

const MAX_READ_RECORDS: usize = 100;
const MAX_READ_BYTES: usize = 256 * 1024;
const MIN_READ_OUTPUT_BYTES: usize = 1024;
const DEFAULT_READ_OUTPUT_BYTES: usize = 512 * 1024;
const MAX_READ_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_WAIT_SECONDS: u32 = 25;
const WAIT_RESPONSE_GRACE_SECONDS: u64 = 1;
const MAX_APPEND_RECORDS: usize = 100;
const MAX_APPEND_METERED_BYTES: usize = 1024 * 1024;
const MAX_APPEND_REQUEST_BYTES: usize = 2 * 1024 * 1024;

impl Operations {
    pub(super) async fn read_records(&self, arguments: Value) -> Result<Value> {
        let request: ReadRecordsRequest = parse(arguments)?;
        let stream = self.stream(&request.basin, &request.stream)?;
        let bounds = read_bounds(request.limit, request.max_bytes, request.max_output_bytes)?;
        let fallback_next_seq_num = request.start.fallback_next_seq_num();
        let input = read_input(
            request.start,
            request.clamp_to_tail,
            bounds,
            0,
            request.until_timestamp,
        );

        let batch = stream.read(input).await?;
        let prepared =
            PreparedReadOutput::new(batch, fallback_next_seq_num, request.ignore_command_records);
        serialize(prepared.into_read_records(bounds.output_bytes)?)
    }

    pub(super) async fn wait_for_records(&self, arguments: Value) -> Result<Value> {
        let request: WaitForRecordsRequest = parse(arguments)?;
        let stream = self.stream(&request.basin, &request.stream)?;
        let bounds = read_bounds(request.limit, request.max_bytes, request.max_output_bytes)?;
        let timeout_seconds = bounded_wait_timeout(request.timeout_seconds)?;
        let fallback_next_seq_num = request.start.fallback_next_seq_num();
        let input = read_input(
            request.start,
            request.clamp_to_tail,
            bounds,
            timeout_seconds,
            None,
        );
        let deadline = Duration::from_secs(
            u64::from(timeout_seconds).saturating_add(WAIT_RESPONSE_GRACE_SECONDS),
        );

        let batch = match timeout(deadline, stream.read(input)).await {
            Ok(batch) => Some(batch?),
            Err(_) => None,
        };
        let (prepared, client_timed_out) = if let Some(batch) = batch {
            (
                PreparedReadOutput::new(
                    batch,
                    fallback_next_seq_num,
                    request.ignore_command_records,
                ),
                false,
            )
        } else {
            (PreparedReadOutput::empty(fallback_next_seq_num), true)
        };
        serialize(prepared.into_wait_for_records(bounds.output_bytes, client_timed_out)?)
    }

    pub(super) async fn append_records(&self, arguments: Value) -> Result<Value> {
        validate_append_request_size(&arguments)?;
        let request: AppendRecordsRequest = parse(arguments)?;
        let stream = self.stream(&request.basin, &request.stream)?;
        validate_append_batch(&request.records)?;

        let records = request
            .records
            .into_iter()
            .map(AppendRecordInput::try_into_sdk)
            .collect::<Result<Vec<_>>>()?;
        let batch = AppendRecordBatch::try_from_iter(records)?;
        if batch.metered_bytes() > MAX_APPEND_METERED_BYTES {
            return Err(Error::InvalidArguments(format!(
                "records cannot exceed {MAX_APPEND_METERED_BYTES} decoded metered bytes"
            )));
        }

        let mut input = AppendInput::new(batch);
        if let Some(match_seq_num) = request.match_seq_num {
            input = input.with_match_seq_num(match_seq_num);
        }
        if let Some(fencing_token) = request.fencing_token {
            let fencing_token: FencingToken = fencing_token.parse()?;
            input = input.with_fencing_token(fencing_token);
        }

        let acknowledgement = stream.append(input).await?;
        serialize(AppendRecordsOutput {
            start: acknowledgement.start.into(),
            end: acknowledgement.end.into(),
            tail: acknowledgement.tail.into(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct ReadBounds {
    count: usize,
    bytes: usize,
    output_bytes: usize,
}

fn read_bounds(
    count: Option<usize>,
    bytes: Option<usize>,
    output_bytes: Option<usize>,
) -> Result<ReadBounds> {
    Ok(ReadBounds {
        count: bounded(count.unwrap_or(MAX_READ_RECORDS), MAX_READ_RECORDS, "limit")?,
        bytes: bounded(bytes.unwrap_or(MAX_READ_BYTES), MAX_READ_BYTES, "max_bytes")?,
        output_bytes: bounded_read_output_bytes(output_bytes.unwrap_or(DEFAULT_READ_OUTPUT_BYTES))?,
    })
}

fn bounded_read_output_bytes(value: usize) -> Result<usize> {
    if !(MIN_READ_OUTPUT_BYTES..=MAX_READ_OUTPUT_BYTES).contains(&value) {
        return Err(Error::InvalidArguments(format!(
            "max_output_bytes must be between {MIN_READ_OUTPUT_BYTES} and {MAX_READ_OUTPUT_BYTES}"
        )));
    }
    Ok(value)
}

fn bounded_wait_timeout(value: u32) -> Result<u32> {
    if !(1..=MAX_WAIT_SECONDS).contains(&value) {
        return Err(Error::InvalidArguments(format!(
            "timeout_seconds must be between 1 and {MAX_WAIT_SECONDS}"
        )));
    }
    Ok(value)
}

fn read_input(
    from: ReadPosition,
    clamp_to_tail: bool,
    bounds: ReadBounds,
    wait_seconds: u32,
    until_timestamp: Option<u64>,
) -> ReadInput {
    let start = ReadStart::new()
        .with_from(from.into_sdk())
        .with_clamp_to_tail(clamp_to_tail);
    let limits = ReadLimits::new()
        .with_count(bounds.count)
        .with_bytes(bounds.bytes);
    let mut stop = ReadStop::new().with_limits(limits).with_wait(wait_seconds);
    if let Some(until_timestamp) = until_timestamp {
        stop = stop.with_until(RangeTo {
            end: until_timestamp,
        });
    }
    ReadInput::new()
        .with_start(start)
        .with_stop(stop)
        .with_ignore_command_records(false)
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ReadPosition {
    SeqNum { value: u64 },
    Timestamp { value: u64 },
    TailOffset { value: u64 },
}

impl ReadPosition {
    fn into_sdk(self) -> ReadFrom {
        match self {
            Self::SeqNum { value } => ReadFrom::SeqNum(value),
            Self::Timestamp { value } => ReadFrom::Timestamp(value),
            Self::TailOffset { value } => ReadFrom::TailOffset(value),
        }
    }

    fn fallback_next_seq_num(self) -> Option<u64> {
        match self {
            Self::SeqNum { value } => Some(value),
            Self::Timestamp { .. } | Self::TailOffset { .. } => None,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadRecordsRequest {
    basin: String,
    stream: String,
    start: ReadPosition,
    #[serde(default)]
    clamp_to_tail: bool,
    limit: Option<usize>,
    max_bytes: Option<usize>,
    max_output_bytes: Option<usize>,
    until_timestamp: Option<u64>,
    #[serde(default)]
    ignore_command_records: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitForRecordsRequest {
    basin: String,
    stream: String,
    start: ReadPosition,
    timeout_seconds: u32,
    #[serde(default)]
    clamp_to_tail: bool,
    limit: Option<usize>,
    max_bytes: Option<usize>,
    max_output_bytes: Option<usize>,
    #[serde(default)]
    ignore_command_records: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ReadRecordsOutput {
    records: Vec<RecordOutput>,
    next_seq_num: Option<u64>,
    tail: Option<PositionOutput>,
    truncated: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct WaitForRecordsOutput {
    records: Vec<RecordOutput>,
    next_seq_num: Option<u64>,
    tail: Option<PositionOutput>,
    truncated: bool,
    caught_up: bool,
    timed_out: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
struct RecordOutput {
    seq_num: u64,
    timestamp: u64,
    headers: Vec<HeaderOutput>,
    body: EncodedBytes,
}

impl From<&SequencedRecord> for RecordOutput {
    fn from(value: &SequencedRecord) -> Self {
        Self {
            seq_num: value.seq_num,
            timestamp: value.timestamp,
            headers: value
                .headers
                .iter()
                .map(|header| HeaderOutput {
                    name: EncodedBytes::encode(&header.name),
                    value: EncodedBytes::encode(&header.value),
                })
                .collect(),
            body: EncodedBytes::encode(&value.body),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
struct HeaderOutput {
    name: EncodedBytes,
    value: EncodedBytes,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "encoding", content = "data", rename_all = "snake_case")]
enum EncodedBytes {
    Utf8(String),
    Base64(String),
}

impl EncodedBytes {
    fn encode(bytes: &[u8]) -> Self {
        match str::from_utf8(bytes) {
            Ok(value) => Self::Utf8(value.to_owned()),
            Err(_) => Self::Base64(Base64::encode_string(bytes)),
        }
    }

    fn decoded_size(&self) -> Result<usize> {
        match self {
            Self::Utf8(value) => Ok(value.len()),
            Self::Base64(value) => base64_decoded_size(value),
        }
    }

    fn decode(self) -> Result<Vec<u8>> {
        match self {
            Self::Utf8(value) => Ok(value.into_bytes()),
            Self::Base64(value) => Base64::decode_vec(&value).map_err(|_| invalid_base64()),
        }
    }
}

fn base64_decoded_size(value: &str) -> Result<usize> {
    if !value.len().is_multiple_of(4) {
        return Err(invalid_base64());
    }
    let padding = value.bytes().rev().take_while(|byte| *byte == b'=').count();
    if padding > 2 || value[..value.len() - padding].contains('=') {
        return Err(invalid_base64());
    }
    value
        .len()
        .checked_div(4)
        .and_then(|blocks| blocks.checked_mul(3))
        .and_then(|size| size.checked_sub(padding))
        .ok_or_else(invalid_base64)
}

fn invalid_base64() -> Error {
    Error::InvalidArguments("record data contains invalid base64".to_owned())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppendRecordsRequest {
    basin: String,
    stream: String,
    records: Vec<AppendRecordInput>,
    match_seq_num: Option<u64>,
    fencing_token: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AppendRecordInput {
    body: EncodedBytes,
    #[serde(default)]
    headers: Vec<HeaderInput>,
    timestamp: Option<u64>,
}

impl AppendRecordInput {
    fn metered_bytes(&self) -> Result<usize> {
        let header_overhead = self
            .headers
            .len()
            .checked_mul(2)
            .ok_or_else(append_size_overflow)?;
        let headers = self.headers.iter().try_fold(
            8usize
                .checked_add(header_overhead)
                .ok_or_else(append_size_overflow)?,
            |size, header| {
                size.checked_add(header.decoded_size()?)
                    .ok_or_else(append_size_overflow)
            },
        )?;
        headers
            .checked_add(self.body.decoded_size()?)
            .ok_or_else(append_size_overflow)
    }

    fn try_into_sdk(self) -> Result<AppendRecord> {
        let headers = self
            .headers
            .into_iter()
            .map(HeaderInput::try_into_sdk)
            .collect::<Result<Vec<_>>>()?;
        let mut record = AppendRecord::new(self.body.decode()?)?.with_headers(headers)?;
        if let Some(timestamp) = self.timestamp {
            record = record.with_timestamp(timestamp);
        }
        Ok(record)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HeaderInput {
    name: EncodedBytes,
    value: EncodedBytes,
}

impl HeaderInput {
    fn decoded_size(&self) -> Result<usize> {
        self.name
            .decoded_size()?
            .checked_add(self.value.decoded_size()?)
            .ok_or_else(append_size_overflow)
    }

    fn try_into_sdk(self) -> Result<Header> {
        Ok(Header::new(self.name.decode()?, self.value.decode()?))
    }
}

fn append_size_overflow() -> Error {
    Error::InvalidArguments("record batch size overflowed".to_owned())
}

fn validate_append_batch(records: &[AppendRecordInput]) -> Result<()> {
    bounded(records.len(), MAX_APPEND_RECORDS, "records")?;
    let metered_bytes = records.iter().try_fold(0usize, |size, record| {
        size.checked_add(record.metered_bytes()?)
            .ok_or_else(append_size_overflow)
    })?;
    if metered_bytes > MAX_APPEND_METERED_BYTES {
        return Err(Error::InvalidArguments(format!(
            "records cannot exceed {MAX_APPEND_METERED_BYTES} decoded metered bytes"
        )));
    }
    Ok(())
}

fn validate_append_request_size(arguments: &Value) -> Result<()> {
    let size = serialized_size(arguments)?;
    if size > MAX_APPEND_REQUEST_BYTES {
        return Err(Error::InvalidArguments(format!(
            "append request cannot exceed {MAX_APPEND_REQUEST_BYTES} encoded bytes"
        )));
    }
    Ok(())
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct AppendRecordsOutput {
    start: PositionOutput,
    end: PositionOutput,
    tail: PositionOutput,
}

struct PreparedReadOutput {
    records: Vec<RecordOutput>,
    next_seq_num: Option<u64>,
    tail: Option<PositionOutput>,
    caught_up: bool,
    had_records: bool,
}

impl PreparedReadOutput {
    fn new(
        batch: ReadBatch,
        fallback_next_seq_num: Option<u64>,
        ignore_command_records: bool,
    ) -> Self {
        let caught_up = batch_caught_up(&batch.records, batch.tail);
        let had_records = !batch.records.is_empty();
        let next_seq_num = next_sequence_num(
            &batch.records,
            fallback_next_seq_num,
            batch.tail.map(|tail| tail.seq_num),
        );
        let records = batch
            .records
            .iter()
            .filter(|record| !ignore_command_records || !record.is_command_record())
            .map(RecordOutput::from)
            .collect();
        Self {
            records,
            next_seq_num,
            tail: batch.tail.map(PositionOutput::from),
            caught_up,
            had_records,
        }
    }

    fn empty(next_seq_num: Option<u64>) -> Self {
        Self {
            records: Vec::new(),
            next_seq_num,
            tail: None,
            caught_up: false,
            had_records: false,
        }
    }

    fn into_read_records(self, maximum: usize) -> Result<ReadRecordsOutput> {
        let Self {
            mut records,
            next_seq_num,
            tail,
            ..
        } = self;
        let prefix = bounded_record_prefix(
            &records,
            next_seq_num,
            maximum,
            |candidate_next_seq_num, truncated| {
                serialized_size(&ReadRecordsOutput {
                    records: Vec::new(),
                    next_seq_num: candidate_next_seq_num,
                    tail,
                    truncated,
                })
            },
        )?;
        records.truncate(prefix.count);
        Ok(ReadRecordsOutput {
            records,
            next_seq_num: prefix.next_seq_num,
            tail,
            truncated: prefix.truncated,
        })
    }

    fn into_wait_for_records(
        self,
        maximum: usize,
        client_timed_out: bool,
    ) -> Result<WaitForRecordsOutput> {
        let Self {
            mut records,
            next_seq_num,
            tail,
            caught_up,
            had_records,
        } = self;
        let timed_out = client_timed_out || !had_records;
        let prefix = bounded_record_prefix(
            &records,
            next_seq_num,
            maximum,
            |candidate_next_seq_num, truncated| {
                serialized_size(&WaitForRecordsOutput {
                    records: Vec::new(),
                    next_seq_num: candidate_next_seq_num,
                    tail,
                    truncated,
                    caught_up: caught_up && !truncated,
                    timed_out,
                })
            },
        )?;
        records.truncate(prefix.count);
        Ok(WaitForRecordsOutput {
            records,
            next_seq_num: prefix.next_seq_num,
            tail,
            truncated: prefix.truncated,
            caught_up: caught_up && !prefix.truncated,
            timed_out,
        })
    }
}

fn batch_caught_up(records: &[SequencedRecord], tail: Option<StreamPosition>) -> bool {
    tail.is_some_and(|tail| {
        records.last().is_none_or(|record| {
            record
                .seq_num
                .checked_add(1)
                .is_some_and(|next_seq_num| next_seq_num == tail.seq_num)
        })
    })
}

fn next_sequence_num(
    records: &[SequencedRecord],
    fallback: Option<u64>,
    tail_seq_num: Option<u64>,
) -> Option<u64> {
    if let Some(record) = records.last() {
        record.seq_num.checked_add(1).or(tail_seq_num)
    } else {
        fallback.or(tail_seq_num)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecordPrefix {
    count: usize,
    next_seq_num: Option<u64>,
    truncated: bool,
}

fn bounded_record_prefix(
    records: &[RecordOutput],
    full_next_seq_num: Option<u64>,
    maximum: usize,
    mut empty_envelope_size: impl FnMut(Option<u64>, bool) -> Result<usize>,
) -> Result<RecordPrefix> {
    let mut records_contribution = 0usize;
    let mut best = None;
    for count in 0..=records.len() {
        if count > 0 {
            records_contribution = records_contribution
                .saturating_add(serialized_size(&records[count - 1])?)
                .saturating_add(if count > 1 { 1 } else { 0 });
        }
        let truncated = count < records.len();
        let next_seq_num = if truncated {
            Some(records[count].seq_num)
        } else {
            full_next_seq_num
        };
        let output_size =
            empty_envelope_size(next_seq_num, truncated)?.saturating_add(records_contribution);
        if output_size <= maximum {
            best = Some(RecordPrefix {
                count,
                next_seq_num,
                truncated,
            });
        }
    }
    best.ok_or_else(|| {
        Error::InvalidArguments(
            "max_output_bytes is too small to encode response metadata".to_owned(),
        )
    })
}

fn serialized_size<T>(value: &T) -> Result<usize>
where
    T: Serialize + ?Sized,
{
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.bytes)
}

#[derive(Default)]
struct ByteCounter {
    bytes: usize,
}

impl Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len());
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
