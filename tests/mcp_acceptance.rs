//! Runnable evidence for the plain-language cases in docs/manual-acceptance.md.
//! Live browsing cases are ignored; discovery and configuration checks run offline.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::HashMap, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};

struct Client {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
    pending: HashMap<u64, Value>,
    root: tempfile::TempDir,
}
impl Client {
    async fn start() -> Result<Self> {
        Self::start_with_idle(600).await
    }
    async fn start_with_idle(idle_seconds: u64) -> Result<Self> {
        let root = tempfile::tempdir()?;
        let mut child = Command::new(env!("CARGO_BIN_EXE_tor-mcp"))
            .arg("mcp")
            .env("XDG_RUNTIME_DIR", root.path())
            .env(
                "XDG_STATE_HOME",
                std::env::var_os("TOR_MCP_TEST_STATE_HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| root.path().join("state")),
            )
            .env("TOR_MCP_IDLE_SECONDS", idle_seconds.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let input = child.stdin.take().context("stdin")?;
        let output = BufReader::new(child.stdout.take().context("stdout")?);
        let mut client = Self {
            child,
            input,
            output,
            next: 1,
            pending: HashMap::new(),
            root,
        };
        let id = client.send("initialize", json!({"protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":"manual-acceptance", "version":"1"}})).await?;
        client.receive(id).await?;
        client
            .input
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await?;
        Ok(client)
    }
    async fn send(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next;
        self.next += 1;
        let mut bytes = serde_json::to_vec(
            &json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
        )?;
        bytes.push(b'\n');
        self.input.write_all(&bytes).await?;
        Ok(id)
    }
    async fn receive(&mut self, id: u64) -> Result<Value> {
        if let Some(value) = self.pending.remove(&id) {
            return Ok(value);
        }
        loop {
            let mut line = String::new();
            if timeout(Duration::from_secs(360), self.output.read_line(&mut line)).await?? == 0 {
                bail!("MCP closed before responding");
            }
            let response: Value = serde_json::from_str(&line)?;
            if let Some(other) = response["id"].as_u64() {
                if other == id {
                    return Ok(response);
                }
                self.pending.insert(other, response);
            }
        }
    }
    async fn tool(&mut self, name: &str, args: Value) -> Result<Value> {
        let id = self
            .send("tools/call", json!({"name":name,"arguments":args}))
            .await?;
        self.receive(id).await
    }
    async fn data(&mut self, name: &str, args: Value) -> Result<Value> {
        data(self.tool(name, args).await?)
    }
    async fn finish(mut self) -> Result<()> {
        let output = Command::new(env!("CARGO_BIN_EXE_tor-mcp"))
            .arg("shutdown")
            .env("XDG_RUNTIME_DIR", self.root.path())
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .output()
            .await?;
        assert!(output.status.success());
        self.child.kill().await?;
        assert!(!self.root.path().join("tor-mcp/daemon.socket").exists());
        Ok(())
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = std::process::Command::new(env!("CARGO_BIN_EXE_tor-mcp"))
            .arg("shutdown")
            .env("XDG_RUNTIME_DIR", self.root.path())
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
fn data(response: Value) -> Result<Value> {
    if response.get("error").is_some() || response["result"]["isError"] == true {
        bail!("tool failed: {response}");
    }
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .context("text result")?;
    Ok(serde_json::from_str::<Value>(text)?["data"].clone())
}

#[tokio::test]
async fn mcp_discovery_is_lazy() -> Result<()> {
    let mut client = Client::start().await?;
    let id = client.send("tools/list", json!({})).await?;
    let tools = client.receive(id).await?;
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 16);
    assert!(
        !client.root.path().join("tor-mcp").exists(),
        "initialization and tools/list must not even start the daemon"
    );
    assert_eq!(
        client.data("session_list", json!({})).await?["sessions"],
        json!([])
    );
    assert_eq!(
        client.data("tor_status", json!({"session":"lazy"})).await?["running"],
        false
    );
    assert!(!client.root.path().join("tor-mcp/lazy").exists());
    let duplicate = Command::new(env!("CARGO_BIN_EXE_tor-mcp"))
        .arg("daemon")
        .env("XDG_RUNTIME_DIR", client.root.path())
        .env("XDG_STATE_HOME", client.root.path().join("state"))
        .output()
        .await?;
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("daemon_already_running"));
    // A rejected foreground start must not remove the live daemon's socket.
    assert_eq!(
        client.data("session_list", json!({})).await?["sessions"],
        json!([])
    );
    assert_eq!(
        client
            .data("session_close", json!({"session":"lazy"}))
            .await?["was_running"],
        false
    );
    client.finish().await
}

#[tokio::test]
async fn explicit_browser_path_does_not_require_tor_browser_on_path() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let bin = temp.path().join("bin");
    std::fs::create_dir(&bin)?;
    // Runtime resolution can find the other commands, but not tor-browser.
    // None of these placeholders should ever execute during shutdown.
    for name in ["geckodriver", "tor", "bwrap", "explicit-browser"] {
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_tor-mcp"), bin.join(name))?;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_tor-mcp"))
        .arg("shutdown")
        .env("PATH", &bin)
        .env("TOR_MCP_BINARY", bin.join("explicit-browser"))
        .env("TOR_MCP_BUNDLE", temp.path().join("bundle"))
        .env("XDG_RUNTIME_DIR", temp.path())
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .output()
        .await?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(result["was_running"], false);
    assert!(!temp.path().join("tor-mcp/daemon.socket").exists());
    Ok(())
}

#[tokio::test]
#[ignore = "requires a desktop display and live Ahmia access through Tor"]
async fn headed_and_live_search_acceptance() -> Result<()> {
    let mut client = Client::start_with_idle(30).await?;
    if let (Ok(runtime), Ok(display)) = (
        std::env::var("XDG_RUNTIME_DIR"),
        std::env::var("WAYLAND_DISPLAY"),
    ) {
        std::os::unix::fs::symlink(
            std::path::Path::new(&runtime).join(&display),
            client.root.path().join(display),
        )?;
    }
    assert_eq!(
        client
            .data(
                "browser_open",
                json!({"session":"headed","url":"https://example.com/","headed":true})
            )
            .await?["title"],
        "Example Domain"
    );
    let mismatch = client
        .tool(
            "browser_open",
            json!({"session":"headed","url":"https://example.com/","headed":false}),
        )
        .await?;
    assert_eq!(mismatch["result"]["isError"], true);
    let first = client.send("tools/call", json!({"name":"onion_search","arguments":{"session":"search-one","query":"independent journalism","limit":3}})).await?;
    let second = client.send("tools/call", json!({"name":"onion_search","arguments":{"session":"search-two","query":"Tor Project","limit":3}})).await?;
    for (id, query) in [(first, "independent journalism"), (second, "Tor Project")] {
        let result = data(client.receive(id).await?)?;
        assert_eq!(result["query"], query);
        let results = result["results"].as_array().context("search results")?;
        assert!(!results.is_empty());
        assert!(results.len() <= 3);
        assert_eq!(result["verified_identity"], false);
        for item in results {
            assert!(item["url"].as_str().unwrap().contains(".onion"));
            assert!(
                item["source"]
                    .as_str()
                    .unwrap()
                    .starts_with("https://ahmia.fi/search/")
            );
        }
    }
    tokio::time::sleep(Duration::from_secs(33)).await;
    assert_eq!(
        client
            .data("tor_status", json!({"session":"headed"}))
            .await?["headed"],
        true,
        "headed sessions must not be idle-reaped"
    );
    client.finish().await
}

#[tokio::test]
#[ignore = "requires a harmless local onion fixture; set TOR_MCP_TEST_URL"]
async fn navigation_recovery_and_text_discovery() -> Result<()> {
    let url = std::env::var("TOR_MCP_TEST_URL")
        .context("set TOR_MCP_TEST_URL to the fixture /one URL")?;
    let mut client = Client::start().await?;
    let session = "model-recovery";
    client
        .data("browser_open", json!({"session":session,"url":url}))
        .await?;
    let tabs = client
        .data("browser_tabs", json!({"session":session,"action":"list"}))
        .await?;
    let healthy = tabs["tabs"][0]["id"]
        .as_str()
        .context("original tab")?
        .to_owned();
    client
        .data(
            "browser_tabs",
            json!({"session":session,"action":"new","url":url}),
        )
        .await?;
    let tabs = client
        .data("browser_tabs", json!({"session":session,"action":"list"}))
        .await?;
    let failed_tab = tabs["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tab| tab["active"] == true)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let slow = format!(
        "{}/slow",
        url.trim_end_matches("/one").trim_end_matches('/')
    );
    let failed = client
        .tool("browser_open", json!({"session":session,"url":slow}))
        .await?;
    assert_eq!(failed["result"]["isError"], true);
    let error: Value =
        serde_json::from_str(failed["result"]["content"][0]["text"].as_str().unwrap())?;
    assert_eq!(error["error"]["code"], "navigation_failed");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("webdriver_timeout")
    );
    for (name, args) in [
        ("browser_read", json!({"session":session})),
        ("browser_snapshot", json!({"session":session})),
        ("browser_links", json!({"session":session})),
        ("browser_screenshot", json!({"session":session})),
        (
            "browser_click",
            json!({"session":session,"target":"#change"}),
        ),
        (
            "browser_fill",
            json!({"session":session,"target":"#name","text":"must not fill"}),
        ),
        ("browser_press", json!({"session":session,"key":"Enter"})),
        ("browser_wait", json!({"session":session,"text":"Fixture"})),
    ] {
        let response = client.tool(name, args).await?;
        assert_eq!(response["result"]["isError"], true, "{name}");
        let error: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())?;
        assert_eq!(error["error"]["code"], "navigation_unconfirmed", "{name}");
        assert!(error["error"]["next_action"].is_string());
    }
    client
        .data(
            "browser_tabs",
            json!({"session":session,"action":"select","tab":healthy}),
        )
        .await?;
    assert_eq!(
        client
            .data("browser_read", json!({"session":session}))
            .await?["title"],
        "Fixture One"
    );
    client
        .data(
            "browser_tabs",
            json!({"session":session,"action":"select","tab":failed_tab}),
        )
        .await?;
    assert_eq!(
        client
            .tool("browser_read", json!({"session":session}))
            .await?["result"]["isError"],
        true
    );
    client
        .data("browser_open", json!({"session":session,"url":url}))
        .await?;
    assert_eq!(
        client
            .data("browser_read", json!({"session":session}))
            .await?["title"],
        "Fixture One"
    );
    let discovery = client
        .data("onion_discover", json!({"session":session,"url":url}))
        .await?;
    assert!(
        discovery["claims"]
            .as_array()
            .unwrap()
            .iter()
            .any(|claim| claim["method"] == "publisher_text"
                && claim["url"].as_str().unwrap().ends_with("/text-only"))
    );
    assert!(
        discovery["claims"]
            .as_array()
            .unwrap()
            .iter()
            .all(|claim| !claim["url"].as_str().unwrap().contains(&"a".repeat(56)))
    );
    let bounded = client
        .data(
            "onion_discover",
            json!({"session":session,"url":url,"limit":1}),
        )
        .await?;
    assert_eq!(bounded["claims"].as_array().unwrap().len(), 1);
    assert_eq!(bounded["truncated"], true);
    assert_eq!(bounded["source"], url);
    assert_eq!(bounded["verified_identity"], false);
    client.finish().await
}

#[tokio::test]
#[ignore = "requires a harmless local onion fixture; set TOR_MCP_TEST_URL"]
async fn local_onion_command_acceptance() -> Result<()> {
    let url = std::env::var("TOR_MCP_TEST_URL")
        .context("set TOR_MCP_TEST_URL to the fixture /one URL")?;
    let mut client = Client::start().await?;
    let one = client
        .send(
            "tools/call",
            json!({"name":"browser_open","arguments":{"session":"one","url":url}}),
        )
        .await?;
    let two = client
        .send(
            "tools/call",
            json!({"name":"browser_open","arguments":{"session":"two","url":url}}),
        )
        .await?;
    assert_eq!(data(client.receive(one).await?)?["title"], "Fixture One");
    assert_eq!(data(client.receive(two).await?)?["title"], "Fixture One");
    assert_eq!(
        client.data("session_list", json!({})).await?["sessions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let status = client.data("tor_status", json!({"session":"one"})).await?;
    assert_eq!(status["bootstrap_ready"], true);
    assert_eq!(status["containment"]["direct_tcp_blocked"], true);
    assert_eq!(status["containment"]["direct_dns_blocked"], true);
    let other_status = client.data("tor_status", json!({"session":"two"})).await?;
    assert_ne!(
        status["containment"]["network_namespace"],
        other_status["containment"]["network_namespace"]
    );
    let upload = client
        .tool(
            "browser_fill",
            json!({"session":"one","target":"#upload","text":"/etc/group"}),
        )
        .await?;
    assert_eq!(
        upload["result"]["isError"], true,
        "file attachment must be disabled"
    );
    assert!(upload.to_string().contains("upload_disabled"));
    let snapshot = client
        .data(
            "browser_snapshot",
            json!({"session":"one","interactive":true}),
        )
        .await?;
    assert!(
        !snapshot.to_string().contains("not-for-snapshots"),
        "password value leaked into snapshot"
    );
    assert!(
        snapshot["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["name"] == "Test password")
    );
    let reference = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "Your test name")
        .context("field label")?["ref"]
        .as_str()
        .context("field ref")?
        .to_owned();
    let wrong = client
        .tool(
            "browser_fill",
            json!({"session":"two","target":reference,"text":"wrong"}),
        )
        .await?;
    assert_eq!(wrong["result"]["isError"], true);
    client
        .data(
            "browser_fill",
            json!({"session":"one","target":reference,"text":"Alice"}),
        )
        .await?;
    client
        .data(
            "browser_fill",
            json!({"session":"two","target":"#name","text":"Bob"}),
        )
        .await?;
    let stale = client
        .tool(
            "browser_fill",
            json!({"session":"one","target":reference,"text":"not applied"}),
        )
        .await?;
    assert_eq!(stale["result"]["isError"], true);
    let missing = client
        .tool("browser_click", json!({"session":"one","target":"#absent"}))
        .await?;
    assert_eq!(missing["result"]["isError"], true);
    client
        .data("browser_click", json!({"session":"one","target":"#change"}))
        .await?;
    client
        .data("browser_click", json!({"session":"two","target":"#change"}))
        .await?;
    assert!(
        client
            .data("browser_read", json!({"session":"one"}))
            .await?["text"]
            .as_str()
            .unwrap()
            .contains("Hello Alice")
    );
    assert!(
        client
            .data("browser_read", json!({"session":"two"}))
            .await?["text"]
            .as_str()
            .unwrap()
            .contains("Hello Bob")
    );
    client
        .data(
            "browser_fill",
            json!({"session":"one","target":"#name","text":"Alice"}),
        )
        .await?;
    client
        .data("browser_press", json!({"session":"one","key":"Enter"}))
        .await?;
    client
        .data(
            "browser_wait",
            json!({"session":"one","text":"Enter recorded","timeout_ms":1000}),
        )
        .await?;
    let missed = client
        .tool(
            "browser_wait",
            json!({"session":"one","text":"never appears","timeout_ms":100}),
        )
        .await?;
    assert_eq!(missed["result"]["isError"], true);
    let image = client
        .tool("browser_screenshot", json!({"session":"one"}))
        .await?;
    assert_eq!(image["result"]["content"][1]["type"], "image");
    assert_eq!(image["result"]["content"][1]["mimeType"], "image/png");
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD
        .decode(image["result"]["content"][1]["data"].as_str().unwrap())?;
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let links = client
        .data("browser_links", json!({"session":"one","onion_only":true}))
        .await?;
    assert!(!links["links"].as_array().unwrap().is_empty());
    let tabs = client
        .data("browser_tabs", json!({"session":"one","action":"list"}))
        .await?;
    let first_tab = tabs["tabs"][0]["id"].as_str().unwrap().to_owned();
    let unknown = client
        .tool(
            "browser_tabs",
            json!({"session":"one","action":"select","tab":"missing"}),
        )
        .await?;
    assert_eq!(unknown["result"]["isError"], true);
    let tabs = client
        .data(
            "browser_tabs",
            json!({"session":"one","action":"new","url":url.replace("/one", "/two")}),
        )
        .await?;
    let new_tab = tabs["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["active"] == true)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    client
        .data(
            "browser_tabs",
            json!({"session":"one","action":"select","tab":first_tab}),
        )
        .await?;
    client
        .data(
            "browser_tabs",
            json!({"session":"one","action":"close","tab":new_tab}),
        )
        .await?;
    let clicked = client
        .data("browser_click", json!({"session":"one","target":"#next"}))
        .await?;
    assert_eq!(
        clicked["title"], "Fixture Two",
        "link click must finish navigation: {clicked}"
    );
    assert_eq!(
        client
            .data("browser_navigate", json!({"session":"one","action":"back"}))
            .await?["title"],
        "Fixture One"
    );
    assert_eq!(
        client
            .data(
                "browser_navigate",
                json!({"session":"one","action":"forward"})
            )
            .await?["title"],
        "Fixture Two"
    );
    assert_eq!(
        client
            .data(
                "browser_navigate",
                json!({"session":"one","action":"reload"})
            )
            .await?["title"],
        "Fixture Two"
    );
    let discovered = client
        .data("onion_discover", json!({"session":"one","url":url}))
        .await?;
    assert!(
        discovered["claims"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["method"] == "onion_location_header")
    );
    assert_eq!(
        client
            .data("session_close", json!({"session":"one"}))
            .await?["closed"],
        true
    );
    assert!(!client.root.path().join("tor-mcp/one").exists());
    assert!(
        client
            .data("browser_read", json!({"session":"two"}))
            .await?["text"]
            .as_str()
            .unwrap()
            .contains("Hello Bob")
    );
    // The Tor process is owned by this disposable test session only.
    let tor_pid = other_status["tor_pid"].as_u64().unwrap();
    assert_eq!(unsafe { libc::kill(tor_pid as i32, libc::SIGTERM) }, 0);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        client.data("tor_status", json!({"session":"two"})).await?["tor_alive"],
        false
    );
    let failed = client
        .tool(
            "browser_open",
            json!({"session":"two","url":url.replace("/one", "/two?tor-stopped=1")}),
        )
        .await?;
    assert_eq!(
        failed["result"]["isError"], true,
        "Tor death must not allow direct egress"
    );
    client
        .data("session_close", json!({"session":"two"}))
        .await?;
    for state in [&status, &other_status] {
        for key in ["tor_pid", "sandbox_pid"] {
            assert!(
                !std::path::Path::new(&format!("/proc/{}", state[key].as_u64().unwrap())).exists(),
                "owned process survived close: {key}"
            );
        }
    }
    assert_eq!(
        client.data("session_list", json!({})).await?["sessions"],
        json!([])
    );
    client
        .data("browser_open", json!({"session":"shutdown","url":url}))
        .await?;
    let shutting_down = client
        .data("tor_status", json!({"session":"shutdown"}))
        .await?;
    client.finish().await?;
    for key in ["tor_pid", "sandbox_pid"] {
        assert!(
            !std::path::Path::new(&format!("/proc/{}", shutting_down[key].as_u64().unwrap()))
                .exists(),
            "owned process survived shutdown"
        );
    }
    // Test short idle cleanup separately so a slow second Tor bootstrap cannot
    // expire the first session during the parallel-session exercise.
    let mut idle = Client::start_with_idle(2).await?;
    idle.data("browser_open", json!({"session":"idle","url":url}))
        .await?;
    let reopened = idle.data("tor_status", json!({"session":"idle"})).await?;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        !idle.root.path().join("tor-mcp/idle").exists(),
        "idle profile survived"
    );
    for key in ["tor_pid", "sandbox_pid"] {
        assert!(
            !std::path::Path::new(&format!("/proc/{}", reopened[key].as_u64().unwrap())).exists(),
            "owned process survived idle cleanup"
        );
    }
    assert_eq!(
        idle.data("session_list", json!({})).await?["sessions"],
        json!([])
    );
    idle.data("browser_open", json!({"session":"idle","url":url}))
        .await?;
    assert_ne!(
        idle.data("tor_status", json!({"session":"idle"})).await?["sandbox_pid"],
        reopened["sandbox_pid"]
    );
    idle.finish().await
}
