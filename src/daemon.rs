use crate::{
    browser::Session,
    protocol::Action,
    runtime::{Config, private_dir},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::Path,
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    process::Command,
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
    time::{sleep, timeout},
};

const MAX_REQUEST: usize = 256 * 1024;
const MAX_RESPONSE: usize = 32 * 1024 * 1024;
#[derive(Debug, Deserialize, Serialize)]
pub struct Reply {
    pub ok: bool,
    pub data: Value,
}
impl Reply {
    pub fn result(self) -> Result<Value> {
        if self.ok {
            Ok(self.data)
        } else {
            bail!(
                "{}",
                self.data
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("daemon request failed")
            )
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Request {
    Action(Action),
    Shutdown,
}
struct Slot {
    session: Option<Session>,
    permit: Option<OwnedSemaphorePermit>,
}
struct Manager {
    config: Config,
    slots: Mutex<BTreeMap<String, Arc<Mutex<Slot>>>>,
    capacity: Arc<Semaphore>,
}
impl Manager {
    async fn execute(&self, action: Action) -> Result<Value> {
        action.validate()?;
        if matches!(action, Action::SessionList(_)) {
            let slots = self
                .slots
                .lock()
                .await
                .iter()
                .map(|(name, slot)| (name.clone(), slot.clone()))
                .collect::<Vec<_>>();
            let mut sessions = Vec::new();
            for (name, slot) in slots {
                if let Ok(slot) = slot.try_lock() {
                    if let Some(session) = &slot.session {
                        sessions.push(json!({"session": name, "headed": session.runtime.headed, "containment": session.runtime.containment, "busy": false}));
                    }
                } else {
                    sessions.push(json!({"session": name, "busy": true}));
                }
            }
            return Ok(json!({"sessions": sessions, "capacity": self.config.max_sessions}));
        }
        let name = action.session().context("session_required")?.to_owned();
        let start = matches!(
            action,
            Action::BrowserOpen(_) | Action::OnionSearch(_) | Action::OnionDiscover(_)
        );
        let headed = matches!(&action, Action::BrowserOpen(a) if a.headed);
        let slot = {
            let mut slots = self.slots.lock().await;
            if let Some(slot) = slots.get(&name) {
                Some(slot.clone())
            } else if start {
                if slots.len() >= 128 {
                    bail!(
                        "session_name_limit: restart the daemon before allocating more than 128 distinct names"
                    );
                }
                let slot = Arc::new(Mutex::new(Slot {
                    session: None,
                    permit: None,
                }));
                slots.insert(name.clone(), slot.clone());
                Some(slot)
            } else {
                None
            }
        };
        let Some(slot) = slot else {
            return match action {
                Action::TorStatus(_) => Ok(json!({"session": name, "running": false})),
                Action::SessionClose(_) => {
                    Ok(json!({"session": name, "closed": true, "was_running": false}))
                }
                _ => bail!("session_not_running: call browser_open for this session first"),
            };
        };
        let mut slot = slot.lock().await;
        if matches!(action, Action::SessionClose(_)) {
            let was_running = slot.session.is_some();
            if let Some(session) = slot.session.take() {
                session.runtime.stop().await;
            }
            slot.permit.take();
            return Ok(json!({"session": name, "closed": true, "was_running": was_running}));
        }
        if slot.session.is_none() {
            if !start {
                if matches!(action, Action::TorStatus(_)) {
                    return Ok(json!({"session": name, "running": false}));
                }
                bail!("session_not_running: call browser_open for this session first");
            }
            let permit = self
                .capacity
                .clone()
                .try_acquire_owned()
                .context("session_capacity: close a session or raise TOR_MCP_MAX_SESSIONS")?;
            let session = Session::launch(&self.config, &name, headed).await?;
            slot.session = Some(session);
            slot.permit = Some(permit);
        }
        slot.session
            .as_mut()
            .context("session_not_running")?
            .execute(action)
            .await
    }
    async fn reap(&self) {
        let slots = self
            .slots
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for slot in slots {
            if let Ok(mut slot) = slot.try_lock() {
                let expired = slot.session.as_ref().is_some_and(|s| {
                    !s.runtime.headed
                        && self.config.idle_seconds > 0
                        && s.last_used.elapsed().as_secs() >= self.config.idle_seconds
                });
                if expired {
                    if let Some(session) = slot.session.take() {
                        session.runtime.stop().await;
                    }
                    slot.permit.take();
                }
            }
        }
    }
    async fn close_all(&self) {
        let slots = self
            .slots
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for slot in slots {
            let mut slot = slot.lock().await;
            if let Some(session) = slot.session.take() {
                session.runtime.stop().await;
            }
            slot.permit.take();
        }
    }
}

pub async fn serve(config: Config) -> Result<()> {
    config.prepare()?;
    let socket = config.root.join("daemon.socket");
    if socket.exists() {
        if UnixStream::connect(&socket).await.is_ok() {
            bail!("daemon_already_running");
        }
        fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let _socket_guard = SocketGuard(socket);
    let manager = Arc::new(Manager {
        capacity: Arc::new(Semaphore::new(config.max_sessions)),
        config,
        slots: Mutex::new(BTreeMap::new()),
    });
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let mut jobs = tokio::task::JoinSet::new();
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = terminate.recv() => break,
            _ = shutdown.notified() => break,
            _ = interval.tick() => manager.reap().await,
            Some(_) = jobs.join_next(), if !jobs.is_empty() => {},
            connection = listener.accept() => {
                let (stream, _) = connection?;
                let manager = manager.clone();
                let shutdown = shutdown.clone();
                jobs.spawn(async move {
                    let _ = handle(stream, manager, shutdown).await;
                });
            }
        }
    }
    jobs.abort_all();
    while jobs.join_next().await.is_some() {}
    manager.close_all().await;
    Ok(())
}
struct SocketGuard(std::path::PathBuf);
impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
async fn handle(
    stream: UnixStream,
    manager: Arc<Manager>,
    shutdown: Arc<tokio::sync::Notify>,
) -> Result<()> {
    if stream.peer_cred()?.uid() != unsafe { libc::geteuid() } {
        bail!("unauthorized local peer");
    }
    let (reader, mut writer) = stream.into_split();
    let data = timeout(
        Duration::from_secs(10),
        read_line_limited(reader, MAX_REQUEST),
    )
    .await??;
    let request: Result<Request> = serde_json::from_slice(&data).map_err(Into::into);
    let is_shutdown = matches!(request, Ok(Request::Shutdown));
    let result = match request {
        Ok(Request::Action(action)) => manager.execute(action).await,
        Ok(Request::Shutdown) => Ok(json!({"shutdown": true})),
        Err(error) => Err(error),
    };
    let reply = match result {
        Ok(data) => Reply { ok: true, data },
        Err(error) => Reply {
            ok: false,
            data: json!({"error": format!("{error:#}")}),
        },
    };
    writer.write_all(&serde_json::to_vec(&reply)?).await?;
    writer.write_all(b"\n").await?;
    writer.shutdown().await?;
    if is_shutdown {
        shutdown.notify_one();
    }
    Ok(())
}
async fn read_line_limited<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    limit: usize,
) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut reader = BufReader::new(reader).take((limit + 1) as u64);
    let mut bytes = Vec::new();
    reader.read_until(b'\n', &mut bytes).await?;
    if bytes.len() > limit || !bytes.ends_with(b"\n") {
        bail!("invalid_message: expected one bounded newline-delimited JSON message");
    }
    Ok(bytes)
}

pub async fn request(config: &Config, request: Request) -> Result<Value> {
    config.prepare()?;
    let socket = config.root.join("daemon.socket");
    let mut stream = match UnixStream::connect(&socket).await {
        Ok(stream) => stream,
        Err(_) if matches!(request, Request::Shutdown) => {
            return Ok(json!({"shutdown": true, "was_running": false}));
        }
        Err(_) => {
            start_daemon(config, &socket).await?;
            UnixStream::connect(&socket).await?
        }
    };
    if stream.peer_cred()?.uid() != unsafe { libc::geteuid() } {
        bail!("unauthorized daemon owner");
    }
    let mut bytes = serde_json::to_vec(&request)?;
    if bytes.len() >= MAX_REQUEST {
        bail!("request_too_large");
    }
    bytes.push(b'\n');
    stream.write_all(&bytes).await?;
    let response = timeout(
        Duration::from_secs(config.bootstrap_seconds + 150),
        read_line_limited(stream, MAX_RESPONSE),
    )
    .await
    .context(
        "daemon_timeout: operation outcome may be unknown; inspect session before retrying",
    )??;
    let reply: Reply = serde_json::from_slice(&response)?;
    let result = reply.result()?;
    if matches!(request, Request::Shutdown) {
        // Acknowledgement is not cleanup completion. Do not let a following CLI
        // call reconnect to a daemon that is still shutting down.
        timeout(Duration::from_secs(240), async {
            while socket.exists() {
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .context("daemon_shutdown_timeout: cleanup has not completed")?;
    }
    Ok(result)
}
async fn start_daemon(config: &Config, socket: &Path) -> Result<()> {
    private_dir(&config.root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(config.root.join("startup.lock"))?;
    // A short filesystem lock prevents two CLI processes from launching competing daemons.
    let lock = tokio::task::spawn_blocking(move || {
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(lock)
    })
    .await??;
    if UnixStream::connect(socket).await.is_ok() {
        drop(lock);
        return Ok(());
    }
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(config.root.join("daemon.log"))?;
    let mut child = Command::new(std::env::current_exe()?)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()?;
    timeout(Duration::from_secs(10), async {
        loop {
            if let Some(status) = child.try_wait()? {
                bail!("daemon_start_failed: {status}");
            }
            if UnixStream::connect(socket).await.is_ok() {
                return Ok::<_, anyhow::Error>(());
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("daemon_start_timeout")??;
    // Do not kill a daemon when a short-lived CLI invocation exits.
    drop(lock);
    Ok(())
}
