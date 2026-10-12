use super::*;
use std::process::Stdio;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
    process::Command,
};

// Exercise the real WebDriver transport and session state without Tor or a browser.
async fn session(
    replies: Vec<(&'static str, Value)>,
) -> (tempfile::TempDir, Session, tokio::task::JoinHandle<()>) {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("session");
    std::fs::create_dir(&dir).unwrap();
    let socket = dir.join("webdriver.socket");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = tokio::spawn(async move {
        for (expected, value) in replies {
            let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            let (method, endpoint) = expected.split_once(' ').unwrap();
            assert_eq!(
                line,
                format!("{method} /session/test/{endpoint} HTTP/1.1\r\n")
            );
            let mut length = 0;
            loop {
                line.clear();
                stream.read_line(&mut line).await.unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            stream.read_exact(&mut vec![0; length]).await.unwrap();
            let body = json!({"value": value}).to_string();
            stream
                .write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes(),
                )
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    let child = || {
        Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    };
    let runtime = Runtime {
        dir,
        headed: false,
        tor: child(),
        sandbox: child(),
        webdriver: reqwest::Client::builder()
            .no_proxy()
            .unix_socket(socket)
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap(),
        webdriver_id: "test".into(),
        containment: Value::Null,
    };
    (temp, Session::new(runtime), server)
}

#[tokio::test]
async fn interaction_failures_block_page_tools_until_recovery() {
    for tool in ["browser_click", "browser_fill", "browser_press"] {
        let mut replies = Vec::new();
        if tool != "browser_press" {
            replies.push(("POST element", json!({ELEMENT_KEY: "field"})));
            replies.push(("POST execute/sync", Value::Null));
        }
        replies.push(("GET window", json!("tab-one")));
        replies.push((
            match tool {
                "browser_click" => "POST element/field/click",
                "browser_fill" => "POST execute/sync",
                _ => "POST actions",
            },
            json!({"error": "timeout", "message": "navigation did not finish"}),
        ));
        if tool == "browser_press" {
            replies.push(("DELETE actions", Value::Null));
        }
        // A read checks the guard, never running the page extraction script.
        replies.push(("GET window", json!("tab-one")));
        // Another tab remains readable without clearing the original guard.
        replies.push(("GET window", json!("tab-two")));
        replies.push(("POST execute/sync", json!({"text": "other tab"})));
        replies.push(("GET window", json!("tab-one")));
        replies.push(("POST url", Value::Null));
        replies.push(("GET url", json!("https://example.com/")));
        replies.push(("GET title", json!("Recovered")));
        replies.push(("POST execute/sync", json!({"text": "recovered page"})));
        let (_temp, mut session, server) = session(replies).await;
        let args = match tool {
            "browser_click" => json!({"session":"test", "target":"#field"}),
            "browser_fill" => json!({"session":"test", "target":"#field", "text":"value"}),
            _ => json!({"session":"test", "key":"Enter"}),
        };
        let error = session
            .execute(decode_action(tool, args).unwrap())
            .await
            .unwrap_err();
        assert!(
            error.to_string().starts_with("interaction_failed:"),
            "{tool}: {error:#}"
        );
        let read = || decode_action("browser_read", json!({"session":"test"})).unwrap();
        let error = session.execute(read()).await.unwrap_err();
        assert!(error.to_string().starts_with("navigation_unconfirmed:"));
        assert_eq!(session.execute(read()).await.unwrap()["text"], "other tab");
        assert!(session.failed_pages.contains_key("tab-one"));
        session.open("https://example.com/").await.unwrap();
        assert!(session.failed_pages.is_empty());
        assert_eq!(
            session.execute(read()).await.unwrap()["text"],
            "recovered page"
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn rejected_fill_preflight_does_not_block_the_page() {
    let (_temp, mut session, server) = session(vec![
        ("POST element", json!({ELEMENT_KEY: "upload"})),
        (
            "POST execute/sync",
            json!({"error":"javascript error", "message":"upload_disabled"}),
        ),
        ("POST execute/sync", json!({"text":"unchanged page"})),
    ])
    .await;
    let action = decode_action(
        "browser_fill",
        json!({"session":"test", "target":"#upload", "text":"unused"}),
    )
    .unwrap();
    assert!(session.execute(action).await.is_err());
    assert!(session.failed_pages.is_empty());
    let action = decode_action("browser_read", json!({"session":"test"})).unwrap();
    assert_eq!(
        session.execute(action).await.unwrap()["text"],
        "unchanged page"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn successful_interaction_clears_the_pending_guard() {
    let (_temp, mut session, server) = session(vec![
        ("GET window", json!("tab-one")),
        ("POST actions", Value::Null),
        ("DELETE actions", Value::Null),
    ])
    .await;
    let action = decode_action("browser_press", json!({"session":"test", "key":"Tab"})).unwrap();
    assert_eq!(session.execute(action).await.unwrap()["pressed"], "Tab");
    assert!(session.failed_pages.is_empty());
    server.await.unwrap();
}
