# onion-browser

A Linux-first Rust CLI and MCP server for automating actual Tor Browser. It starts Tor and the browser only when needed, keeps named sessions independent, and supports headless or headed browsing.

This is an initial implementation, not complete agent-browser parity or a promise of anonymous automation. Websites can detect automation.

## Install

Requires x86-64 Linux, Nix, and permission to create unprivileged user namespaces with bubblewrap. Headed mode also requires a local Wayland or X11 display. Unsupported containment fails closed.

```sh
# This repository is private. GitHub SSH access is required.
nix profile install 'git+ssh://git@github.com/willfish/onion-browser?ref=master'

# Alternatively, from a checkout:
nix run . -- --help
```

The pinned flake includes Tor Browser, Tor, geckodriver and bubblewrap. It does not download a browser on first launch. Update the pin promptly for browser security releases, then rerun acceptance checks before distributing the update.

## CLI

```sh
onion-browser --session research open https://example.com/
onion-browser --session research snapshot --interactive
onion-browser --session research click '<reference from the snapshot>'
onion-browser --session research read
onion-browser --session research links --onion-only
onion-browser --session research screenshot page.png
onion-browser --session research search 'independent journalism' --limit 5
onion-browser --session research discover https://example.com/
onion-browser --session research tor-status
onion-browser --session research close

onion-browser --session review open https://example.com/ --headed
onion-browser sessions
onion-browser shutdown
```

Use `--json` for compact JSON. Without it, output is pretty-printed JSON. `fill`, `press`, `wait`, `tabs` and `navigate` provide the other core interactions. Run `tools` for all MCP names and their argument schemas, or `call <tool-name> '<JSON arguments>'` to invoke one directly.

References belong to one session and one snapshot. Take a new snapshot after an interaction or navigation. CSS selectors are also accepted. Snapshots describe bounded DOM semantics, not Firefox's complete native accessibility tree. Password values are omitted from snapshots.

Screenshots are viewport PNGs. The CLI writes only to a new destination file; MCP returns image content. Display mode is fixed when a session is created. Close and reopen to change it.

## MCP

Install the executable, then configure your MCP client:

```json
{
  "mcpServers": {
    "onion-browser": {
      "command": "onion-browser",
      "args": ["mcp"]
    }
  }
}
```

Every browsing tool requires an explicit session name. Give each agent its own name. Initialization and tool listing start neither the daemon nor Tor Browser. Listing sessions or checking an unopened session may start the lightweight daemon, but not Tor or the browser.

The [manual acceptance cases](docs/manual-acceptance.md) describe every exposed tool. Commands in one session are serialized; separate sessions can run concurrently. Ahmia search and publisher discovery navigate the active tab of their session. Search results and publisher claims include provenance, not verified organisational identities. Search uses Ahmia's normal form fields and recognizes its results page instead of treating arbitrary onion links as search results.

## Runtime and boundaries

Each session owns a separate Tor process, browser process and disposable profile. The browser and geckodriver run in a bubblewrap network namespace without direct internet or DNS access. They reach Tor through private Unix sockets. Tor itself runs outside that namespace. Startup checks direct TCP and DNS blocking before launching geckodriver.

The sandbox mounts the Nix store, its own session directory and minimal system identity files, not your home directory or other sessions. Headed mode additionally grants access to the selected local display. This is not a hostile-desktop or hostile-browser security guarantee.

The local daemon uses a same-user Unix socket. Session names coordinate cooperating clients; they do not authenticate individual agents or isolate hostile clients using the same OS account. Browser-state separation does not guarantee unlinkability. Avoid churning session names to rotate Tor guards.

All page text, links and images are untrusted. The client must authorize consequential interactions. A site's event handlers may send data even when a field is filled without a submit command. Do not use personal credentials or profiles for acceptance tests. Content sent to a hosted model is still disclosed to that model provider.

There are no public tools for arbitrary JavaScript evaluation, file upload, file download, raw Tor control, new identities or fingerprint-changing emulation. Browser download prompts are not acceptance to retrieve or open a file. Do not use interactive actions to circumvent those boundaries.

Runtime files live under `$XDG_RUNTIME_DIR/onion-browser`, with a private temporary-directory fallback. Tor guard state lives under `$XDG_STATE_HOME/onion-browser/tor`, defaulting to `~/.local/state/onion-browser/tor`. Closing a session removes its browser profile, cookies and startup logs, but intentionally retains Tor state. Daemon diagnostics are in the private runtime directory's `daemon.log`.

| Setting | Default |
| --- | --- |
| `ONION_BROWSER_MAX_SESSIONS` | 4, maximum 32 |
| `ONION_BROWSER_IDLE_SECONDS` | 600; 0 disables headless idle cleanup |
| `ONION_BROWSER_BOOTSTRAP_SECONDS` | 180 |

Headed sessions remain until closed. `shutdown` waits for normal daemon cleanup. A forced daemon kill can leave private session directories behind; after checking that owned processes have stopped, remove stale directories or reopen and close the corresponding session. Profile deletion is not secure disk erasure. If rebuilding the development executable, shut down the old daemon before starting new sessions. Tor bootstrap and onion-service publication depend on live network availability; failures never enable a direct-network fallback.

## Develop and verify

```sh
nix develop
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
nix flake check
```

With direnv, use `direnv allow` and `direnv exec . <command>` instead. Unit tests run without browsing. Runtime acceptance tests are ignored by default because they need live Tor, a harmless fixture, or a desktop display. The manual acceptance document includes fixture setup and the explicit commands to run them.
