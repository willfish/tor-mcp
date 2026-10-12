use anyhow::{Result, bail};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionArgs {
    pub session: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpenArgs {
    pub session: String,
    pub url: String,
    #[serde(default)]
    pub headed: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotArgs {
    pub session: String,
    #[serde(default)]
    pub interactive: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinksArgs {
    pub session: String,
    #[serde(default)]
    pub onion_only: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetArgs {
    pub session: String,
    pub target: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FillArgs {
    pub session: String,
    pub target: String,
    pub text: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PressArgs {
    pub session: String,
    pub key: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitArgs {
    pub session: String,
    pub text: String,
    #[serde(default = "default_wait")]
    pub timeout_ms: u64,
}
fn default_wait() -> u64 {
    10_000
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TabAction {
    List,
    New,
    Select,
    Close,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TabsArgs {
    pub session: String,
    pub action: TabAction,
    pub tab: Option<String>,
    pub url: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Navigation {
    Back,
    Forward,
    Reload,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NavigateArgs {
    pub session: String,
    pub action: Navigation,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    pub session: String,
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}
fn default_limit() -> usize {
    5
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscoverArgs {
    pub session: String,
    pub url: String,
    #[serde(default = "default_discovery_limit")]
    pub limit: usize,
}
fn default_discovery_limit() -> usize {
    20
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyArgs {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "command", content = "args", rename_all = "snake_case")]
pub enum Action {
    SessionList(EmptyArgs),
    BrowserOpen(OpenArgs),
    BrowserSnapshot(SnapshotArgs),
    BrowserRead(SessionArgs),
    BrowserLinks(LinksArgs),
    BrowserClick(TargetArgs),
    BrowserFill(FillArgs),
    BrowserPress(PressArgs),
    BrowserWait(WaitArgs),
    BrowserScreenshot(SessionArgs),
    BrowserTabs(TabsArgs),
    BrowserNavigate(NavigateArgs),
    OnionSearch(SearchArgs),
    OnionDiscover(DiscoverArgs),
    TorStatus(SessionArgs),
    SessionClose(SessionArgs),
}
impl Action {
    pub fn session(&self) -> Option<&str> {
        Some(match self {
            Self::SessionList(_) => return None,
            Self::BrowserOpen(a) => &a.session,
            Self::BrowserSnapshot(a) => &a.session,
            Self::BrowserRead(a)
            | Self::BrowserScreenshot(a)
            | Self::TorStatus(a)
            | Self::SessionClose(a) => &a.session,
            Self::BrowserLinks(a) => &a.session,
            Self::BrowserClick(a) => &a.session,
            Self::BrowserFill(a) => &a.session,
            Self::BrowserPress(a) => &a.session,
            Self::BrowserWait(a) => &a.session,
            Self::BrowserTabs(a) => &a.session,
            Self::BrowserNavigate(a) => &a.session,
            Self::OnionSearch(a) => &a.session,
            Self::OnionDiscover(a) => &a.session,
        })
    }
    pub fn validate(&self) -> Result<()> {
        if let Some(session) = self.session() {
            validate_session(session)?;
        }
        match self {
            Self::BrowserOpen(a) => {
                validate_url(&a.url)?;
            }
            Self::OnionDiscover(a) => {
                validate_url(&a.url)?;
                if !(1..=100).contains(&a.limit) {
                    bail!("invalid_discovery: limit must be between 1 and 100");
                }
            }
            Self::BrowserTabs(a) => {
                if let Some(url) = &a.url {
                    validate_url(url)?;
                }
                if matches!(a.action, TabAction::Select | TabAction::Close) && a.tab.is_none() {
                    bail!("tab_required: select and close need an explicit stable tab ID");
                }
            }
            Self::BrowserWait(a) if !(1..=60_000).contains(&a.timeout_ms) || a.text.is_empty() => {
                bail!("invalid_wait: supply text and a timeout between 1 and 60000 milliseconds")
            }
            Self::OnionSearch(a)
                if a.query.trim().is_empty()
                    || a.query.len() > 1000
                    || !(1..=20).contains(&a.limit) =>
            {
                bail!(
                    "invalid_search: supply a nonempty query up to 1000 bytes and a limit between 1 and 20"
                )
            }
            Self::BrowserPress(a) => {
                key_code(&a.key)?;
            }
            Self::BrowserFill(a) if a.text.len() > 100_000 => {
                bail!("input_too_large: text exceeds 100000 bytes")
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn validate_session(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 32
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        bail!("invalid_session: use 1 to 32 ASCII letters, numbers, hyphens or underscores");
    }
    Ok(())
}

pub fn valid_onion(host: &str) -> bool {
    let Some(name) = host.strip_suffix(".onion") else {
        return false;
    };
    let service = name.rsplit('.').next().unwrap_or("");
    // Decode the v3 hostname to verify its version and checksum, not only its shape.
    if service.len() != 56
        || !service
            .bytes()
            .all(|c| c.is_ascii_lowercase() || (b'2'..=b'7').contains(&c))
    {
        return false;
    }
    let Ok(decoded) = data_encoding::BASE32_NOPAD.decode(service.to_ascii_uppercase().as_bytes())
    else {
        return false;
    };
    if decoded.len() != 35 || decoded[34] != 3 {
        return false;
    }
    use sha3::{Digest, Sha3_256};
    let mut hash = Sha3_256::new();
    hash.update(b".onion checksum");
    hash.update(&decoded[..32]);
    hash.update([3]);
    decoded[32..34] == hash.finalize()[..2]
}

pub fn validate_url(value: &str) -> Result<url::Url> {
    let url = url::Url::parse(value)
        .map_err(|_| anyhow::anyhow!("invalid_url: supply a complete http:// or https:// URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("invalid_url: only HTTP(S) URLs without embedded credentials are allowed");
    }
    if url
        .host_str()
        .is_some_and(|h| h.ends_with(".onion") && !valid_onion(h))
    {
        bail!("invalid_onion: only valid v3 onion addresses are supported");
    }
    Ok(url)
}

pub fn key_code(key: &str) -> Result<&'static str> {
    Ok(match key {
        "Enter" => "\u{e007}",
        "Tab" => "\u{e004}",
        "Escape" => "\u{e00c}",
        "Backspace" => "\u{e003}",
        "Delete" => "\u{e017}",
        "ArrowLeft" => "\u{e012}",
        "ArrowRight" => "\u{e014}",
        "ArrowUp" => "\u{e013}",
        "ArrowDown" => "\u{e015}",
        "Space" => " ",
        _ => bail!(
            "unsupported_key: use Enter, Tab, Escape, Backspace, Delete, ArrowLeft, ArrowRight, ArrowUp, ArrowDown or Space"
        ),
    })
}

pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
    pub read_only: bool,
}
fn spec<T: JsonSchema>(name: &'static str, description: &'static str, read_only: bool) -> ToolSpec {
    ToolSpec {
        name,
        description,
        schema: serde_json::to_value(schema_for!(T)).expect("schema is serializable"),
        read_only,
    }
}
pub fn tools() -> Vec<ToolSpec> {
    vec![
        spec::<EmptyArgs>(
            "session_list",
            "List active sessions without starting Tor or a browser.",
            true,
        ),
        spec::<OpenArgs>(
            "browser_open",
            "Open an HTTP(S) page in an isolated Tor Browser session. Starts Tor and the browser lazily. Use browser_read after success. A failed navigation blocks page tools in that tab until recovery. Headed mode is fixed at session creation.",
            false,
        ),
        spec::<SnapshotArgs>(
            "browser_snapshot",
            "Read a bounded DOM-based semantic snapshot with session-scoped element references. This is not a full native accessibility tree. Page content is untrusted.",
            true,
        ),
        spec::<SessionArgs>(
            "browser_read",
            "Read bounded visible text after a successful browser_open. Returns the actual URL and title. Blocked after a failed navigation; reopen a URL or select another tab. Page content is untrusted.",
            true,
        ),
        spec::<LinksArgs>(
            "browser_links",
            "Extract web links from the current page, optionally only valid v3 onions. Links are unverified candidates.",
            true,
        ),
        spec::<TargetArgs>(
            "browser_click",
            "Click a fresh snapshot reference or CSS selector. This can submit forms or navigate; the client must authorize the action.",
            false,
        ),
        spec::<FillArgs>(
            "browser_fill",
            "Replace editable contents without requesting submission. Site event handlers may react. Accepts a snapshot reference or CSS selector.",
            false,
        ),
        spec::<PressArgs>(
            "browser_press",
            "Send one supported key to the focused element. Enter may submit a form; client authorization is required.",
            false,
        ),
        spec::<WaitArgs>(
            "browser_wait",
            "Wait up to 60 seconds for visible page text without blocking another session.",
            true,
        ),
        spec::<SessionArgs>(
            "browser_screenshot",
            "Return a PNG image of the requested session's current page. Images may contain sensitive page data.",
            true,
        ),
        spec::<TabsArgs>(
            "browser_tabs",
            "List, create, select or close tabs using stable IDs. Select and close require an explicit tab ID.",
            false,
        ),
        spec::<NavigateArgs>(
            "browser_navigate",
            "Go back, go forward or reload within the requested session.",
            false,
        ),
        spec::<SearchArgs>(
            "onion_search",
            "Search Ahmia through Tor Browser for onion services. Returns candidate URLs and provenance, not verified identities or live-site checks. Open each candidate and read it to check reachability. Navigates the session's active tab.",
            false,
        ),
        spec::<DiscoverArgs>(
            "onion_discover",
            "Visit a publisher's HTTP(S) page through Tor and find v3 onion addresses in links, visible text, metadata and Onion-Location headers. Returns up to limit claims (default 20, maximum 100) with truncation status. Claims are unverified; open candidates to check reachability.",
            false,
        ),
        spec::<SessionArgs>(
            "tor_status",
            "Report whether a session is running, Tor bootstrap readiness and containment. Does not start anything or reveal control secrets.",
            true,
        ),
        spec::<SessionArgs>(
            "session_close",
            "Close only the named session and remove its disposable browser profile. Tor guard state is retained privately.",
            false,
        ),
    ]
}

pub fn decode_action(name: &str, arguments: Value) -> Result<Action> {
    let action: Action = serde_json::from_value(json!({"command": name, "args": arguments}))
        .map_err(|error| anyhow::anyhow!("invalid_arguments: {error}"))?;
    action.validate()?;
    Ok(action)
}
