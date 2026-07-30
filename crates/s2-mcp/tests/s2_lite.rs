use std::{
    env,
    error::Error,
    ffi::OsString,
    io::{BufRead, BufReader, BufWriter, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use s2_sdk::{
    S2,
    types::{BasinName, CreateBasinInput, CreateStreamInput, S2Config, S2Endpoints, StreamName},
};
use serde_json::{Value, json};

const BASIN: &str = "mcp-lite-test";
const STREAM: &str = "roundtrip";
type TestResult<T = ()> = Result<T, Box<dyn Error>>;

#[tokio::test]
#[ignore = "requires S2 Lite"]
async fn s2_lite_round_trip_covers_tools_code_mode_and_policy() -> TestResult {
    let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port();
    let endpoint = format!("http://127.0.0.1:{port}");
    let mut lite = ChildGuard(
        Command::new(env::var_os("S2_LITE_BIN").unwrap_or_else(|| OsString::from("s2")))
            .args(["lite", "--port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    wait_for_lite(&mut lite, port)?;
    provision(&endpoint).await?;

    let mut tools = Session::start(&endpoint, &["--mode", "tools"])?;
    let append = tools.call(
        "append_records",
        json!({
            "basin": BASIN,
            "stream": STREAM,
            "records": [{ "body": { "encoding": "utf8", "data": "hello" } }],
        }),
    )?;
    assert_eq!(append["end"]["seq_num"], json!(1));
    let read = tools.call(
        "read_records",
        json!({ "basin": BASIN, "stream": STREAM, "start": { "type": "seq_num", "value": 0 } }),
    )?;
    assert_eq!(read["records"][0]["body"]["data"], json!("hello"));

    let mut readonly = Session::start(&endpoint, &["--mode", "tools", "--readonly"])?;
    assert!(
        !readonly
            .tool_names()?
            .iter()
            .any(|name| name == "append_records")
    );

    let mut code = Session::start(&endpoint, &["--mode", "code", "--readonly"])?;
    let output = code.call(
        "execute",
        json!({
            "code": format!(
                "async function run() {{ return await S2.listStreams({{ basin: {BASIN:?} }}); }}"
            ),
        }),
    )?;
    assert_eq!(output["success"], json!(true));
    assert_eq!(output["output"]["streams"][0]["name"], json!(STREAM));
    Ok(())
}

async fn provision(endpoint: &str) -> TestResult {
    let s2 =
        S2::new(S2Config::new("ignored").with_endpoints(S2Endpoints::for_endpoint(endpoint)?))?;
    let basin: BasinName = BASIN.parse()?;
    let stream: StreamName = STREAM.parse()?;
    s2.create_basin(CreateBasinInput::new(basin.clone()))
        .await?;
    s2.basin(basin)
        .create_stream(CreateStreamInput::new(stream))
        .await?;
    Ok(())
}

fn wait_for_lite(lite: &mut ChildGuard, port: u16) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = lite.0.try_wait()? {
            return Err(format!("S2 Lite exited with {status}").into());
        }
        if TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("S2 Lite did not start".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

struct Session {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Session {
    fn start(endpoint: &str, arguments: &[&str]) -> TestResult<Self> {
        let mut child = Command::new(assert_cmd::cargo::cargo_bin!("s2-mcp"))
            .args(arguments)
            .env("S2_ACCESS_TOKEN", "ignored")
            .env("S2_ACCOUNT_ENDPOINT", endpoint)
            .env("S2_BASIN_ENDPOINT", endpoint)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().ok_or("server stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("server stdout unavailable")?;
        let mut session = Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
        };
        session.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" },
            }),
        )?;
        session.notify("notifications/initialized", json!({}))?;
        Ok(session)
    }

    fn tool_names(&mut self) -> TestResult<Vec<String>> {
        let result = self.request("tools/list", json!({}))?;
        result["tools"]
            .as_array()
            .ok_or("missing tools")?
            .iter()
            .map(|tool| -> TestResult<String> {
                tool["name"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "missing tool name".into())
            })
            .collect()
    }

    fn call(&mut self, name: &str, arguments: Value) -> TestResult<Value> {
        let result = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )?;
        result["structuredContent"]
            .as_object()
            .map(|value| Value::Object(value.clone()))
            .ok_or_else(|| format!("tool failed: {result}").into())
    }

    fn request(&mut self, method: &str, params: Value) -> TestResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(
            self.stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
        )?;
        self.stdin.flush()?;
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line)? == 0 {
                return Err("server closed stdout".into());
            }
            let response: Value = serde_json::from_str(&line)?;
            if response["id"] == json!(id) {
                return response
                    .get("result")
                    .cloned()
                    .ok_or_else(|| format!("request failed: {response}").into());
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> TestResult {
        writeln!(
            self.stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "method": method, "params": params })
        )?;
        self.stdin.flush()?;
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
