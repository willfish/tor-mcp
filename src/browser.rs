use crate::{
    protocol::*,
    runtime::{Config, Runtime},
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use tokio::time::{sleep, timeout};

const PAGE: &str = include_str!("page.js");
const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
struct Reference {
    element: Value,
    signature: String,
}
pub struct Session {
    pub runtime: Runtime,
    pub last_used: Instant,
    nonce: String,
    generation: u64,
    refs: HashMap<String, Reference>,
    tabs: BTreeMap<String, String>,
    next_tab: u64,
    no_tabs: bool,
}
impl Session {
    pub async fn launch(config: &Config, name: &str, headed: bool) -> Result<Self> {
        Ok(Self {
            runtime: Runtime::launch(config, name, headed).await?,
            last_used: Instant::now(),
            nonce: format!(
                "{}-{}",
                std::process::id(),
                NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
            ),
            generation: 0,
            refs: HashMap::new(),
            tabs: BTreeMap::new(),
            next_tab: 1,
            no_tabs: false,
        })
    }
    fn invalidate(&mut self) {
        self.generation += 1;
        self.refs.clear();
    }
    async fn info(&self) -> Result<Value> {
        let url = self.runtime.call("GET", "url", None).await?;
        let title = self.runtime.call("GET", "title", None).await?;
        Ok(json!({"url": url, "title": title, "untrusted": true}))
    }
    async fn open(&mut self, url: &str) -> Result<Value> {
        validate_url(url)?;
        if self.no_tabs {
            bail!("session_has_no_tabs: close this session and open a new one");
        }
        self.invalidate();
        self.runtime
            .call("POST", "url", Some(json!({"url": url})))
            .await?;
        self.info().await
    }
    async fn element(&self, target: &str) -> Result<String> {
        let value = if target.starts_with('@') {
            let reference = self.refs.get(target).context(
                "stale_reference: take a new snapshot in this session and use its reference",
            )?;
            let signature = self
                .runtime
                .script(
                    "return arguments[0].outerHTML.slice(0, 2000)",
                    json!([reference.element]),
                )
                .await?;
            if signature.as_str() != Some(&reference.signature) {
                bail!("stale_reference: element changed since the snapshot; take a fresh snapshot");
            }
            reference.element.clone()
        } else {
            if target.is_empty() || target.len() > 1000 {
                bail!("invalid_target: use a nonempty CSS selector or a snapshot reference");
            }
            self.runtime
                .call(
                    "POST",
                    "element",
                    Some(json!({"using": "css selector", "value": target})),
                )
                .await?
        };
        Ok(value
            .get(ELEMENT_KEY)
            .and_then(Value::as_str)
            .context("invalid_element_response")?
            .to_owned())
    }
    async fn sync_tabs(&mut self) -> Result<()> {
        let handles = self.runtime.call("GET", "window/handles", None).await?;
        let handles = handles.as_array().context("invalid_tab_response")?;
        self.tabs
            .retain(|_, handle| handles.iter().any(|h| h.as_str() == Some(handle)));
        for handle in handles.iter().filter_map(Value::as_str) {
            if !self.tabs.values().any(|h| h == handle) {
                self.tabs
                    .insert(format!("t{}", self.next_tab), handle.to_owned());
                self.next_tab += 1;
            }
        }
        Ok(())
    }
    pub async fn execute(&mut self, action: Action) -> Result<Value> {
        self.last_used = Instant::now();
        let result = self.execute_inner(action).await;
        self.last_used = Instant::now();
        result
    }
    async fn execute_inner(&mut self, action: Action) -> Result<Value> {
        match action {
            Action::BrowserOpen(a) => {
                if a.headed != self.runtime.headed {
                    bail!("mode_mismatch: close this session before changing headed mode");
                }
                self.open(&a.url).await
            }
            Action::BrowserSnapshot(a) => {
                self.invalidate();
                let mut snapshot = self
                    .runtime
                    .script(PAGE, json!(["snapshot", a.interactive]))
                    .await?;
                if let Some(nodes) = snapshot.get_mut("nodes").and_then(Value::as_array_mut) {
                    for (i, node) in nodes.iter_mut().enumerate() {
                        let Some(object) = node.as_object_mut() else {
                            continue;
                        };
                        let element = object.remove("element");
                        let signature = object.remove("signature");
                        if let (Some(element), Some(signature)) = (
                            element.filter(|e| e.get(ELEMENT_KEY).is_some()),
                            signature.and_then(|s| s.as_str().map(str::to_owned)),
                        ) {
                            let id = format!("@r{}-{}-{}", self.nonce, self.generation, i + 1);
                            self.refs
                                .insert(id.clone(), Reference { element, signature });
                            object.insert("ref".into(), json!(id));
                        }
                    }
                }
                Ok(snapshot)
            }
            Action::BrowserRead(_) => self.runtime.script(PAGE, json!(["read"])).await,
            Action::BrowserLinks(a) => {
                let links = self.runtime.script(PAGE, json!(["links"])).await?;
                Ok(
                    json!({"links": filter_links(links, a.onion_only), "untrusted": true, "limit": 500}),
                )
            }
            Action::BrowserClick(a) => {
                let element = self.element(&a.target).await?;
                let value = json!({ELEMENT_KEY: element});
                let href = self
                    .runtime
                    .script(
                        "return arguments[0].closest('a[href]')?.href || null",
                        json!([value]),
                    )
                    .await?;
                if let Some(href) = href.as_str() {
                    validate_url(href)?;
                }
                self.invalidate();
                self.runtime
                    .call("POST", &format!("element/{element}/click"), Some(json!({})))
                    .await?;
                self.info().await
            }
            Action::BrowserFill(a) => {
                let element = self.element(&a.target).await?;
                self.invalidate();
                // WebDriver send-keys interprets text as local filenames for a
                // file input. DOM value assignment cannot attach files, even if
                // the page changes the input type during focus/input events.
                self.runtime
                    .script(PAGE, json!(["fill", {ELEMENT_KEY: element}, a.text]))
                    .await
            }
            Action::BrowserPress(a) => {
                let key = key_code(&a.key)?;
                self.invalidate();
                let result = self.runtime.call("POST", "actions", Some(json!({"actions":[{"type":"key","id":"keyboard","actions":[{"type":"keyDown","value":key},{"type":"keyUp","value":key}]}]}))).await;
                let _ = self.runtime.call("DELETE", "actions", None).await;
                result?;
                Ok(json!({"pressed": a.key}))
            }
            Action::BrowserWait(a) => timeout(Duration::from_millis(a.timeout_ms), async {
                loop {
                    let found = self
                        .runtime
                        .script(
                            "return (document.body?.innerText || '').includes(arguments[0])",
                            json!([a.text]),
                        )
                        .await?;
                    if found == true {
                        return Ok::<_, anyhow::Error>(json!({"found": true}));
                    }
                    sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .context("wait_timeout: the requested text did not appear")?,
            Action::BrowserScreenshot(_) => {
                let image = self.runtime.call("GET", "screenshot", None).await?;
                Ok(json!({"mime_type": "image/png", "base64": image, "untrusted": true}))
            }
            Action::BrowserNavigate(a) => {
                self.invalidate();
                let endpoint = match a.action {
                    Navigation::Back => "back",
                    Navigation::Forward => "forward",
                    Navigation::Reload => "refresh",
                };
                self.runtime.call("POST", endpoint, Some(json!({}))).await?;
                self.info().await
            }
            Action::BrowserTabs(a) => {
                if self.no_tabs {
                    bail!("session_has_no_tabs: close this session and open a new one");
                }
                self.sync_tabs().await?;
                match a.action {
                    TabAction::List => {}
                    TabAction::New => {
                        self.invalidate();
                        let tab = self
                            .runtime
                            .call("POST", "window/new", Some(json!({"type": "tab"})))
                            .await?;
                        let handle = tab
                            .get("handle")
                            .and_then(Value::as_str)
                            .context("invalid_tab_response")?;
                        self.runtime
                            .call("POST", "window", Some(json!({"handle": handle})))
                            .await?;
                        if let Some(url) = &a.url {
                            self.open(url).await?;
                        }
                    }
                    TabAction::Select | TabAction::Close => {
                        let id = a.tab.context("tab_required")?;
                        let handle = self
                            .tabs
                            .get(&id)
                            .context("unknown_tab: select a stable ID from browser_tabs list")?
                            .clone();
                        self.invalidate();
                        self.runtime
                            .call("POST", "window", Some(json!({"handle": handle})))
                            .await?;
                        if matches!(a.action, TabAction::Close) {
                            let remaining = self.runtime.call("DELETE", "window", None).await?;
                            self.tabs.remove(&id);
                            if remaining.as_array().is_some_and(Vec::is_empty) {
                                self.no_tabs = true;
                                return Ok(
                                    json!({"tabs": [], "next_action": "close this session, then open a new one"}),
                                );
                            }
                            let successor = remaining
                                .as_array()
                                .and_then(|a| a.first())
                                .context("invalid_tab_response")?;
                            self.runtime
                                .call("POST", "window", Some(json!({"handle": successor})))
                                .await?;
                        }
                    }
                }
                self.sync_tabs().await?;
                let current = self.runtime.call("GET", "window", None).await?;
                Ok(
                    json!({"tabs": self.tabs.iter().map(|(id, handle)| json!({"id": id, "active": current.as_str() == Some(handle)})).collect::<Vec<_>>()}),
                )
            }
            Action::OnionSearch(a) => {
                let mut url = url::Url::parse("https://ahmia.fi/search/")?;
                url.query_pairs_mut().append_pair("q", &a.query);
                // Use the provider's normal form, including its rolling hidden
                // field. A bare search URL is redirected to the homepage.
                let fresh = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_millis();
                self.open(&format!("https://ahmia.fi/?_tor_mcp={fresh}"))
                    .await?;
                let request = self
                    .runtime
                    .script(PAGE, json!(["search_request", a.query]))
                    .await?;
                let request = url::Url::parse(
                    request
                        .as_str()
                        .context("search_provider_unrecognized: Ahmia search form is absent")?,
                )?;
                if request.scheme() != "https"
                    || request.host_str() != Some("ahmia.fi")
                    || request.path() != "/search/"
                {
                    bail!("search_provider_unrecognized: unexpected Ahmia form destination");
                }
                self.open(request.as_str()).await?;
                let page = self.runtime.script(PAGE, json!(["search"])).await?;
                let results = search_results(page, url.as_str(), a.limit)?;
                Ok(
                    json!({"query": a.query, "provider": "Ahmia", "source": url.as_str(), "results": results, "verified_identity": false, "untrusted": true}),
                )
            }
            Action::OnionDiscover(a) => {
                self.open(&a.url).await?;
                let mut claims =
                    filter_links(self.runtime.script(PAGE, json!(["discover"])).await?, true);
                let header = self.runtime.call("POST", "execute/async", Some(json!({"script": "const done = arguments[arguments.length - 1]; fetch(location.href, {method:'HEAD', credentials:'omit', redirect:'error'}).then(r => done({url:r.headers.get('Onion-Location'), source:location.href, method:'onion_location_header', verified_identity:false})).catch(() => done(null));", "args": []}))).await;
                if let Ok(header) = header {
                    claims.extend(filter_links(json!([header]), true));
                }
                let mut seen = std::collections::HashSet::new();
                claims.retain(|r| {
                    seen.insert((
                        r.get("url").cloned().unwrap_or(Value::Null).to_string(),
                        r.get("method").cloned().unwrap_or(Value::Null).to_string(),
                    ))
                });
                Ok(json!({"claims": claims, "untrusted": true, "verified_identity": false}))
            }
            Action::TorStatus(_) => Ok(self.runtime.status().await),
            Action::SessionList(_) | Action::SessionClose(_) => bail!("internal_dispatch_error"),
        }
    }
}
pub fn search_results(page: Value, source: &str, limit: usize) -> Result<Vec<Value>> {
    if page["recognized"] != true {
        bail!(
            "search_provider_unrecognized: Ahmia did not return a recognized results page; it may be unavailable or have changed format"
        );
    }
    let mut results = filter_links(page["results"].clone(), true);
    if results.is_empty() && page["no_results"] != true {
        bail!("search_provider_unrecognized: Ahmia results contained no valid onion addresses");
    }
    let mut seen = std::collections::HashSet::new();
    results.retain(|r| r["url"].as_str().is_some_and(|u| seen.insert(u.to_owned())));
    results.truncate(limit);
    for result in &mut results {
        result["source"] = json!(source);
    }
    Ok(results)
}

pub fn filter_links(links: Value, onion_only: bool) -> Vec<Value> {
    links
        .as_array()
        .into_iter()
        .flatten()
        .filter(|link| {
            link.get("url")
                .and_then(Value::as_str)
                .and_then(|u| validate_url(u).ok())
                .is_some_and(|u| !onion_only || u.host_str().is_some_and(valid_onion))
        })
        .cloned()
        .collect()
}
