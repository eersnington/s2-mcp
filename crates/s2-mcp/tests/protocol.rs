use std::{
    error::Error,
    io::{BufRead, BufReader, BufWriter, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use serde_json::{Value, json};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

#[test]
fn modes_expose_the_expected_tools() -> TestResult {
    let mut code = Session::start(&["--mode", "code"])?;
    assert_eq!(code.tool_names()?, ["execute", "search"]);
    let search = code.call_tool("search", json!({ "query": "connection" }))?;
    assert_eq!(search["matches"][0]["name"], json!("S2.connectionInfo"));

    let full = Session::start(&["--mode", "tools", "--allow-destructive"])?.tool_names()?;
    assert_eq!(full.len(), 20);
    assert!(full.iter().any(|name| name == "delete_basin"));

    let readonly = Session::start(&["--mode", "tools", "--readonly"])?.tool_names()?;
    assert_eq!(readonly.len(), 10);
    assert!(!readonly.iter().any(|name| name == "append_records"));

    let mut basin_scoped = Session::start(&["--mode", "tools", "--basin", "example"])?;
    let tools = basin_scoped.request("tools/list", json!({}))?;
    let get_metrics_schema = tools["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "get_metrics"))
        .and_then(|tool| tool.get("inputSchema"))
        .ok_or("basin-scoped get_metrics input schema was unavailable")?;
    assert!(!get_metrics_schema.to_string().contains("\"account\""));
    Ok(())
}

#[test]
fn managed_development_defers_container_start_until_code_execution() -> TestResult {
    let mut session = Session::start_with_docker_host(
        &["--dev", "--mode", "code"],
        "unix:///definitely-missing-s2-mcp-testcontainers.sock",
    )?;
    assert_eq!(session.tool_names()?, ["execute", "search"]);

    let search = session.call_tool("search", json!({ "query": "connection" }))?;
    assert_eq!(search["matches"][0]["name"], json!("S2.connectionInfo"));

    let output = session.call_tool(
        "execute",
        json!({ "code": "async function run() { return await S2.connectionInfo({}); }" }),
    )?;
    assert!(
        output["error"]
            .as_str()
            .is_some_and(|message| message.contains("Could not start S2 Lite"))
    );
    Ok(())
}

#[test]
fn managed_development_defers_container_start_until_tools_call() -> TestResult {
    let mut session = Session::start_with_docker_host(
        &["--dev", "--mode", "tools"],
        "unix:///definitely-missing-s2-mcp-testcontainers.sock",
    )?;
    assert_eq!(session.tool_names()?.len(), 16);

    let output = session.call_tool("connection_info", json!({}))?;
    assert!(
        output["error"]
            .as_str()
            .is_some_and(|message| message.contains("Could not start S2 Lite"))
    );
    Ok(())
}

#[test]
fn code_mode_executes_an_isolated_s2_callback() -> TestResult {
    let mut session = Session::start(&["--mode", "code", "--readonly"])?;
    let output = session.call_tool(
        "execute",
        json!({
            "code": r#"
async function run() {
  const connection = await S2.connectionInfo({});
  return {
    environment: connection.environment,
    accountEndpoint: connection.account_endpoint,
    readonly: connection.readonly,
    hostGlobalsPresent: ["Deno", "process", "fetch", "WebAssembly", "ArrayBuffer"]
      .some((name) => name in globalThis),
  };
}
"#,
        }),
    )?;

    assert_eq!(output["success"], json!(true));
    assert_eq!(
        output["output"],
        json!({
            "environment": "cloud",
            "accountEndpoint": "S2 Cloud default",
            "readonly": true,
            "hostGlobalsPresent": false
        })
    );
    assert_eq!(output["tool_calls"][0]["name"], json!("S2.connectionInfo"));
    Ok(())
}

#[test]
fn development_flags_enforce_an_explicit_boundary() -> TestResult {
    let binary = assert_cmd::cargo::cargo_bin!("s2-mcp");
    let endpoint_without_dev = Command::new(binary)
        .args(["--endpoint", "http://127.0.0.1:8080"])
        .stdin(Stdio::piped())
        .output()?;
    assert!(!endpoint_without_dev.status.success());
    assert!(String::from_utf8_lossy(&endpoint_without_dev.stderr).contains("--dev"));

    let conflicting_sources = Command::new(binary)
        .args(["--dev", "--endpoint", "http://127.0.0.1:8080", "--from-env"])
        .stdin(Stdio::piped())
        .output()?;
    assert!(!conflicting_sources.status.success());
    assert!(String::from_utf8_lossy(&conflicting_sources.stderr).contains("cannot be used with"));
    Ok(())
}

#[test]
fn help_describes_connection_modes_in_user_facing_language() -> TestResult {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("s2-mcp"))
        .arg("--help")
        .output()?;
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("MCP server for S2 durable streams"));
    assert!(help.contains("Start a temporary S2 Lite container when needed"));
    assert!(help.contains("Use an existing S2 development server"));
    assert!(help.contains("Read endpoints from environment variables"));
    assert!(help.contains("Running it directly in a terminal shows this help."));
    assert!(!help.contains("Start S2 Lite on first use"));
    Ok(())
}

#[test]
fn from_env_rejects_a_partial_endpoint_pair() -> TestResult {
    let output = Command::new(assert_cmd::cargo::cargo_bin!("s2-mcp"))
        .args(["--dev", "--from-env"])
        .env("S2_ACCOUNT_ENDPOINT", "http://127.0.0.1:8080")
        .env_remove("S2_BASIN_ENDPOINT")
        .stdin(Stdio::piped())
        .output()?;
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("requires S2_ACCOUNT_ENDPOINT and S2_BASIN_ENDPOINT")
    );
    Ok(())
}

#[test]
fn syntax_and_invalid_arguments_prevent_execution() -> TestResult {
    let mut session = Session::start(&["--mode", "code", "--readonly"])?;
    let output = session.call_tool(
        "execute",
        json!({
            "code": "async function run( { return 1; }",
        }),
    )?;

    assert!(
        output["error"]
            .as_str()
            .is_some_and(|message| message.contains("TypeScript could not be parsed"))
    );

    let output = session.call_tool(
        "execute",
        json!({
            "code": "async function run() { return await S2.connectionInfo({ unexpected: true }); }",
        }),
    )?;
    assert_eq!(output["success"], json!(false));
    assert_eq!(output["tool_calls"][0]["outcome"], json!("error"));
    Ok(())
}

#[test]
fn execution_output_is_bounded() -> TestResult {
    let mut session = Session::start(&["--mode", "code", "--readonly"])?;
    let output = session.call_tool(
        "execute",
        json!({
            "code": "async function run() { console.log('x'.repeat(300000)); return 'y'.repeat(300000); }",
        }),
    )?;

    assert_eq!(output["success"], json!(false));
    assert_eq!(output["truncated"], json!(true));
    assert!(serde_json::to_vec(&output)?.len() <= 256 * 1024);
    Ok(())
}

#[test]
#[ignore = "takes 30 seconds to exercise the production deadline"]
fn executor_terminates_infinite_program_at_production_deadline() -> TestResult {
    let mut session = Session::start(&["--mode", "code", "--readonly"])?;
    let output = session.call_tool(
        "execute",
        json!({ "code": "async function run() { while (true) {} }" }),
    )?;

    assert_eq!(
        output["error"],
        json!("code execution timed out after 30 seconds")
    );
    Ok(())
}

struct Session {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Session {
    fn start(arguments: &[&str]) -> TestResult<Self> {
        Self::start_with_optional_docker_host(arguments, None)
    }

    fn start_with_docker_host(arguments: &[&str], docker_host: &str) -> TestResult<Self> {
        Self::start_with_optional_docker_host(arguments, Some(docker_host))
    }

    fn start_with_optional_docker_host(
        arguments: &[&str],
        docker_host: Option<&str>,
    ) -> TestResult<Self> {
        let mut command = Command::new(assert_cmd::cargo::cargo_bin!("s2-mcp"));
        command
            .args(arguments)
            .env("S2_ACCESS_TOKEN", "test-token")
            .env("S2_ACCOUNT_ENDPOINT", "http://127.0.0.1:1")
            .env("S2_BASIN_ENDPOINT", "http://127.0.0.1:2")
            .env_remove("S2_ENCRYPTION_KEY")
            .env_remove("S2_COMPRESSION")
            .env_remove("S2_SSL_NO_VERIFY")
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(docker_host) = docker_host {
            command.env("DOCKER_HOST", docker_host);
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().ok_or("server stdin was unavailable")?;
        let stdout = child.stdout.take().ok_or("server stdout was unavailable")?;
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
            .ok_or("tools/list did not return tools")?
            .iter()
            .map(|tool| -> TestResult<String> {
                tool["name"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "tool had no name".into())
            })
            .collect()
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> TestResult<Value> {
        let result = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )?;
        result["structuredContent"]
            .as_object()
            .map(|content| Value::Object(content.clone()))
            .ok_or_else(|| "tools/call did not return structured content".into())
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
