# tor-mcp

A Linux-first Rust CLI and MCP server for automating actual Tor Browser. It starts Tor and the browser only when needed, keeps named sessions independent, and supports headless or headed browsing.

This is an initial implementation, not complete agent-browser parity or a promise of anonymous automation. Websites can detect automation.

## Install

Requires x86-64 Linux, Nix, and permission to create unprivileged user namespaces with bubblewrap. Headed mode also requires a local Wayland or X11 display. Unsupported containment fails closed.

```sh
nix profile install github:willfish/tor-mcp/master

# Alternatively, from a checkout:
nix run . -- --help
```

The pinned flake includes Tor Browser, Tor, geckodriver and bubblewrap. It does not download a browser on first launch. Update the pin promptly for browser security releases, then rerun acceptance checks before distributing the update.

## CLI

```sh
tor-mcp --session research open https://example.com/
tor-mcp --session research snapshot --interactive
tor-mcp --session research click '<reference from the snapshot>'
tor-mcp --session research read
tor-mcp --session research links --onion-only
tor-mcp --session research screenshot page.png
tor-mcp --session research search 'independent journalism' --limit 5
tor-mcp --session research discover https://example.com/
tor-mcp --session research tor-status
tor-mcp --session research close

tor-mcp --session review open https://example.com/ --headed
tor-mcp sessions
tor-mcp shutdown
```

Use `--json` for compact JSON. Without it, output is pretty-printed JSON. `fill`, `press`, `wait`, `tabs` and `navigate` provide the other core interactions. Run `tools` for all MCP names and their argument schemas, or `call <tool-name> '<JSON arguments>'` to invoke one directly.

References belong to one session and one snapshot. Take a new snapshot after an interaction or navigation. CSS selectors are also accepted. Snapshots describe bounded DOM semantics, not Firefox's complete native accessibility tree. They include at most 200 nodes and 1,000 characters per field value, with `value_truncated` on clipped values. Password values are omitted from snapshots.

Screenshots are viewport PNGs. The CLI writes only to a new destination file; MCP returns image content. Display mode is fixed when a session is created. Close and reopen to change it.

## MCP

Install the executable, then configure your MCP client:

```json
{
  "mcpServers": {
    "tor-mcp": {
      "command": "tor-mcp",
      "args": ["mcp"]
    }
  }
}
```

Every browsing tool requires an explicit session name. Give each agent its own name. Initialization and tool listing start neither the daemon nor Tor Browser. Listing sessions or checking an unopened session may start the lightweight daemon, but not Tor or the browser.

The [manual acceptance cases](docs/manual-acceptance.md) describe every exposed tool. Commands in one session are serialized; separate sessions can run concurrently. Ahmia search and publisher discovery navigate the active tab of their session. Search results and publisher claims include provenance, not verified organisational identities. Search uses Ahmia's normal form fields and recognizes its results page instead of treating arbitrary onion links as search results. Publisher discovery also finds addresses written as visible text. It returns at most 20 claims by default, with `truncated` when more are available; set `limit` (or CLI `discover --limit`) up to 100 for a larger list.

For research, discover candidates, then use `browser_open` followed by `browser_read` to check each one. Listings are not live-site checks. A failed navigation or uncertain dispatched interaction blocks page reads, snapshots and interactions in that tab, so a previous document cannot be mistaken for a successful outcome. Recover with a successful open/navigation or select another tab; other tabs remain usable. An `interaction_failed` error does not mean the action had no effect. Check consequential outcomes independently before retrying. MCP errors retain `isError` and return JSON with `error.code`, `error.message` and `error.next_action`. Do not automatically retry consequential interactions. Tor failures never trigger direct-network fallback. Sensitive submissions require an appropriate separately reviewed browser setup; this controller does not bypass sites' browser-security warnings.

## Runtime and boundaries

Each session owns a separate Tor process, browser process and disposable profile. The browser and geckodriver run in a bubblewrap network namespace without direct internet or DNS access. They reach Tor through private Unix sockets. Tor itself runs outside that namespace. Startup checks direct TCP and DNS blocking before launching geckodriver.

The sandbox mounts the Nix store, its own session directory and minimal system identity files, not your home directory or other sessions. Headed mode additionally grants access to the selected local display. This is not a hostile-desktop or hostile-browser security guarantee.

The local daemon uses a same-user Unix socket. Session names coordinate cooperating clients; they do not authenticate individual agents or isolate hostile clients using the same OS account. Browser-state separation does not guarantee unlinkability. Avoid churning session names to rotate Tor guards.

All page text, links and images are untrusted. The client must authorize consequential interactions. A site's event handlers may send data even when a field is filled without a submit command. Do not use personal credentials or profiles for acceptance tests. Content sent to a hosted model is still disclosed to that model provider.

There are no public tools for arbitrary JavaScript evaluation, file upload, file download, raw Tor control, new identities or fingerprint-changing emulation. Browser download prompts are not acceptance to retrieve or open a file. Do not use interactive actions to circumvent those boundaries.

Runtime files live under `$XDG_RUNTIME_DIR/tor-mcp`, with a private temporary-directory fallback. Tor guard state lives under `$XDG_STATE_HOME/tor-mcp/tor`, defaulting to `~/.local/state/tor-mcp/tor`. Closing a session removes its browser profile, cookies and startup logs, but intentionally retains Tor state. Daemon diagnostics are in the private runtime directory's `daemon.log`.

| Setting | Default |
| --- | --- |
| `TOR_MCP_MAX_SESSIONS` | 4, maximum 32 |
| `TOR_MCP_IDLE_SECONDS` | 600; 0 disables headless idle cleanup |
| `TOR_MCP_BOOTSTRAP_SECONDS` | 180 |

Headed sessions remain until closed. `shutdown` waits for normal daemon cleanup. A forced daemon kill can leave private session directories behind; after checking that owned processes have stopped, remove stale directories or reopen and close the corresponding session. Profile deletion is not secure disk erasure. If rebuilding the development executable, shut down the old daemon before starting new sessions. Tor bootstrap and onion-service publication depend on live network availability; failures never enable a direct-network fallback.

## Develop and verify

```sh
nix develop
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
node --test tests/page.test.cjs
nix flake check
```

With direnv, use `direnv allow` and `direnv exec . <command>` instead. Unit tests, page-extraction regressions and the MCP discovery smoke test run without browsing. The Nix check runs the Rust and JavaScript tests. Live browsing acceptance tests are ignored by default because they need Tor, a harmless fixture, or a desktop display. The manual acceptance document includes fixture setup and the explicit commands to run them.
