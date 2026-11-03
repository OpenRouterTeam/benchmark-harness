use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, anyhow};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex, mpsc, oneshot},
};

use crate::launch::{INHERITED_ENV, LaunchSpec};

/// How long a single request may wait for its response.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(300);

type Pending = Arc<std::sync::Mutex<HashMap<String, oneshot::Sender<Value>>>>;
/// Where server→client requests go while a streaming call is open (else they get -32601).
type Outbox = Arc<std::sync::Mutex<Option<mpsc::UnboundedSender<Value>>>>;

/// A running stdio MCP server, multiplexing concurrent requests by JSON-RPC id.
pub struct StdioConn {
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Pending,
    outbox: Outbox,
    child: Mutex<Child>,
    next_id: AtomicU64,
}

impl StdioConn {
    pub async fn spawn(spec: &LaunchSpec) -> anyhow::Result<Arc<Self>> {
        let mut command = Command::new(&spec.command);
        command
            .args(&spec.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        for key in INHERITED_ENV {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command.envs(&spec.env);
        let mut child = command
            .spawn()
            .with_context(|| format!("starting {}", spec.command))?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().context("child stdin")?));
        let stdout = child.stdout.take().context("child stdout")?;
        let pending: Pending = Arc::default();
        let outbox: Outbox = Arc::default();

        let reader_pending = pending.clone();
        let reader_outbox = outbox.clone();
        let reader_stdin = stdin.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    continue; // servers that log to stdout
                };
                route(&msg, &reader_pending, &reader_outbox, &reader_stdin).await;
            }
            // Child exited: dropping the senders fails every waiting call.
            reader_pending.lock().expect("pending lock").clear();
        });

        Ok(Arc::new(Self {
            stdin,
            pending,
            outbox,
            child: Mutex::new(child),
            next_id: AtomicU64::new(1),
        }))
    }

    /// Sends a request that already carries an `id` and returns the full response message.
    pub async fn call(&self, msg: Value) -> anyhow::Result<Value> {
        let key = msg.get("id").context("request without id")?.to_string();
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending lock")
            .insert(key.clone(), tx);
        if let Err(e) = self.write(&msg).await {
            self.pending.lock().expect("pending lock").remove(&key);
            return Err(e);
        }
        match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(anyhow!("MCP server exited before answering")),
            Err(_) => {
                self.pending.lock().expect("pending lock").remove(&key);
                Err(anyhow!("MCP server did not answer within {CALL_TIMEOUT:?}"))
            }
        }
    }

    /// Like [`StdioConn::call`], but server→client requests the server sends meanwhile
    /// (elicitation, sampling, roots) are yielded first, then the final response. The caller
    /// relays them to its client and returns the client's answers via [`StdioConn::notify`].
    pub fn call_streaming(self: &Arc<Self>, msg: Value) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.outbox.lock().expect("outbox lock") = Some(tx.clone());
        let conn = self.clone();
        tokio::spawn(async move {
            let id = msg.get("id").cloned().unwrap_or(Value::Null);
            let response = conn.call(msg).await.unwrap_or_else(|e| {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": e.to_string()}})
            });
            let mut outbox = conn.outbox.lock().expect("outbox lock");
            if outbox
                .as_ref()
                .is_some_and(|current| current.same_channel(&tx))
            {
                *outbox = None;
            }
            drop(outbox);
            let _ = tx.send(response);
        });
        rx
    }

    /// Sends a request with a fresh id and returns its `result`, or its `error` as `Err`.
    pub async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let response = self
            .call(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        into_result(response)
    }

    /// Sends a notification or a response (anything not awaiting a reply).
    pub async fn notify(&self, msg: Value) -> anyhow::Result<()> {
        self.write(&msg).await
    }

    pub async fn shutdown(&self) {
        let _ = self.child.lock().await.kill().await;
    }

    async fn write(&self, msg: &Value) -> anyhow::Result<()> {
        write_line(&self.stdin, msg).await
    }
}

/// `result` of a JSON-RPC response, or its error message as `Err`.
pub fn into_result(response: Value) -> anyhow::Result<Value> {
    if let Some(error) = response.get("error") {
        return Err(anyhow!(
            "MCP error {}: {}",
            error["code"],
            error["message"].as_str().unwrap_or_default()
        ));
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

async fn route(msg: &Value, pending: &Pending, outbox: &Outbox, stdin: &Arc<Mutex<ChildStdin>>) {
    let has_method = msg.get("method").is_some();
    match (msg.get("id"), has_method) {
        (Some(id), false) => {
            if let Some(tx) = pending
                .lock()
                .expect("pending lock")
                .remove(&id.to_string())
            {
                let _ = tx.send(msg.clone());
            }
        }
        (Some(id), true) => {
            // Server→client request: relay it to the client if a streaming call is open.
            let relayed = outbox
                .lock()
                .expect("outbox lock")
                .as_ref()
                .is_some_and(|tx| tx.send(msg.clone()).is_ok());
            if relayed {
                return;
            }
            let reply = json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}});
            let _ = write_line(stdin, &reply).await;
        }
        (None, _) => {} // notifications from the server are not forwarded
    }
}

async fn write_line(stdin: &Arc<Mutex<ChildStdin>>, msg: &Value) -> anyhow::Result<()> {
    let mut line = serde_json::to_vec(msg)?;
    line.push(b'\n');
    let mut stdin = stdin.lock().await;
    stdin.write_all(&line).await?;
    stdin.flush().await?;
    Ok(())
}
