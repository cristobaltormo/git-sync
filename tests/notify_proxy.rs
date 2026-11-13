use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

#[test]
fn notifications_go_through_the_proxy() {
    let ln = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = ln.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = ln.accept().unwrap();
        let mut r = BufReader::new(stream.try_clone().unwrap());
        let mut w = stream;
        let mut lines = Vec::new();
        loop {
            let (first, len) = head(&mut r);
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            lines.push(first.clone());
            if first.starts_with("CONNECT ") {
                w.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .unwrap();
                continue;
            }
            w.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .unwrap();
            break;
        }
        tx.send(lines).unwrap();
    });
    unsafe {
        std::env::set_var("HTTP_PROXY", format!("http://{addr}"));
        std::env::remove_var("NO_PROXY");
        std::env::remove_var("no_proxy");
    }
    gitsync::engine::send_test("http://notify.invalid/hook", "json").unwrap();
    let lines = rx.recv().unwrap();
    assert!(
        lines[0].contains("notify.invalid"),
        "the proxy must see the notification target: {lines:?}"
    );
    assert!(lines.last().unwrap().starts_with("POST "), "{lines:?}");
}

fn head(r: &mut impl BufRead) -> (String, usize) {
    let mut first = String::new();
    r.read_line(&mut first).unwrap();
    let mut len = 0;
    loop {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        if line.trim().is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap();
        }
    }
    (first.trim_end().to_string(), len)
}
