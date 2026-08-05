//! Minimal HTTP/1.1, both directions. The server backs the single-port API;
//! the client talks to sensor firmware and Kasa devices. Connection-per-
//! request, Content-Length only (no chunked encoding — none of the devices
//! use it, and refusing it is simpler than trusting it), bounded bodies,
//! short timeouts everywhere.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

pub const MAX_BODY: usize = 1024 * 1024;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, body: String) -> Response {
        Response {
            status,
            content_type: "application/json",
            body: body.into_bytes(),
        }
    }

    pub fn text(status: u16, body: &str) -> Response {
        Response {
            status,
            content_type: "text/plain; charset=utf-8",
            body: body.as_bytes().to_vec(),
        }
    }

    pub fn empty(status: u16) -> Response {
        Response {
            status,
            content_type: "text/plain; charset=utf-8",
            body: Vec::new(),
        }
    }
}

fn status_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Internal Server Error",
    }
}

/// Read one request from a connection. Enforces MAX_BODY and basic shape;
/// errors mean the connection is dropped without a response body worth
/// crafting.
pub fn read_request(stream: &mut TcpStream) -> Result<Request, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|e| e.to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("empty request line")?.to_string();
    let target = parts.next().ok_or("missing request target")?.to_string();
    let (path, query_text) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    let mut query = BTreeMap::new();
    for pair in query_text.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        query.insert(key.to_string(), value.to_string());
    }

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
        if headers.len() > 64 {
            return Err("too many headers".into());
        }
    }

    let content_length: usize = headers
        .get("content-length")
        .map(|v| v.parse().map_err(|_| "bad content-length".to_string()))
        .transpose()?
        .unwrap_or(0);
    if content_length > MAX_BODY {
        return Err("body too large".into());
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;

    Ok(Request {
        method,
        path,
        query,
        headers,
        body,
    })
}

pub fn write_response(stream: &mut TcpStream, response: &Response) {
    let head = format!(
        "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nx-content-type-options: nosniff\r\ncache-control: no-store\r\nconnection: close\r\n\r\n",
        response.status,
        status_phrase(response.status),
        response.content_type,
        response.body.len(),
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response.body);
    let _ = stream.flush();
}

// ---------------------------------------------------------------------------
// Client

pub struct ClientResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl ClientResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// All values for a repeatable header (Set-Cookie).
    pub fn header_all(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

pub fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    extra_headers: &[(String, String)],
    body: &[u8],
    timeout: Duration,
) -> Result<ClientResponse, String> {
    let mut stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;

    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nhost: {}\r\nconnection: close\r\ncontent-length: {}\r\n",
        addr.ip(),
        body.len()
    );
    for (key, value) in extra_headers {
        head.push_str(key);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(body).map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).map_err(|e| e.to_string())?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line {status_line:?}"))?;

    let mut headers = Vec::new();
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            if key.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().ok();
            }
            if key.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                return Err("chunked responses unsupported".into());
            }
            headers.push((key, value));
        }
        if headers.len() > 64 {
            return Err("too many response headers".into());
        }
    }

    let body = match content_length {
        Some(len) if len > MAX_BODY => return Err("response too large".into()),
        Some(len) => {
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
            buf
        }
        None => {
            // connection: close semantics — read to EOF, bounded.
            let mut buf = Vec::new();
            reader
                .take(MAX_BODY as u64 + 1)
                .read_to_end(&mut buf)
                .map_err(|e| e.to_string())?;
            if buf.len() > MAX_BODY {
                return Err("response too large".into());
            }
            buf
        }
    };

    Ok(ClientResponse {
        status,
        headers,
        body,
    })
}

pub fn basic_auth_header(username: &str, password: &str) -> (String, String) {
    (
        "authorization".to_string(),
        format!(
            "Basic {}",
            crate::crypto::base64_encode(format!("{username}:{password}").as_bytes())
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn client_and_server_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream).unwrap();
            assert_eq!(request.method, "POST");
            assert_eq!(request.path, "/echo");
            assert_eq!(request.query.get("q").map(String::as_str), Some("1"));
            assert_eq!(request.body, b"hello");
            write_response(&mut stream, &Response::json(200, "{\"ok\":true}".into()));
        });
        let response = request(
            addr,
            "POST",
            "/echo?q=1",
            &[("x-test".into(), "yes".into())],
            b"hello",
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"{\"ok\":true}");
        handle.join().unwrap();
    }

    #[test]
    fn rejects_oversized_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(read_request(&mut stream).is_err());
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        let huge = MAX_BODY + 1;
        stream
            .write_all(format!("POST / HTTP/1.1\r\ncontent-length: {huge}\r\n\r\n").as_bytes())
            .unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn builds_basic_auth() {
        // RFC 7617's example credentials.
        let (name, value) = basic_auth_header("Aladdin", "open sesame");
        assert_eq!(name, "authorization");
        assert_eq!(value, "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
    }
}
