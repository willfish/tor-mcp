# Initial acceptance assessment

Date: 2026-10-11. Environment: x86-64 Linux, the pinned Nix development environment, Tor Browser 15.0.24, and a harmless temporary onion service. Headed browsing used Wayland.

**Assessment: the bounded initial implementation passes its representative acceptance workflow. This is not complete acceptance of every manual edge case or a security audit.**

The plain-language expectations are in [manual-acceptance.md](manual-acceptance.md). The following results came from real MCP calls in the runnable acceptance client, not inferred from unit tests.

| MCP command | Representative result |
| --- | --- |
| `session_list` | Passed: empty before browsing; both independent sessions listed after launch. |
| `browser_open` | Passed: actual onion titles, headless and headed launch, and rejection of changing an existing session's display mode. |
| `browser_snapshot` | Passed: labelled interactive references; cross-session and invalidated references rejected; password values omitted. |
| `browser_read` | Passed: JavaScript-generated messages and separate Alice/Bob state read from the right sessions. |
| `browser_links` | Passed: fixture onion links returned with ordinary links excluded when requested. |
| `browser_click` | Passed: fixture button and page link worked; an absent selector failed. |
| `browser_fill` | Passed: existing field contents replaced independently by reference and selector; file attachment attempts rejected. |
| `browser_press` | Passed: Enter reached the harmless fixture's event handler. |
| `browser_wait` | Passed: expected text found; a short wait for absent text returned an error. |
| `browser_screenshot` | Passed: MCP image content decoded to a PNG. |
| `browser_tabs` | Passed: list, create, select by stable ID, close, and reject an unknown ID. |
| `browser_navigate` | Passed: back, forward and reload returned the expected fixture titles. |
| `onion_search` | Passed: concurrent live Ahmia searches returned separate query-specific results with bounded URLs and provenance. |
| `onion_discover` | Passed: the controlled onion publisher's Onion-Location header was returned as an unverified claim. |
| `tor_status` | Passed: unopened sessions stayed unopened; live bootstrap and blocked TCP/DNS probes reported; stopped Tor reported dead. |
| `session_close` | Passed: owned processes and profile removed; the other session still worked; closing an unopened session was harmless. |

## Safety and lifecycle evidence

- MCP initialization and tool listing did not even create a daemon runtime directory.
- Concurrent sessions reported different browser network namespaces and maintained independent page state.
- Direct TCP and DNS probes failed inside the browser namespace while actual Tor browsing worked.
- A fresh navigation after stopping the session's owned Tor process failed instead of reaching the destination directly.
- Disabling nested user namespaces for the sandbox caused `containment_unavailable`; the failed session profile was removed. No insecure fallback was used.
- Short headless idle expiry removed the browser, Tor process and profile. Reopening created a new browser. Headed sessions survived their idle deadline.
- Normal shutdown waited for owned-process cleanup. Interrupting a foreground daemon during Tor startup removed its owned startup child and incomplete session directory.
- The sandbox mount construction was checked by unit tests and source review. A live mount-table inspection of the packaged headless runtime confirmed a private root, Nix store, own session, controller executable, minimal identity files and kernel/device mounts, without a host-home or other-session mount.

## Automated checks

The Rust unit suite passed all 34 tests. Rust formatting, Clippy with warnings treated as errors, Nix formatting, and `nix flake check` passed. Nix built the wrapped release executable and ran the unit suite offline. Runtime acceptance is intentionally excluded from the default offline checks and was invoked explicitly. Final live reruns used dedicated retained test Tor state, seeded from this project's earlier harmless smoke-test state; browser profiles and page data were not retained. Fresh-state runs also exposed bootstrap timeouts, so warm-state passes are not evidence of reliable first-run bootstrap.

## Remaining limits

Not every subcase in the manual document was exercised: examples include oversized rendered text, Tab focus movement, non-editable fill targets, closing the final tab, every stale-reference/navigation combination, and an official HTTPS publisher with an Onion-Location header. PNG structure and MCP delivery were checked, and a packaged CLI screenshot of a public example page was viewed. Visual fidelity was not reviewed pixel by pixel. Headed acceptance covered Wayland, not X11. Forced SIGKILL and secure disk erasure were not tested.

Live Tor runs intermittently failed at bootstrap or navigation deadlines before successful reruns. These successes establish compatibility and the representative workflow, not a network-reliability guarantee. Public discovery does not establish a site's organisational identity, and browser-state isolation does not establish unlinkability.
