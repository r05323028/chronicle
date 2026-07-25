use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedRequest {
    pub method: String,
    pub target: String,
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseMode {
    Pass,
    Mismatch,
}

pub struct HttpTestServer {
    address: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    task: JoinHandle<()>,
}

impl HttpTestServer {
    pub async fn spawn(mode: ResponseMode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener must bind");
        let address = listener.local_addr().expect("test listener has address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let recorded = Arc::clone(&recorded);
                tokio::spawn(async move {
                    if let Some((stream, request)) = read_request(stream).await {
                        recorded
                            .lock()
                            .expect("request lock poisoned")
                            .push(request.clone());
                        let _ = write_response(stream, request, mode).await;
                    }
                });
            }
        });
        Self {
            address,
            requests,
            task,
        }
    }

    pub fn origin(&self) -> String {
        format!("http://{}", self.address)
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("request lock poisoned").clone()
    }
}

impl Drop for HttpTestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(mut stream: TcpStream) -> Option<(TcpStream, RecordedRequest)> {
    let mut bytes = Vec::new();
    let head_len = loop {
        if let Some(head_len) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break head_len + 4;
        }
        let mut buffer = [0_u8; 1024];
        let count = stream.read(&mut buffer).await.ok()?;
        if count == 0 || bytes.len() > 64 * 1024 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
    };
    let head = std::str::from_utf8(&bytes[..head_len]).ok()?;
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_owned();
    let target = request_line.next()?.to_owned();
    let body_len = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < head_len + body_len {
        let mut buffer = [0_u8; 1024];
        let count = stream.read(&mut buffer).await.ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Some((
        stream,
        RecordedRequest {
            method,
            target,
            body: bytes[head_len..head_len + body_len].to_vec(),
        },
    ))
}

async fn write_response(
    mut stream: TcpStream,
    request: RecordedRequest,
    mode: ResponseMode,
) -> std::io::Result<()> {
    let (status, body) = match mode {
        ResponseMode::Mismatch => ("500 Mismatch", b"mismatch".to_vec()),
        ResponseMode::Pass => match (request.method.as_str(), request.target.as_str()) {
            ("GET", "/get") => ("200 OK", b"get".to_vec()),
            ("GET", "/hello") => ("200 OK", b"OK".to_vec()),
            ("GET", "/binary") => ("200 OK", vec![0, 0xff, 1]),
            ("GET", "/non-2xx") => ("418 I'm a teapot", b"teapot".to_vec()),
            ("POST", "/post") => ("201 Created", request.body),
            _ => ("404 Not Found", b"not found".to_vec()),
        },
    };
    stream
        .write_all(
            format!(
                "HTTP/1.1 {status}\r\nx-fixture: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                if mode == ResponseMode::Pass { "basic" } else { "mismatch" },
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(&body).await
}
