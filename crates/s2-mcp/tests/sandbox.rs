use std::{
    collections::VecDeque,
    error::Error,
    io::{BufRead, BufReader, BufWriter, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
type TestResult<T = ()> = Result<T, Box<dyn Error>>;

#[test]
fn sandbox_denies_host_authority_and_imports() -> TestResult {
    let mut client = Client::start()?;
    let output = client.execute(
        r#"
async function run() {
  const deniedGlobals = [
    "Deno", "process", "fetch", "WebSocket", "XMLHttpRequest",
    "Worker", "crypto", "ArrayBuffer", "SharedArrayBuffer",
    "WebAssembly", "__bootstrap", "__infra",
  ].filter((name) => name in globalThis);

  let importDenied = false;
  try {
    await Function("return import('file:///etc/hosts')")();
  } catch {
    importDenied = true;
  }

  let unknownCallbackDenied = false;
  try {
    await invokeInternal({ name: "S2__not_real", arguments: {} });
  } catch {
    unknownCallbackDenied = true;
  }
  return { deniedGlobals, importDenied, unknownCallbackDenied };
}
"#,
    )?;

    assert_eq!(
        output["output"],
        json!({
            "deniedGlobals": [],
            "importDenied": true,
            "unknownCallbackDenied": true,
        })
    );
    Ok(())
}

#[test]
fn cancellation_kills_the_executor_and_server_recovers() -> TestResult {
    let mut client = Client::start()?;
    let request_id = client.send_execute("async function run() { while (true) {} }")?;
    thread::sleep(Duration::from_millis(500));
    client.notify(
        "notifications/cancelled",
        json!({ "requestId": request_id, "reason": "test" }),
    )?;

    let output = client.execute("async function run() { return 'recovered'; }")?;
    assert_eq!(output["output"], json!("recovered"));
    Ok(())
}

enum ReaderEvent {
    Message(Value),
    Closed,
    Failed(String),
}

struct Client {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    receiver: Receiver<ReaderEvent>,
    pending: VecDeque<Value>,
    next_id: u64,
}

impl Client {
    fn start() -> TestResult<Self> {
        let mut child = Command::new(assert_cmd::cargo::cargo_bin!("s2-mcp"))
            .args(["--mode", "code", "--readonly"])
            .env("S2_ACCESS_TOKEN", "test-token")
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().ok_or("server stdin was unavailable")?;
        let stdout = child.stdout.take().ok_or("server stdout was unavailable")?;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        let _ = sender.send(ReaderEvent::Closed);
                        break;
                    }
                    Ok(_) => match serde_json::from_str(&line) {
                        Ok(message) => {
                            if sender.send(ReaderEvent::Message(message)).is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            let _ = sender.send(ReaderEvent::Failed(error.to_string()));
                            break;
                        }
                    },
                    Err(error) => {
                        let _ = sender.send(ReaderEvent::Failed(error.to_string()));
                        break;
                    }
                }
            }
        });

        let mut client = Self {
            child,
            stdin: BufWriter::new(stdin),
            receiver,
            pending: VecDeque::new(),
            next_id: 1,
        };
        client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" },
            }),
        )?;
        client.notify("notifications/initialized", json!({}))?;
        Ok(client)
    }

    fn execute(&mut self, code: &str) -> TestResult<Value> {
        let id = self.send_execute(code)?;
        let response = self.response(id)?;
        response
            .pointer("/result/structuredContent")
            .cloned()
            .ok_or_else(|| format!("execute failed: {response}").into())
    }

    fn send_execute(&mut self, code: &str) -> TestResult<u64> {
        self.send_request(
            "tools/call",
            json!({ "name": "execute", "arguments": { "code": code } }),
        )
    }

    fn request(&mut self, method: &str, params: Value) -> TestResult<Value> {
        let id = self.send_request(method, params)?;
        let response = self.response(id)?;
        response
            .get("result")
            .cloned()
            .ok_or_else(|| format!("request failed: {response}").into())
    }

    fn send_request(&mut self, method: &str, params: Value) -> TestResult<u64> {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(
            self.stdin,
            "{}",
            json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
        )?;
        self.stdin.flush()?;
        Ok(id)
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

    fn response(&mut self, id: u64) -> TestResult<Value> {
        if let Some(index) = self
            .pending
            .iter()
            .position(|message| message["id"] == json!(id))
        {
            return self
                .pending
                .remove(index)
                .ok_or_else(|| "pending response disappeared".into());
        }
        loop {
            match self.receiver.recv_timeout(RESPONSE_TIMEOUT)? {
                ReaderEvent::Message(message) if message["id"] == json!(id) => return Ok(message),
                ReaderEvent::Message(message) => self.pending.push_back(message),
                ReaderEvent::Closed => return Err("server closed stdout".into()),
                ReaderEvent::Failed(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
