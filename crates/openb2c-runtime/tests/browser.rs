#![cfg(target_os = "linux")]
mod common;
use openb2c_runtime::{Service, router};
use serde_json::{Value, json};
use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

struct Server {
    base: String,
    admin_key: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn start(auth: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let (send, receive) = tokio::sync::oneshot::channel();
        let schema = common::schema();
        let service = Service::open(schema, ":memory:", true).unwrap();
        let admin_key = service
            .create_api_key(
                &json!({"user_id":1,"name":"Browser staff","scopes":"*"}),
                &openb2c_runtime::AuthContext::system(),
            )
            .unwrap()["key"]
            .as_str()
            .unwrap()
            .to_string();
        let worker = thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async move {
                    let app = router(service, "{}".into(), auth);
                    let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                    axum::serve(listener, app)
                        .with_graceful_shutdown(async {
                            let _ = receive.await;
                        })
                        .await
                        .unwrap();
                });
        });
        Self {
            base: format!("http://{address}"),
            admin_key,
            stop: Some(send),
            thread: Some(worker),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
struct Browser {
    process: Child,
    client: reqwest::blocking::Client,
    base: String,
    session: String,
}
impl Browser {
    fn new() -> Self {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let process = Command::new("chromedriver")
            .arg(format!("--port={port}"))
            .arg("--allowed-ips=127.0.0.1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("Run browser tests in devenv shell");
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();
        let base = format!("http://127.0.0.1:{port}");
        for _ in 0..100 {
            if client.get(format!("{base}/status")).send().is_ok() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let binary = std::env::var("CHROME_BINARY").expect("Devenv must supply CHROME_BINARY");
        let result:Value=client.post(format!("{base}/session")).json(&json!({"capabilities":{"alwaysMatch":{"browserName":"chrome","goog:chromeOptions":{"binary":binary,"args":["--headless=new","--no-sandbox","--disable-dev-shm-usage"]}}}})).send().unwrap().json().unwrap();
        let session = result["value"]["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("Could not start Chromium: {result}"))
            .to_string();
        Self {
            process,
            client,
            base,
            session,
        }
    }
    fn command(&self, method: &str, path: &str, input: Value) -> Value {
        let url = format!("{}/session/{}{}", self.base, self.session, path);
        let response = if method == "GET" {
            self.client.get(url).send()
        } else {
            self.client.post(url).json(&input).send()
        }
        .unwrap();
        let value: Value = response.json().unwrap();
        assert!(
            value["value"].get("error").is_none(),
            "WebDriver {path}: {value}"
        );
        value["value"].clone()
    }
    fn go(&self, url: &str) {
        self.command("POST", "/url", json!({"url":url}));
    }
    fn element(&self, css: &str) -> String {
        let result = self.command(
            "POST",
            "/element",
            json!({"using":"css selector","value":css}),
        );
        result["element-6066-11e4-a52e-4f735466cecf"]
            .as_str()
            .unwrap()
            .into()
    }
    fn click(&self, css: &str) {
        let id = self.element(css);
        let tag = self.command("GET", &format!("/element/{id}/name"), Value::Null);
        let token = self.command(
            "POST",
            "/execute/sync",
            json!({"script":"return performance.timeOrigin;","args":[]}),
        );
        self.command("POST", &format!("/element/{id}/click"), json!({}));
        if tag == "a" || tag == "button" {
            for _ in 0..200 {
                let current=self.command("POST","/execute/sync",json!({"script":"return {token:performance.timeOrigin,ready:document.readyState};","args":[]}));
                if current["token"] != token && current["ready"] == "complete" {
                    return;
                }
                thread::sleep(Duration::from_millis(25));
            }
            panic!(
                "Navigation did not complete after clicking {css}: {}",
                self.text()
            );
        }
    }
    fn fill(&self, css: &str, value: &str) {
        let id = self.element(css);
        self.command("POST", &format!("/element/{id}/clear"), json!({}));
        self.command(
            "POST",
            &format!("/element/{id}/value"),
            json!({"text":value}),
        );
    }
    fn text(&self) -> String {
        self.command(
            "POST",
            "/execute/sync",
            json!({"script":"return document.body.innerText;","args":[]}),
        )
        .as_str()
        .unwrap()
        .into()
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self
            .client
            .delete(format!("{}/session/{}", self.base, self.session))
            .send();
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[test]
fn public_browser_signs_in_checks_out_pays_and_views_owned_tickets() {
    let server = Server::start(true);
    let browser = Browser::new();
    browser.go(&server.base);
    assert!(browser.text().contains("The Magic Flute"));
    browser.click("a[href='/catalog/1']");
    browser.fill("input[name='quantity']", "2");
    browser.click("select[name='option_ticket_type'] option[value='patron']");
    browser.click("form[action='/cart'] button");
    assert!(browser.text().contains("Review cart"));
    assert!(browser.text().contains("£50.00"));
    browser.click("a[href='/login?return=%2Fcart']");
    browser.fill("input[name='email']", "browser-checkout@example.test");
    browser.click("form[action='/login'] button");
    assert!(browser.text().contains("Verification code"));
    browser.click("form[action='/login'] button");
    assert!(browser.text().contains("Reserve and continue"));
    browser.click("form input[value='checkout'] + button");
    assert!(browser.text().contains("Payment details"));
    browser.click("main form button");
    assert!(browser.text().contains("Complete payment"));
    browser.click("main a.button");
    assert!(browser.text().contains("Local payment simulator"));
    browser.click("form input[value='authorized'] + button");
    assert!(browser.text().contains("Booking confirmed"));
    browser.click("main a[href='/account']");
    assert!(browser.text().contains("My tickets"));
    assert!(browser.text().contains("confirmed"));
    browser.command("POST", "/window/rect", json!({"width":390,"height":844}));
    browser.go(&server.base);
    let width=browser.command("POST","/execute/sync",json!({"script":"return document.documentElement.scrollWidth <= window.innerWidth;","args":[]}));
    assert_eq!(width, true);
}
#[test]
fn admin_browser_edits_records_confirms_workflows_and_uses_graph_views() {
    let server = Server::start(true);
    let browser = Browser::new();
    browser.go(&format!("{}/admin/production/1", server.base));
    assert!(browser.text().contains("Sign in"));
    browser.click("details summary");
    browser.fill("input[name='token']", &server.admin_key);
    browser.click("details form button");
    assert!(browser.text().contains("The Magic Flute"));
    assert!(browser.text().contains("Rehearsal"));
    browser.fill(
        "form[action='/admin/production/1'] input[name='name']",
        "The Magic Flute — Rust",
    );
    browser.click("form[action='/admin/production/1'] button");
    assert!(browser.text().contains("The Magic Flute — Rust"));
    browser.go(&format!("{}/admin/rehearsal_requirement/1", server.base));
    browser.click("a[href='/admin/rehearsal_requirement/1/operations/cover']");
    assert!(browser.text().contains("Confirm"));
    browser.click("main form button");
    assert!(browser.text().contains("Covered"));
    browser.go(&format!(
        "{}/admin/rehearsal_requirement?view=board",
        server.base
    ));
    assert!(browser.text().contains("Act I finale"));
    browser.go(&format!("{}/admin/rehearsal?view=calendar", server.base));
    assert!(browser.text().contains("29 Jun 2026"));
}
