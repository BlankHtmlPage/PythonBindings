// src/main.rs for the Rust helper "Flurion's Python Bindings"

use std::io::{self, BufRead, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::fs::{self, File};
use std::env;
use log::{info, debug, error, warn};

fn main() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    let log_level = if args.contains(&"--debug".to_string()) {
        "debug"
    } else if args.contains(&"--info".to_string()) {
        "info"
    } else if args.contains(&"--error".to_string()) {
        "error"
    } else {
        "warn"
    };
    env::set_var("RUST_LOG", log_level);
    env_logger::init();

    let listener = TcpListener::bind("127.0.0.1:6914")?;
    info!("Flurion's Python Bindings listening on localhost:6914");

    for stream in listener.incoming() {
        let stream = stream?;
        if let Err(e) = handle_connection(stream) {
            error!("Error handling connection: {}", e);
        }
    }

    Ok(())
}

fn handle_connection(mut stream: TcpStream) -> io::Result<()> {
    debug!("Received connection from: {:?}", stream.peer_addr());

    let mut buffer = Vec::new();
    let mut reader = io::BufReader::new(&stream);
    let mut line = String::new();

    // Read request line
    if reader.read_line(&mut line).is_err() {
        error!("Failed to read request line");
        send_response(&mut stream, 500, "Internal Server Error")?;
        return Ok(());
    }
    let request_line = line.trim().to_string();
    debug!("Request line: {}", request_line);
    line.clear();

    if request_line.starts_with("GET / HTTP/1.1") {
        let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Flurion's Python Bindings for MinePy mod</title>
</head>
<body>
    <h1>Helper is running.</h1>
</body>
</html>"#;
        send_response(&mut stream, 200, html)?;
        return Ok(());
    }

    if !request_line.starts_with("POST /api/interpreter HTTP/1.1") {
        info!("Invalid request path: {}", request_line);
        send_response(&mut stream, 404, "Not Found")?;
        return Ok(());
    }

    // Read headers
    let mut content_length = 0;
    loop {
        if reader.read_line(&mut line).is_err() {
            error!("Failed to read headers");
            send_response(&mut stream, 500, "Internal Server Error")?;
            return Ok(());
        }
        if line.trim().is_empty() {
            break;
        }
        if line.to_lowercase().starts_with("content-length:") {
            content_length = line.split(':').nth(1).unwrap().trim().parse::<usize>().unwrap_or(0);
            debug!("Content-Length: {}", content_length);
        }
        line.clear();
    }

    // Read body
    if content_length > 0 {
        buffer.resize(content_length, 0);
        if reader.read_exact(&mut buffer).is_err() {
            error!("Failed to read body");
            send_response(&mut stream, 500, "Internal Server Error")?;
            return Ok(());
        }
        debug!("Read body of length: {}", buffer.len());
    } else {
        info!("Missing body in request");
        send_response(&mut stream, 400, "Bad Request: Missing body")?;
        return Ok(());
    }

    let body = String::from_utf8_lossy(&buffer).to_string();
    debug!("Request body: {}", body);

    // Simple JSON parsing (assuming {"command": "code here"})
    let command = match extract_command(&body) {
        Some(cmd) => {
            debug!("Extracted command: {}", cmd);
            cmd
        }
        None => {
            info!("Invalid JSON in body");
            send_response(&mut stream, 400, "Bad Request: Invalid JSON")?;
            return Ok(());
        }
    };

    // Get temp dir and create fpb if needed
    let mut temp_path = std::env::temp_dir();
    temp_path.push("fpb");
    if let Err(e) = fs::create_dir_all(&temp_path) {
        error!("Failed to create temp dir: {}", e);
        send_response(&mut stream, 500, "Internal Server Error")?;
        return Ok(());
    }
    debug!("Created temp dir: {:?}", temp_path);

    let mut script_path = temp_path.clone();
    script_path.push("script.py");

    // Write code to file
    let mut file = match File::create(&script_path) {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to create script file: {}", e);
            send_response(&mut stream, 500, "Internal Server Error")?;
            return Ok(());
        }
    };
    if let Err(e) = file.write_all(command.as_bytes()) {
        error!("Failed to write to script file: {}", e);
        send_response(&mut stream, 500, "Internal Server Error")?;
        return Ok(());
    }
    debug!("Wrote script to: {:?}", script_path);

    // Run python
    debug!("Executing python on {:?}", script_path);
    let output = Command::new("python")
        .arg(script_path.to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let (status, response_body) = match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            debug!("Python stdout: {}", stdout);
            if !stderr.is_empty() {
                warn!("Python stderr: {}", stderr);
                (200, format!("Error: {}\nOutput: {}", stderr, stdout))
            } else {
                (200, stdout)
            }
        }
        Err(e) => {
            error!("Failed to execute python: {}", e);
            (500, format!("Failed to execute python: {}", e))
        }
    };

    debug!("Sending response: {}", response_body);
    send_response(&mut stream, status, &response_body)?;
    Ok(())
}

fn send_response(stream: &mut TcpStream, status: u32, body: &str) -> io::Result<()> {
    let status_text = if status == 200 {
        "OK"
    } else if status == 500 {
        "Internal Server Error"
    } else if status == 404 {
        "Not Found"
    } else {
        "Bad Request"
    };
    let status_line = format!("HTTP/1.1 {} {}\r\n", status, status_text);
    let content_type = if status == 200 && body.contains("<!DOCTYPE html") {
        "Content-Type: text/html\r\n"
    } else {
        "Content-Type: text/plain\r\n"
    };
    let content_length = format!("Content-Length: {}\r\n", body.len());
    let headers = format!("{}\r\n", content_type);

    if let Err(e) = stream.write_all(status_line.as_bytes()) {
        error!("Failed to send status line: {}", e);
        return Err(e);
    }
    if let Err(e) = stream.write_all(content_length.as_bytes()) {
        error!("Failed to send content length: {}", e);
        return Err(e);
    }
    if let Err(e) = stream.write_all(headers.as_bytes()) {
        error!("Failed to send headers: {}", e);
        return Err(e);
    }
    if let Err(e) = stream.write_all(body.as_bytes()) {
        error!("Failed to send body: {}", e);
        return Err(e);
    }
    debug!("Sent response with status: {}", status);
    Ok(())
}

fn extract_command(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let inner = &trimmed[1..trimmed.len()-1];
    if !inner.trim().starts_with("\"command\":") {
        return None;
    }
    let value_start = inner.find(':')? + 1;
    let value = inner[value_start..].trim();
    if value.starts_with('"') && value.ends_with('"') {
        Some(value[1..value.len()-1].to_string())
    } else {
        Some(value.to_string())
    }
}