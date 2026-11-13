mod common;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
fn receive(mut stream: TcpStream) -> (String, std::collections::BTreeMap<String, String>, Vec<u8>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reader = BufReader::new(&mut stream);
    let mut first = String::new();
    reader.read_line(&mut first).unwrap();
    let mut headers = std::collections::BTreeMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.trim().split_once(':') {
            headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).unwrap();
    (first, headers, bytes)
}
#[test]
fn failed_deliveries_back_off_retry_dead_letter_and_sign_each_webhook() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(Vec::new()));
    let capture = received.clone();
    let mock = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        for status in [500, 500, 200, 500, 500, 500] {
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = receive(stream.try_clone().unwrap());
                        capture.lock().unwrap().push(request);
                        write!(stream,"HTTP/1.1 {status} Response\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
                        break;
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(e) => panic!("Delivery mock did not receive request: {e}"),
                }
            }
        }
    });
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "delivery_retry_child"])
        .env("OPENB2C_DELIVERY_CHILD", "1")
        .env("WEBHOOK_URL", url)
        .env("WEBHOOK_SIGNING_SECRET", "test-signing-secret")
        .env("EFFECT_RETRY_DELAY_MS", "10000")
        .env("EFFECT_MAX_ATTEMPTS", "3")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    mock.join().unwrap();
    let captured = received.lock().unwrap();
    assert_eq!(captured.len(), 6);
    for (_, headers, body) in captured.iter() {
        assert!(openb2c_runtime::effects::verify_webhook(
            "test-signing-secret",
            body,
            &headers["x-openb2c-timestamp"],
            &headers["x-openb2c-signature"],
            300
        ));
        assert!(headers.contains_key("idempotency-key"));
    }
    assert_eq!(
        captured[0].1["idempotency-key"],
        captured[2].1["idempotency-key"]
    );
}
#[test]
#[ignore = "Executed by the parent with isolated provider environment"]
fn delivery_retry_child() {
    assert_eq!(std::env::var("OPENB2C_DELIVERY_CHILD").as_deref(), Ok("1"));
    let service = common::service();
    service
        .queue_effect(
            "retry-once",
            "call",
            &json!({"service":"webhook","action":"dispatch","amount":5000}),
            &json!({}),
        )
        .unwrap();
    assert_eq!(service.dispatch_effects(false).unwrap()["failed"], 1);
    assert_eq!(service.dispatch_effects(false).unwrap()["attempted"], 0);
    assert_eq!(service.dispatch_effects(true).unwrap()["failed"], 1);
    assert_eq!(service.dispatch_effects(true).unwrap()["succeeded"], 1);
    assert_eq!(service.effects(None).unwrap()[0]["attempts"], 3);
    service
        .queue_effect(
            "dead-letter",
            "call",
            &json!({"service":"webhook","action":"dispatch"}),
            &json!({}),
        )
        .unwrap();
    for _ in 0..3 {
        service.dispatch_effects(true).unwrap();
    }
    let row = service
        .effects(None)
        .unwrap()
        .into_iter()
        .find(|e| e["idempotency_key"] == "dead-letter")
        .unwrap();
    assert_eq!(row["status"], "dead_letter");
    service.retry_effect(row["id"].as_i64().unwrap()).unwrap();
    assert_eq!(service.effects(None).unwrap()[0]["status"], "pending");
}
#[test]
fn resend_identity_delivery_uses_the_configured_endpoint_and_idempotency_key() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/emails", listener.local_addr().unwrap());
    let mock = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = receive(stream.try_clone().unwrap());
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        )
        .unwrap();
        request
    });
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "resend_child"])
        .env("EMAIL_PROVIDER", "resend")
        .env("RESEND_API_KEY", "test-key")
        .env("EMAIL_FROM", "Opera <login@example.test>")
        .env("RESEND_EMAILS_URL", url)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let (path, headers, body) = mock.join().unwrap();
    assert!(path.starts_with("POST /emails"));
    assert_eq!(headers["authorization"], "Bearer test-key");
    assert_eq!(headers["idempotency-key"], "identity-challenge-42");
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["to"][0], "recipient@example.test");
    assert!(body["text"].as_str().unwrap().contains("123456"));
}
#[test]
#[ignore = "Executed by the parent with isolated email environment"]
fn resend_child() {
    common::service()
        .deliver_identity_email("recipient@example.test", "123456", 42)
        .unwrap();
}
