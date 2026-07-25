mod support;

use support::{HttpTestServer, ResponseMode};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{Duration, sleep},
};

async fn request(origin: &str, wire: &[u8]) -> Vec<u8> {
    let address = origin.strip_prefix("http://").expect("test origin is HTTP");
    let mut stream = TcpStream::connect(address)
        .await
        .expect("connect test server");
    stream.write_all(wire).await.expect("write request");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");
    response
}

async fn wait_for_requests(server: &HttpTestServer, count: usize) {
    for _ in 0..20 {
        if server.requests().len() == count {
            return;
        }
        sleep(Duration::from_millis(5)).await;
    }
    panic!("server did not receive {count} requests");
}

#[tokio::test]
async fn loopback_server_returns_deterministic_routes_and_records_requests() {
    let server = HttpTestServer::spawn(ResponseMode::Pass).await;
    let get = request(
        &server.origin(),
        b"GET /get HTTP/1.1\r\ncontent-length: 0\r\n\r\n",
    )
    .await;
    let post = request(
        &server.origin(),
        b"POST /post HTTP/1.1\r\ncontent-length: 3\r\n\r\none",
    )
    .await;
    let binary = request(
        &server.origin(),
        b"GET /binary HTTP/1.1\r\ncontent-length: 0\r\n\r\n",
    )
    .await;
    let non_2xx = request(
        &server.origin(),
        b"GET /non-2xx HTTP/1.1\r\ncontent-length: 0\r\n\r\n",
    )
    .await;

    assert!(get.ends_with(b"get"));
    assert!(post.ends_with(b"one"));
    assert!(binary.ends_with(&[0, 0xff, 1]));
    assert!(non_2xx.starts_with(b"HTTP/1.1 418"));
    wait_for_requests(&server, 4).await;
    assert_eq!(server.requests()[1].body, b"one");
}

#[tokio::test]
async fn loopback_server_mismatch_mode_is_deterministic() {
    let server = HttpTestServer::spawn(ResponseMode::Mismatch).await;
    let response = request(
        &server.origin(),
        b"GET /get HTTP/1.1\r\ncontent-length: 0\r\n\r\n",
    )
    .await;
    assert!(response.starts_with(b"HTTP/1.1 500 Mismatch"));
    assert!(response.ends_with(b"mismatch"));
}
