use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[tokio::main]
async fn main() {
    let port = std::env::args()
        .nth(1)
        .expect("usage: cargo run -p chronicle-application --example http_test_server -- <port>")
        .parse::<u16>()
        .expect("port must be a non-zero u16");
    assert_ne!(port, 0, "port must be non-zero");
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
        .await
        .expect("bind loopback test server");
    println!("listening on http://127.0.0.1:{port}");
    loop {
        let (stream, _) = listener.accept().await.expect("accept test connection");
        tokio::spawn(async move {
            let _ = respond(stream).await;
        });
    }
}

async fn respond(mut stream: TcpStream) -> std::io::Result<()> {
    let mut buffer = [0_u8; 1024];
    let _ = stream.read(&mut buffer).await?;
    stream
        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nx-fixture: basic\r\nconnection: close\r\n\r\nOK")
        .await
}
