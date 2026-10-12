# Manual acceptance tests

Use a disposable test session, not your personal Tor Browser profile. Use a small test website that you control, preferably served as a temporary onion service. It should have two pages with different titles, a labelled text field, a button that changes a message, a link to the second page, and an onion link. Do not submit real credentials or download unknown files.

Record each case as Passed, Failed, or Not tested, with a short explanation. A unit test passing does not count as a manual pass. Run these through an MCP client, not only through the CLI. Headed cases require a desktop display. Search cases need Tor connectivity and a reachable search provider.

## Run the fixture

These commands create a temporary public onion service containing only harmless test HTML. Keep the fixture running while checking commands. Use a spare local port if 8765 is already occupied. Run inside `nix develop`, or prefix commands with `direnv exec .`.

```sh
cargo build --example fixture
work=$(mktemp -d)
chmod 700 "$work"
cat > "$work/torrc" <<EOF
DataDirectory $work/tor-data
SocksPort unix:$work/fixture-socks.socket
HiddenServiceDir $work/onion-service
HiddenServicePort 80 127.0.0.1:8765
Log notice stdout
EOF
tor -f "$work/torrc" > "$work/tor.log" 2>&1 &
tor_pid=$!
while ! test -s "$work/onion-service/hostname"; do sleep 1; done
export TOR_MCP_FIXTURE_ONION=$(< "$work/onion-service/hostname")
target/debug/examples/fixture > "$work/fixture.log" 2>&1 &
fixture_pid=$!
export TOR_MCP_TEST_URL="http://$TOR_MCP_FIXTURE_ONION/one"
```

Check the private fixture log for Tor bootstrap completion. A hostname file or 100% bootstrap alone does not prove the onion descriptor has been published. Publication can take several minutes. Runtime tests should fail clearly if the fixture or Tor is unavailable; do not count such a run as a pass.

The following checks exercise the plain-language cases through a real MCP client:

```sh
cargo test --test mcp_acceptance mcp_discovery_is_lazy -- --ignored --nocapture
cargo test --test mcp_acceptance local_onion_command_acceptance -- --ignored --nocapture
# Requires a local display and reachable Ahmia:
cargo test --test mcp_acceptance headed_and_live_search_acceptance -- --ignored --nocapture
```

For repeated live runs, optionally set `TOR_MCP_TEST_STATE_HOME` to a dedicated absolute private directory, such as `$PWD/manual-results/client-state`. This retains only test Tor state between runs, avoiding unnecessary guard and directory-cache churn; disposable browser profiles still disappear. Do not point it at personal Tor state. Without this setting, tests use fresh temporary state.

These are representative scripted acceptance exercises, not a substitute for every manual edge case below. After checking results, stop only the fixture processes and remove their disposable state:

```sh
kill "$fixture_pid" "$tor_pid"
wait "$fixture_pid" "$tor_pid" 2>/dev/null || true
rm -r "$work"
```

## session_list

Before browsing, list sessions. The list should be empty, and neither Tor nor Tor Browser should start. After opening two named sessions, list them again. Both names should appear, with their headed/headless mode and containment status. One agent's session must not be reported as another's.

## browser_open

Open the first test page in a new headless session. See bootstrap progress on the server's diagnostic output, then receive the actual page URL and title. Open another page in the same session: it should reuse the browser. Try a file URL and an invalid onion address: both should fail before navigating. Open a fresh session with headed mode: a visible Tor Browser window should appear. Request a different mode for an existing session: get an explanation that a restart is needed, not silent state loss.

## browser_snapshot

Take a snapshot of the test page. Read the heading, field label, button and link, with usable references for interactive elements. Request an interactive-only snapshot and check that unnecessary prose is omitted. Password fields may have labels and references, but their values must not appear. Navigate to the second page and try an old reference: receive a stale-reference error, never an action on an unrelated element. References from another session must not work.

## browser_read

Read the current page. Receive its title, URL and readable text, including a message added by JavaScript. Scripts and styles should not appear as article text. Long content must be bounded and marked as truncated. Page text must be identified as untrusted content, not instructions for the agent.

## browser_links

List links on the test page. Each result should include link text and the complete resolved URL. Ask for onion links only and check that ordinary links are removed. Invalid onion addresses and non-web schemes should not be offered as navigation candidates. Links remain unverified candidates, not endorsed sites.

## browser_click

Take a fresh snapshot and click the test button by its reference. Verify the changed message with a new read or snapshot. Click the page link and verify the second page's title. Try an absent CSS selector: get a clear error, without clicking something else. Old references should stop working after an interaction invalidates the snapshot.

## browser_fill

Fill the labelled test field by reference or CSS selector. Check that its previous contents were replaced, not appended. Confirm that the page reports the new value. Filling a missing or non-editable element should fail. Filling a file input must report that uploads are disabled, without selecting a local file. The controller must not request a form submission merely because a field was filled. A real site's input event handlers can still send data, so only use the harmless fixture for this check.

## browser_press

Focus the test field, type with fill, and press Tab. Confirm focus moved as expected. Test Enter only on a harmless fixture that records the event. Unsupported key names should give a useful error. Do not use this test to approve a real form submission.

## browser_wait

Wait for the fixture's delayed success message and receive success once it appears. Wait for a message that never appears with a short timeout: receive a timeout error. Waiting must not block work in another session. An excessive timeout must be rejected rather than making the server wait indefinitely.

## browser_screenshot

Capture the current page. The MCP client should receive a viewable PNG showing the fixture, not just a path on the server. The screenshot must belong to the requested session. Taking a screenshot should not create an unsolicited download or copy of browser credentials.

## browser_tabs

List tabs, open a new tab on the second test page, select the original tab using its stable ID, and close the second tab. IDs must not change when another tab closes. Selecting an unknown ID should fail rather than selecting a neighbouring tab. A tab change should invalidate element references. Closing the last tab should explain how to open a new page, not act in another session.

## browser_navigate

Follow the fixture's link, go back, go forward, and reload. Check the title and URL after each step. Old references must fail after every navigation. An unsupported navigation action should fail without altering the page.

## onion_search

Search for a harmless topic such as independent journalism. Receive a bounded list of complete onion URLs, titles and the provider/source that supplied them. Search in two sessions at once and check that the result sets do not overwrite each other. An empty query or excessive limit should be rejected. If the provider is unavailable or its page format cannot be recognised, receive an explanation rather than fabricated results. Every provider request must go through Tor.

## onion_discover

Open an organisation's official HTTPS page that you control and that publishes an onion link, meta tag, or Onion-Location header. Discover its onion counterpart and see the exact source of the claim. Repeat on a page that publishes no onion address and receive an empty result. Reject malformed onion addresses. Do not report a site's identity as verified just because an address has the right shape.

## tor_status

Ask for status before opening a session. It should say the session is not running and must not start Tor. After opening a page, see bootstrap readiness and network-containment status. Onion browsing must not be described as using an exit node. If Tor stops, status should report that failure and browsing must not fall back to the direct internet. Status must not reveal control authentication secrets.

## session_close

Close one of two active sessions. Its browser, geckodriver, Tor process and disposable browser profile should disappear. The other session must still work. Closing an already closed session should be harmless and should not start anything. Close all test sessions at the end; retained Tor guard state is intentional and must not contain browser cookies.

## Cross-command security and lifecycle checks

- Start MCP, initialise it and list tools without calling a browsing command. No browser or Tor process should start.
- Open two sessions concurrently, fill different values and move between different pages. Neither agent should see or change the other's browser state.
- Inside the browser's network namespace, try a direct connection to a public IP and a direct DNS query. Both should be blocked. Normal browsing through the managed Tor Unix socket should still work.
- Stop Tor while leaving the browser running. A fresh external request should fail, never go directly to the destination. Restore by explicitly closing and reopening the session.
- Make network namespaces unavailable in a disposable environment. Launch must fail with an actionable containment error; there is no insecure fallback switch.
- Wait beyond a short configured headless idle timeout. Its processes and profile should be removed. The next open should create a fresh browser. A headed session should remain until explicitly closed.
- Shut down the daemon normally and interrupt it during startup. No owned browser or Tor process should remain. Do not kill unrelated Tor Browser instances.
- Inspect the sandbox mounts. The browser must not have access to your home directory, SSH keys, personal browser profile or unrelated session data. Headed display access is an explicit exception and is not a hostile-desktop security boundary.

## Acceptance result

The first milestone is successful only when the Rust unit tests and Nix checks pass, all exposed commands have the acceptance descriptions above, and the representative local-onion workflow establishes lazy startup, core interactions, two-session isolation, Tor-only network access and cleanup. Record headed and live-search checks separately when they cannot be exercised. Do not claim complete acceptance while a required safety case fails or remains untested.
