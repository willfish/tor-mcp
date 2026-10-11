//! Harmless local website for the manual acceptance cases. Serve it behind a
//! temporary onion service; never put real credentials or private content here.
use anyhow::Result;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[tokio::main]
async fn main() -> Result<()> {
    let port = std::env::var("ONION_BROWSER_FIXTURE_PORT").unwrap_or_else(|_| "8765".into());
    let onion = std::env::var("ONION_BROWSER_FIXTURE_ONION")?;
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    println!("Fixture listening on 127.0.0.1:{port}");
    loop {
        let (mut stream, _) = listener.accept().await?;
        let onion = onion.clone();
        tokio::spawn(async move {
            let mut request = [0; 8192];
            let size = stream.read(&mut request).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..size]);
            let path = request.split_whitespace().nth(1).unwrap_or("/");
            let second = path.starts_with("/two");
            let title = if second { "Fixture Two" } else { "Fixture One" };
            let body = format!(
                r#"<!doctype html><html><head><title>{title}</title><meta http-equiv="Onion-Location" content="http://{onion}/one"></head><body>
<h1>{title}</h1><p>A harmless test page.</p><label for="name">Your test name</label><input id="name" value="original">
<label for="password">Test password</label><input id="password" type="password" value="not-for-snapshots">
<label for="upload">Test file field</label><input id="upload" type="file">
<button id="change" onclick="document.getElementById('message').textContent='Hello '+document.getElementById('name').value">Change message</button>
<p id="message">Not changed</p><p id="delayed"></p><a id="next" href="/two">Second page</a>
<a href="http://{onion}/one">Published onion link</a><a href="https://example.com/">Ordinary link</a>
<script>setTimeout(()=>document.getElementById('delayed').textContent='Delayed ready',300); document.getElementById('name').addEventListener('keydown', e=>{{if(e.key==='Enter') document.getElementById('message').textContent='Enter recorded';}});</script>
</body></html>"#
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nOnion-Location: http://{onion}/one\r\nConnection: close\r\n\r\n{}",
                body.len(),
                if request.starts_with("HEAD ") {
                    ""
                } else {
                    &body
                }
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}
