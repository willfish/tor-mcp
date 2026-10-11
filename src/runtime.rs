use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpStream, UnixListener, UnixStream},
    process::{Child, Command},
    time::{sleep, timeout},
};

#[derive(Clone)]
pub struct Config {
    pub root: PathBuf,
    pub state: PathBuf,
    pub browser: PathBuf,
    pub bundle: PathBuf,
    pub geckodriver: PathBuf,
    pub tor: PathBuf,
    pub bwrap: PathBuf,
    pub max_sessions: usize,
    pub idle_seconds: u64,
    pub bootstrap_seconds: u64,
}
impl Config {
    pub fn from_env() -> Result<Self> {
        let home = std::env::var_os("HOME").context("HOME is required")?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(format!("/tmp/onion-browser-{}", unsafe { libc::geteuid() }))
            });
        let root = runtime.join("onion-browser");
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home).join(".local/state"))
            .join("onion-browser");
        Ok(Self {
            root, state,
            browser: std::env::var_os("ONION_BROWSER_BINARY").map(PathBuf::from).unwrap_or(resolve("tor-browser")?),
            bundle: std::env::var_os("ONION_BROWSER_BUNDLE").map(PathBuf::from).context("ONION_BROWSER_BUNDLE must identify the packaged Tor Browser directory; use the Nix flake")?,
            geckodriver: resolve("geckodriver")?, tor: resolve("tor")?, bwrap: resolve("bwrap")?,
            max_sessions: env_number("ONION_BROWSER_MAX_SESSIONS", 4)?.try_into()?,
            idle_seconds: env_number("ONION_BROWSER_IDLE_SECONDS", 600)?,
            bootstrap_seconds: env_number("ONION_BROWSER_BOOTSTRAP_SECONDS", 180)?,
        })
    }
    pub fn prepare(&self) -> Result<()> {
        // The predictable /tmp fallback's parent must not be pre-owned or
        // replaceable by another user, even if its child is ours.
        private_dir(
            self.root
                .parent()
                .context("invalid runtime base directory")?,
        )?;
        private_dir(&self.root)?;
        private_dir(&self.state)?;
        if self.root.join("daemon.socket").as_os_str().len() > 100 {
            bail!("runtime_path_too_long: choose a shorter XDG_RUNTIME_DIR");
        }
        if self.max_sessions == 0 || self.max_sessions > 32 {
            bail!("ONION_BROWSER_MAX_SESSIONS must be between 1 and 32");
        }
        Ok(())
    }
}
fn env_number(name: &str, default: u64) -> Result<u64> {
    std::env::var(name)
        .map(|v| v.parse().with_context(|| format!("invalid {name}")))
        .unwrap_or(Ok(default))
}
fn resolve(name: &str) -> Result<PathBuf> {
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let path = dir.join(name);
        if path.is_file() {
            return Ok(path);
        }
    }
    bail!("runtime_missing: {name} is not on PATH; use the Nix flake")
}
pub fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    let meta = fs::symlink_metadata(path)?;
    use std::os::unix::fs::MetadataExt;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
        bail!(
            "unsafe_runtime_directory: directory must be owned by the current user and must not be a symlink"
        );
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn log_file(path: &Path) -> Result<Stdio> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .into())
}
fn child_command(binary: &Path) -> Command {
    let mut cmd = Command::new(binary);
    cmd.kill_on_drop(true).stdin(Stdio::null());
    // Owned children stop if the daemon is killed without a graceful shutdown.
    unsafe {
        cmd.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() == 1 {
                return Err(std::io::Error::other("parent exited during startup"));
            }
            Ok(())
        });
    }
    cmd
}
fn copy_profile(from: &Path, to: &Path) -> Result<()> {
    private_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_dir() {
            copy_profile(&entry.path(), &dest)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &dest)?;
        } else {
            bail!("unexpected symlink in packaged browser profile");
        }
    }
    Ok(())
}

pub struct Runtime {
    pub dir: PathBuf,
    pub headed: bool,
    pub tor: Child,
    pub sandbox: Child,
    pub webdriver: reqwest::Client,
    pub webdriver_id: String,
    pub containment: Value,
}
impl Runtime {
    pub async fn launch(config: &Config, name: &str, headed: bool) -> Result<Self> {
        let dir = config.root.join(name);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        private_dir(&dir)?;
        // Startup owns this guard until it has a fully initialized runtime.
        let mut guard = LaunchGuard {
            dir: dir.clone(),
            committed: false,
        };
        let profile = dir.join("profile");
        copy_profile(
            &config
                .bundle
                .join("TorBrowser/Data/Browser/profile.default"),
            &profile,
        )?;
        // Packaged Tor Browser expects its default profile to be registered even
        // when geckodriver passes -profile. A missing profiles.ini crashes ESR.
        let browser_home = dir.join("home/.tor project/firefox");
        private_dir(&browser_home)?;
        fs::write(
            browser_home.join("profiles.ini"),
            "[General]\nStartWithLastProfile=1\n[Profile0]\nName=default\nIsRelative=0\nPath=/session/profile\nDefault=1\n",
        )?;
        let tor_state = config.state.join("tor").join(name);
        private_dir(&tor_state)?;
        let torrc = format!(
            "DataDirectory {}\nSocksPort unix:{} IsolateSOCKSAuth\nControlSocket {}\nCookieAuthentication 1\nCookieAuthFile {}\nAvoidDiskWrites 1\nLog notice stdout\n",
            tor_quote(&tor_state)?,
            tor_quote(&dir.join("socks.socket"))?,
            tor_quote(&dir.join("control.socket"))?,
            tor_quote(&dir.join("control.cookie"))?
        );
        fs::write(dir.join("torrc"), torrc)?;
        let mut tor = child_command(&config.tor)
            .arg("-f")
            .arg(dir.join("torrc"))
            .stdout(log_file(&dir.join("tor.log"))?)
            .stderr(log_file(&dir.join("tor.stderr"))?)
            .spawn()
            .context("tor_launch_failed")?;
        eprintln!(
            "[{name}] Waiting for Tor bootstrap (up to {} seconds)",
            config.bootstrap_seconds
        );
        let bootstrap = timeout(Duration::from_secs(config.bootstrap_seconds), async {
            loop {
                if let Some(status) = tor.try_wait()? { bail!("tor_exited: {status}; see private session startup logs"); }
                if let Ok(response) = tor_control(&dir, "GETINFO status/bootstrap-phase").await
                    && response.contains("PROGRESS=100") { return Ok::<_, anyhow::Error>(()); }
                sleep(Duration::from_millis(500)).await;
            }
        }).await.context("tor_bootstrap_timeout: Tor did not become ready; no direct-network fallback is permitted")?;
        bootstrap?;
        eprintln!(
            "[{name}] Tor ready; starting contained {} browser",
            if headed { "headed" } else { "headless" }
        );
        let mut sandbox = child_command(&config.bwrap);
        sandbox.args(sandbox_args(config, &dir, headed)?);
        sandbox
            .stdout(log_file(&dir.join("driver.log"))?)
            .stderr(log_file(&dir.join("driver.stderr"))?);
        let mut sandbox = sandbox
            .spawn()
            .context("containment_unavailable: bubblewrap could not start")?;
        let webdriver = reqwest::Client::builder()
            .no_proxy()
            .unix_socket(dir.join("webdriver.socket"))
            .timeout(Duration::from_secs(90))
            .build()?;
        timeout(Duration::from_secs(30), async {
            loop {
                if let Some(status) = sandbox.try_wait()? {
                    let diagnostics = fs::read_to_string(dir.join("driver.stderr")).unwrap_or_default();
                    bail!("containment_unavailable: sandbox exited {status}; network namespaces are required; {}", diagnostics.chars().take(2000).collect::<String>());
                }
                if webdriver.get("http://127.0.0.1:4444/status").send().await.is_ok() { return Ok::<_, anyhow::Error>(()); }
                sleep(Duration::from_millis(100)).await;
            }
        }).await.context("webdriver_start_timeout")??;
        let mut args = vec!["-profile", "/session/profile", "-no-remote"];
        if !headed {
            args.push("-headless");
        }
        let capabilities = json!({"capabilities": {"alwaysMatch": {
            "browserName": "firefox", "acceptInsecureCerts": false,
            "moz:firefoxOptions": {"binary": config.browser, "args": args, "prefs": {
                "remote.prefs.recommended": false,
                "network.proxy.type": 1,
                "network.proxy.socks": "file:///session/socks.socket",
                "network.proxy.socks_port": 0,
                "network.proxy.socks_version": 5,
                "network.proxy.socks_remote_dns": true,
                "network.proxy.no_proxies_on": "",
                "torbrowser.settings.quickstart.enabled": true,
                "browser.shell.checkDefaultBrowser": false,
                "browser.startup.homepage_override.mstone": "ignore",
                "intl.language_notification.shown": true,
                "browser.download.useDownloadDir": false,
                "browser.download.always_ask_before_handling_new_types": true
            }}
        }}});
        let response: Value = webdriver
            .post("http://127.0.0.1:4444/session")
            .json(&capabilities)
            .send()
            .await?
            .json()
            .await?;
        let webdriver_id = response
            .pointer("/value/sessionId")
            .and_then(Value::as_str)
            .with_context(|| {
                let diagnostics = format!(
                    "{} {}",
                    fs::read_to_string(dir.join("driver.stderr")).unwrap_or_default(),
                    fs::read_to_string(dir.join("driver.log")).unwrap_or_default()
                );
                format!(
                    "browser_launch_failed: {}; startup diagnostics: {}",
                    response.get("value").unwrap_or(&response),
                    diagnostics.chars().take(2000).collect::<String>()
                )
            })?
            .to_owned();
        let containment: Value = serde_json::from_slice(&fs::read(dir.join("containment.json"))?)?;
        guard.committed = true;
        let runtime = Self {
            dir,
            headed,
            tor,
            sandbox,
            webdriver,
            webdriver_id,
            containment,
        };
        runtime
            .call(
                "POST",
                "timeouts",
                Some(json!({"pageLoad": 60000, "script": 15000, "implicit": 0})),
            )
            .await?;
        Ok(runtime)
    }
    pub async fn call(&self, method: &str, endpoint: &str, body: Option<Value>) -> Result<Value> {
        let suffix = if endpoint.is_empty() {
            String::new()
        } else {
            format!("/{endpoint}")
        };
        let url = format!(
            "http://127.0.0.1:4444/session/{}{suffix}",
            self.webdriver_id
        );
        let mut req = self
            .webdriver
            .request(method.parse::<reqwest::Method>()?, url);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response: Value = req
            .send()
            .await
            .context("webdriver_unreachable")?
            .json()
            .await?;
        let value = response
            .get("value")
            .context("invalid_webdriver_response")?;
        if let Some(error) = value.get("error").and_then(Value::as_str) {
            bail!(
                "webdriver_{error}: {}",
                value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("request failed")
            );
        }
        Ok(value.clone())
    }
    pub async fn script(&self, script: &str, args: Value) -> Result<Value> {
        self.call(
            "POST",
            "execute/sync",
            Some(json!({"script": script, "args": args})),
        )
        .await
    }
    pub async fn stop(mut self) {
        let _ = timeout(Duration::from_secs(3), self.call("DELETE", "", None)).await;
        let _ = self.sandbox.kill().await;
        // Let Tor flush its guard state before using SIGKILL as a fallback.
        if let Some(pid) = self.tor.id() {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        if timeout(Duration::from_secs(3), self.tor.wait())
            .await
            .is_err()
        {
            let _ = self.tor.kill().await;
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
    pub async fn status(&mut self) -> Value {
        let alive = self.tor.try_wait().map(|s| s.is_none()).unwrap_or(false);
        let response = tor_control(&self.dir, "GETINFO status/bootstrap-phase")
            .await
            .ok();
        json!({"running": true, "tor_alive": alive, "tor_pid": self.tor.id(), "sandbox_pid": self.sandbox.id(), "bootstrap_ready": response.as_ref().is_some_and(|r| r.contains("PROGRESS=100")), "containment": self.containment, "headed": self.headed, "onion_connections_use_exit_nodes": false})
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.sandbox.start_kill();
        let _ = self.tor.start_kill();
        let _ = fs::remove_dir_all(&self.dir);
    }
}
struct LaunchGuard {
    dir: PathBuf,
    committed: bool,
}
impl Drop for LaunchGuard {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

fn tor_quote(path: &Path) -> Result<String> {
    let text = path.to_str().context("Tor paths must be valid UTF-8")?;
    if text.contains(['\n', '\r', '"', '\\']) {
        bail!("unsafe_tor_path: unsupported control character or quote in path");
    }
    Ok(format!("\"{text}\""))
}

pub fn sandbox_args(config: &Config, dir: &Path, headed: bool) -> Result<Vec<String>> {
    let exe = std::env::current_exe()?;
    let mut args = vec![
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
        "--ro-bind",
        "/nix/store",
        "/nix/store",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
        "--dir",
        "/etc",
        "--dir",
        "/home",
        "--bind",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    args.extend([
        dir.display().to_string(),
        "/session".into(),
        "--ro-bind".into(),
        exe.display().to_string(),
        "/onion-browser".into(),
    ]);
    for file in ["/etc/passwd", "/etc/group"] {
        if Path::new(file).exists() {
            args.extend(["--ro-bind".into(), file.into(), file.into()]);
        }
    }
    for (key, value) in [
        ("HOME", "/session/home".to_owned()),
        ("PATH", std::env::var("PATH")?),
        ("TOR_SKIP_LAUNCH", "1".into()),
        ("TOR_SOCKS_IPC_PATH", "/session/socks.socket".into()),
        ("TOR_CONTROL_IPC_PATH", "/session/control.socket".into()),
        (
            "TOR_CONTROL_COOKIE_AUTH_FILE",
            "/session/control.cookie".into(),
        ),
        ("MOZ_ENABLE_WAYLAND", "1".into()),
    ] {
        args.extend(["--setenv".into(), key.into(), value]);
    }
    if headed {
        if let (Ok(runtime), Ok(display)) = (
            std::env::var("XDG_RUNTIME_DIR"),
            std::env::var("WAYLAND_DISPLAY"),
        ) {
            if display.contains('/') {
                bail!("headed_display_invalid: use a local Wayland socket name");
            }
            let socket = Path::new(&runtime).join(&display);
            if !socket.exists() {
                bail!("headed_display_unavailable: Wayland socket is absent");
            }
            args.extend([
                "--dir".into(),
                "/display".into(),
                "--ro-bind".into(),
                socket.display().to_string(),
                format!("/display/{display}"),
                "--setenv".into(),
                "XDG_RUNTIME_DIR".into(),
                "/display".into(),
                "--setenv".into(),
                "WAYLAND_DISPLAY".into(),
                display,
            ]);
        } else if let Ok(display) = std::env::var("DISPLAY") {
            if !display.starts_with(':') {
                bail!("headed_display_invalid: only local X11 displays are supported");
            }
            let number = display
                .trim_start_matches(':')
                .split('.')
                .next()
                .unwrap_or("");
            if !number.bytes().all(|b| b.is_ascii_digit()) || number.is_empty() {
                bail!("headed_display_invalid");
            }
            let socket = format!("/tmp/.X11-unix/X{number}");
            args.extend([
                "--dir".into(),
                "/tmp/.X11-unix".into(),
                "--ro-bind".into(),
                socket.clone(),
                socket,
                "--setenv".into(),
                "DISPLAY".into(),
                display,
                "--setenv".into(),
                "MOZ_ENABLE_WAYLAND".into(),
                "0".into(),
            ]);
            let auth = std::env::var_os("XAUTHORITY")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".Xauthority")));
            if let Some(auth) = auth.filter(|p| p.is_file()) {
                args.extend([
                    "--ro-bind".into(),
                    auth.display().to_string(),
                    "/display-auth".into(),
                    "--setenv".into(),
                    "XAUTHORITY".into(),
                    "/display-auth".into(),
                ]);
            }
        } else {
            bail!("headed_display_unavailable: set WAYLAND_DISPLAY or a local DISPLAY");
        }
    }
    args.extend([
        "--chdir".into(),
        "/session".into(),
        "/onion-browser".into(),
        "internal-driver".into(),
        "--geckodriver".into(),
        config.geckodriver.display().to_string(),
    ]);
    Ok(args)
}

pub async fn tor_control(dir: &Path, command: &str) -> Result<String> {
    timeout(Duration::from_secs(3), async {
        let cookie = fs::read(dir.join("control.cookie"))?;
        let cookie = cookie
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let stream = UnixStream::connect(dir.join("control.socket")).await?;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        writer
            .write_all(format!("AUTHENTICATE {cookie}\r\n").as_bytes())
            .await?;
        let auth = control_reply(&mut reader).await?;
        if !auth.starts_with("250") {
            bail!("Tor control authentication failed");
        }
        writer
            .write_all(format!("{command}\r\n").as_bytes())
            .await?;
        control_reply(&mut reader).await
    })
    .await
    .context("Tor control timeout")?
}
async fn control_reply(reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>) -> Result<String> {
    let mut result = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            bail!("Tor control closed");
        }
        if result.len() + line.len() > 16_384 {
            bail!("Tor control reply too large");
        }
        let done = line.as_bytes().get(3) == Some(&b' ');
        result.push_str(&line);
        if done {
            return Ok(result);
        }
    }
}

pub async fn internal_driver(geckodriver: &Path) -> Result<()> {
    // These probes are made before launching the browser, in the same namespace.
    let tcp_blocked = timeout(Duration::from_secs(1), TcpStream::connect("1.1.1.1:443"))
        .await
        .is_ok_and(|r| r.is_err());
    let udp = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    let dns_blocked = udp.send_to(&[0; 12], "1.1.1.1:53").await.is_err();
    if !tcp_blocked || !dns_blocked {
        bail!("containment_failed: direct TCP or DNS egress is available");
    }
    let namespace = fs::read_link("/proc/self/ns/net")?.display().to_string();
    fs::write(
        "/session/containment.json",
        serde_json::to_vec(
            &json!({"network_namespace": namespace, "direct_tcp_blocked": tcp_blocked, "direct_dns_blocked": dns_blocked, "browser_filesystem": "store, own session, minimal system identity, optional display"}),
        )?,
    )?;
    let mut driver = child_command(geckodriver)
        .args(["--host", "127.0.0.1", "--port", "4444", "--log", "error"])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    let listener = UnixListener::bind("/session/webdriver.socket")?;
    fs::set_permissions(
        "/session/webdriver.socket",
        fs::Permissions::from_mode(0o600),
    )?;
    loop {
        tokio::select! {
            status = driver.wait() => bail!("geckodriver exited: {}", status?),
            connection = listener.accept() => {
                let (mut stream, _) = connection?;
                tokio::spawn(async move {
                    if let Ok(mut tcp) = TcpStream::connect("127.0.0.1:4444").await {
                        let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
                    }
                });
            }
        }
    }
}
