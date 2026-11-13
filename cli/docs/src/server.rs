//! A loopback-only static preview of the generated site, never a publishing server.
use anyhow::Result;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Component, Path},
    time::Duration,
};

pub fn serve(root: &Path, base: &str, port: u16) -> Result<()> {
    let root = root.canonicalize()?;
    let prefix = format!("{}/", base.trim_end_matches('/'));
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    println!(
        "Documentation preview: http://{}{prefix}",
        listener.local_addr()?
    );
    for stream in listener.incoming() {
        let mut stream = stream?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        // A disconnected or malformed client must not stop the preview server.
        if let Err(error) = request(&root, &prefix, &mut stream) {
            eprintln!("Preview request failed: {error}");
        }
    }
    Ok(())
}

fn request(root: &Path, prefix: &str, stream: &mut TcpStream) -> Result<()> {
    let mut reader = BufReader::new(stream.take(8192));
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let first = line.clone();
    while !line.trim().is_empty() {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
    }
    let stream = reader.into_inner().into_inner();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let head = method == "HEAD";
    if method != "GET" && !head {
        return respond(
            stream,
            "405 Method Not Allowed",
            "text/plain",
            b"Method not allowed",
            head,
        );
    }
    let path = target.split('?').next().unwrap_or("");
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8()?;
    if !decoded.starts_with('/')
        || decoded.contains(['\\', '\0'])
        || Path::new(decoded.trim_start_matches('/'))
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return respond(
            stream,
            "400 Bad Request",
            "text/plain",
            b"Invalid preview path",
            head,
        );
    }
    let relative = if decoded == prefix.trim_end_matches('/') {
        Some("")
    } else {
        decoded.strip_prefix(prefix)
    };
    let Some(relative) = relative else {
        return respond(stream, "404 Not Found", "text/plain", b"Not found", head);
    };
    let mut file = root.join(relative);
    if file.is_dir() {
        file.push("index.html");
    }
    let Ok(file) = file.canonicalize() else {
        return respond(stream, "404 Not Found", "text/plain", b"Not found", head);
    };
    if !file.starts_with(root) || !file.is_file() {
        return respond(stream, "404 Not Found", "text/plain", b"Not found", head);
    }
    let content_type = match file.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("md" | "txt") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    };
    respond(stream, "200 OK", content_type, &fs::read(file)?, head)
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    bytes: &[u8],
    head: bool,
) -> Result<()> {
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n", bytes.len())?;
    if !head {
        stream.write_all(bytes)?;
    }
    stream.flush()?;
    Ok(())
}
