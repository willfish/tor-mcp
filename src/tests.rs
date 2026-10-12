use crate::{
    browser::{filter_links, search_results},
    mcp,
    protocol::*,
    runtime::{Config, private_dir, sandbox_args},
};
use serde_json::{Value, json};
use std::path::PathBuf;

const ONION: &str = "2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid.onion";
fn config(root: PathBuf) -> Config {
    Config {
        state: root.join("state"),
        root,
        browser: "/nix/store/test/bin/tor-browser".into(),
        bundle: "/nix/store/test/share/tor-browser".into(),
        geckodriver: "/nix/store/test/bin/geckodriver".into(),
        tor: "/nix/store/test/bin/tor".into(),
        bwrap: "/nix/store/test/bin/bwrap".into(),
        max_sessions: 4,
        idle_seconds: 600,
        bootstrap_seconds: 180,
    }
}
#[test]
fn valid_session_names() {
    for name in ["default", "agent-1", "agent_2", "A123"] {
        validate_session(name).unwrap();
    }
}
#[test]
fn reject_path_traversal_session_names() {
    for name in [
        "",
        ".",
        "..",
        "a/b",
        "../../profile",
        "a b",
        "a\n",
        "café",
        "name.socket",
    ] {
        assert!(validate_session(name).is_err(), "{name}");
    }
}
#[test]
fn reject_long_session_names() {
    assert!(validate_session(&"a".repeat(33)).is_err());
}
#[test]
fn recognize_valid_v3_onion() {
    assert!(valid_onion(ONION));
    assert!(valid_onion(&format!("www.{ONION}")));
}
#[test]
fn reject_v2_onions() {
    assert!(!valid_onion("abcdefghijklmnop.onion"));
}
#[test]
fn reject_forged_v3_checksum() {
    assert!(!valid_onion(&format!("{}.onion", "a".repeat(56))));
    let changed = ONION.replacen('2', "3", 1);
    assert!(!valid_onion(&changed));
}
#[test]
fn accept_complete_web_urls() {
    for url in [
        "https://example.com/path?q=test",
        "http://example.com",
        &format!("http://{ONION}"),
    ] {
        validate_url(url).unwrap();
    }
}
#[test]
fn reject_non_web_schemes() {
    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:text/html,hello",
        "ftp://example.com",
        "about:config",
        "example.com",
    ] {
        assert!(validate_url(url).is_err(), "{url}");
    }
}
#[test]
fn reject_embedded_credentials() {
    assert!(validate_url("https://user:secret@example.com").is_err());
    assert!(validate_url("https://user@example.com").is_err());
}
#[test]
fn reject_malformed_onion_url() {
    assert!(validate_url("http://fake.onion").is_err());
}
#[test]
fn search_requires_nonempty_query() {
    assert!(decode_action("onion_search", json!({"session":"a", "query":"  "})).is_err());
}
#[test]
fn search_limits_are_bounded() {
    for limit in [0, 21] {
        assert!(
            decode_action(
                "onion_search",
                json!({"session":"a", "query":"news", "limit":limit})
            )
            .is_err()
        );
    }
}
#[test]
fn wait_limits_are_bounded() {
    for timeout in [0, 60_001] {
        assert!(
            decode_action(
                "browser_wait",
                json!({"session":"a", "text":"ready", "timeout_ms":timeout})
            )
            .is_err()
        );
    }
}
#[test]
fn wait_requires_text() {
    assert!(decode_action("browser_wait", json!({"session":"a", "text":""})).is_err());
}
#[test]
fn known_keys_have_webdriver_codes() {
    assert_eq!(key_code("Enter").unwrap(), "\u{e007}");
    assert_eq!(key_code("Tab").unwrap(), "\u{e004}");
    assert!(key_code("run shell").is_err());
}
#[test]
fn tab_selection_requires_stable_id() {
    assert!(decode_action("browser_tabs", json!({"session":"a", "action":"select"})).is_err());
    assert!(
        decode_action(
            "browser_tabs",
            json!({"session":"a", "action":"select", "tab":"t1"})
        )
        .is_ok()
    );
}
#[test]
fn reject_unknown_actions_and_arguments() {
    assert!(decode_action("execute_shell", json!({})).is_err());
    assert!(
        decode_action(
            "browser_open",
            json!({"session":"a", "url":"https://example.com", "disable_sandbox":true})
        )
        .is_err()
    );
    assert!(
        decode_action(
            "browser_navigate",
            json!({"session":"a", "action":"delete"})
        )
        .is_err()
    );
}
#[test]
fn browsing_arguments_require_explicit_session() {
    assert!(decode_action("browser_open", json!({"url":"https://example.com"})).is_err());
}
#[test]
fn defaults_are_safe() {
    let Action::BrowserOpen(open) = decode_action(
        "browser_open",
        json!({"session":"a", "url":"https://example.com"}),
    )
    .unwrap() else {
        panic!()
    };
    assert!(!open.headed);
    let Action::OnionSearch(search) =
        decode_action("onion_search", json!({"session":"a", "query":"news"})).unwrap()
    else {
        panic!()
    };
    assert_eq!(search.limit, 5);
}
#[test]
fn links_filter_schemes_and_bad_onions() {
    let links = json!([{"url":"https://example.com"}, {"url":format!("http://{ONION}")}, {"url":"javascript:alert(1)"}, {"url":"http://fake.onion"}]);
    assert_eq!(filter_links(links.clone(), false).len(), 2);
    assert_eq!(filter_links(links, true).len(), 1);
}
#[test]
fn search_rejects_homepage_footer_as_results() {
    let page = json!({"recognized":false,"results":[{"url":format!("http://{ONION}")}]});
    assert!(search_results(page, "https://ahmia.fi/search/?q=news", 5).is_err());
}
#[test]
fn search_accepts_only_recognized_empty_pages() {
    let page = json!({"recognized":true,"no_results":true,"results":[]});
    assert!(
        search_results(page, "https://ahmia.fi/search/?q=news", 5)
            .unwrap()
            .is_empty()
    );
    let page =
        json!({"recognized":true,"no_results":false,"results":[{"url":"http://fake.onion"}]});
    assert!(search_results(page, "https://ahmia.fi/search/?q=news", 5).is_err());
}
#[test]
fn search_preserves_provenance_and_bounds_unique_results() {
    let link = json!({"url":format!("http://{ONION}"),"text":"A title"});
    let page = json!({"recognized":true,"results":[link.clone(),link]});
    let source = "https://ahmia.fi/search/?q=news";
    let results = search_results(page, source, 1).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["source"], source);
    assert_eq!(results[0]["text"], "A title");
}
#[test]
fn tools_have_manual_acceptance_cases() {
    let docs = include_str!("../docs/manual-acceptance.md");
    for tool in tools() {
        assert!(
            docs.contains(&format!("## {}\n", tool.name)),
            "missing manual acceptance case: {}",
            tool.name
        );
    }
}
#[test]
fn tool_names_are_unique_and_schemas_are_closed() {
    let mut names = std::collections::HashSet::new();
    for tool in tools() {
        assert!(names.insert(tool.name));
        assert_eq!(tool.schema["type"], "object");
        assert_eq!(tool.schema["additionalProperties"], false);
    }
    assert_eq!(names.len(), 16);
}
#[test]
fn mcp_definitions_have_safety_annotations() {
    let tools = mcp::definitions();
    let click = tools.iter().find(|t| t.name == "browser_click").unwrap();
    assert_eq!(
        click.annotations.as_ref().unwrap().read_only_hint,
        Some(false)
    );
    let read = tools.iter().find(|t| t.name == "browser_read").unwrap();
    assert_eq!(
        read.annotations.as_ref().unwrap().read_only_hint,
        Some(true)
    );
}
#[test]
fn runtime_directories_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("runtime");
    private_dir(&dir).unwrap();
    assert_eq!(
        std::fs::metadata(dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
#[test]
fn runtime_rejects_symlink_directory() {
    let temp = tempfile::tempdir().unwrap();
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(temp.path(), &link).unwrap();
    assert!(private_dir(&link).is_err());
}
#[test]
fn runtime_rejects_symlink_parent() {
    let temp = tempfile::tempdir().unwrap();
    let link = temp.path().join("runtime-parent");
    std::os::unix::fs::symlink(temp.path(), &link).unwrap();
    let config = config(link.join("runtime"));
    assert!(config.prepare().is_err());
}
#[test]
fn runtime_path_budget_includes_the_longest_session_socket() {
    let temp = tempfile::tempdir().unwrap();
    let name = "s".repeat(32);
    let suffix = PathBuf::from(&name).join("webdriver.socket");
    let max_root = 107 - suffix.as_os_str().len() - 1;
    let padding = max_root - temp.path().as_os_str().len() - 1;
    let valid = config(temp.path().join("r".repeat(padding)));
    valid.prepare().unwrap();
    let session = valid.root.join(&name);
    private_dir(&session).unwrap();
    let socket = session.join("webdriver.socket");
    assert_eq!(socket.as_os_str().len(), 107);
    let _listener = std::os::unix::net::UnixListener::bind(socket).unwrap();

    let invalid = config(temp.path().join("r".repeat(padding + 1)));
    let error = invalid.prepare().unwrap_err();
    assert!(error.to_string().starts_with("runtime_path_too_long:"));
}
#[test]
fn session_capacity_is_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = config(temp.path().join("runtime"));
    config.max_sessions = 0;
    assert!(config.prepare().is_err());
    config.max_sessions = 33;
    assert!(config.prepare().is_err());
}
#[test]
fn sandbox_has_no_host_network_or_home_mount() {
    let temp = tempfile::tempdir().unwrap();
    let config = config(temp.path().join("runtime"));
    let args = sandbox_args(&config, temp.path(), false).unwrap();
    assert!(args.contains(&"--unshare-all".into()));
    assert!(args.contains(&"--die-with-parent".into()));
    assert!(args.contains(&"--clearenv".into()));
    assert!(!args.contains(&"--share-net".into()));
    assert!(!args.windows(3).any(|a| a == ["--ro-bind", "/", "/"]));
    assert!(!args.contains(&std::env::var("HOME").unwrap()));
    assert!(args.contains(&"TOR_SOCKS_IPC_PATH".into()));
}
#[test]
fn public_schema_does_not_expose_unsafe_controls() {
    let names = tools().into_iter().map(|t| t.name).collect::<Vec<_>>();
    for forbidden in [
        "eval",
        "browser_eval",
        "upload",
        "download",
        "tor_control",
        "tor_new_identity",
    ] {
        assert!(!names.contains(&forbidden));
    }
}
#[test]
fn typed_action_roundtrip() {
    let action = decode_action(
        "browser_fill",
        json!({"session":"a", "target":"#field", "text":"hello"}),
    )
    .unwrap();
    let value = serde_json::to_value(&action).unwrap();
    let roundtrip: Action = serde_json::from_value(value).unwrap();
    assert_eq!(roundtrip.session(), Some("a"));
}
#[test]
fn discovery_limits_are_bounded_and_defaulted() {
    let Action::OnionDiscover(args) = decode_action(
        "onion_discover",
        json!({"session":"a", "url":"https://example.com"}),
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(args.limit, 20);
    for limit in [0, 101] {
        assert!(
            decode_action(
                "onion_discover",
                json!({"session":"a", "url":"https://example.com", "limit":limit})
            )
            .is_err()
        );
    }
    assert!(
        decode_action(
            "onion_discover",
            json!({"session":"a", "url":"https://example.com", "limit":100})
        )
        .is_ok()
    );
}
#[test]
fn mcp_errors_include_codes_and_safe_recovery_advice() {
    for (code, tool, advice) in [
        ("navigation_failed", "browser_open", "Do not mark"),
        ("navigation_unconfirmed", "browser_read", "Do not mark"),
        ("interaction_failed", "browser_click", "Do not repeat"),
        ("stale_reference", "browser_click", "fresh browser_snapshot"),
        ("session_not_running", "browser_read", "browser_open"),
        ("session_capacity", "browser_open", "that you own"),
        (
            "tor_bootstrap_timeout",
            "browser_open",
            "never use a direct-network fallback",
        ),
    ] {
        let payload = mcp::error_payload(tool, &anyhow::anyhow!("{code}: test failure"));
        assert_eq!(payload["error"]["code"], code);
        assert_eq!(payload["tool"], tool);
        assert!(
            payload["error"]["next_action"]
                .as_str()
                .unwrap()
                .contains(advice)
        );
        assert_eq!(payload["page_content_is_untrusted"], true);
    }
    let payload = mcp::error_payload("browser_press", &anyhow::anyhow!("unclassified failure"));
    assert!(
        payload["error"]["next_action"]
            .as_str()
            .unwrap()
            .contains("Do not automatically repeat")
    );
}
#[test]
fn argument_errors_have_a_stable_code() {
    let error = decode_action("browser_open", json!({"url":"https://example.com"})).unwrap_err();
    assert_eq!(
        mcp::error_payload("browser_open", &error)["error"]["code"],
        "invalid_arguments"
    );
}
#[test]
fn every_tool_schema_is_json_serializable() {
    for tool in mcp::definitions() {
        let value: Value = serde_json::to_value(tool).unwrap();
        assert!(value["inputSchema"].is_object());
    }
}
