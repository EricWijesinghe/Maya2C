//! A minimal W3C `WebDriver` client for driving the wallet through
//! `tauri-driver`, in Rust so the end-to-end suite needs no Node toolchain.
//!
//! Only the commands the suite uses: new session, find element(s), click,
//! send keys, text, enabled, delete session.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The W3C element reference key.
const ELEMENT: &str = "element-6066-11e4-a52e-4f735466cecf";

/// A `WebDriver` session.
pub struct Session {
    http: reqwest::blocking::Client,
    base: String,
    id: String,
}

/// An element handle.
#[derive(Clone, Debug)]
pub struct Element(String);

/// A failed `WebDriver` call, with the command that failed.
#[derive(Debug, thiserror::Error)]
#[error("webdriver {command}: {detail}")]
pub struct DriverError {
    command: String,
    detail: String,
}

fn failed(command: &str, detail: &impl ToString) -> DriverError {
    DriverError {
        command: command.to_owned(),
        detail: detail.to_string(),
    }
}

impl Session {
    /// Starts the Tauri application at `application` through the driver at
    /// `base` (e.g. `http://127.0.0.1:4444`).
    ///
    /// # Errors
    ///
    /// The driver refused or did not answer.
    pub fn start(base: &str, application: &str) -> Result<Self, DriverError> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| failed("client", &e))?;
        let caps = json!({"capabilities": {"alwaysMatch": {"browserName": "wry", "tauri:options": {"application": application}}}});
        let reply: Value = http
            .post(format!("{base}/session"))
            .json(&caps)
            .send()
            .and_then(reqwest::blocking::Response::json)
            .map_err(|e| failed("new session", &e))?;
        let id = reply["value"]["sessionId"]
            .as_str()
            .ok_or_else(|| failed("new session", &reply))?
            .to_owned();
        Ok(Self {
            http,
            base: base.to_owned(),
            id,
        })
    }

    fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, DriverError> {
        let url = format!("{}/session/{}{path}", self.base, self.id);
        let request = self.http.request(method, &url);
        let request = match body {
            Some(b) => request.json(&b),
            None => request,
        };
        let reply: Value = request
            .send()
            .and_then(reqwest::blocking::Response::json)
            .map_err(|e| failed(path, &e))?;
        if reply["value"].get("error").is_some() {
            return Err(failed(path, &reply["value"]));
        }
        Ok(reply["value"].clone())
    }

    /// Every element matching a CSS selector.
    ///
    /// # Errors
    ///
    /// The driver failed.
    pub fn all(&self, css: &str) -> Result<Vec<Element>, DriverError> {
        let found = self.call(
            reqwest::Method::POST,
            "/elements",
            Some(json!({"using": "css selector", "value": css})),
        )?;
        Ok(found
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| e[ELEMENT].as_str().map(|s| Element(s.to_owned())))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The first element matching an `XPath`, waiting up to `wait`.
    ///
    /// # Errors
    ///
    /// Nothing matched in time.
    pub fn xpath(&self, xpath: &str, wait: Duration) -> Result<Element, DriverError> {
        let deadline = Instant::now() + wait;
        loop {
            let found = self.call(
                reqwest::Method::POST,
                "/elements",
                Some(json!({"using": "xpath", "value": xpath})),
            )?;
            if let Some(id) = found
                .as_array()
                .and_then(|a| a.first())
                .and_then(|e| e[ELEMENT].as_str())
            {
                return Ok(Element(id.to_owned()));
            }
            if Instant::now() > deadline {
                return Err(failed(
                    "xpath",
                    &format!("{xpath} not found within {wait:?}"),
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Waits until at least `n` elements match a CSS selector.
    ///
    /// # Errors
    ///
    /// Fewer matched in time.
    pub fn wait_for(
        &self,
        css: &str,
        n: usize,
        wait: Duration,
    ) -> Result<Vec<Element>, DriverError> {
        let deadline = Instant::now() + wait;
        loop {
            let found = self.all(css)?;
            if found.len() >= n {
                return Ok(found);
            }
            if Instant::now() > deadline {
                return Err(failed(
                    "wait",
                    &format!("{css}: {} of {n} within {wait:?}", found.len()),
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Clicks.
    ///
    /// # Errors
    ///
    /// The driver failed.
    pub fn click(&self, e: &Element) -> Result<(), DriverError> {
        self.call(
            reqwest::Method::POST,
            &format!("/element/{}/click", e.0),
            Some(json!({})),
        )
        .map(|_| ())
    }

    /// Types into an element.
    ///
    /// # Errors
    ///
    /// The driver failed.
    pub fn type_text(&self, e: &Element, text: &str) -> Result<(), DriverError> {
        self.call(
            reqwest::Method::POST,
            &format!("/element/{}/value", e.0),
            Some(json!({"text": text})),
        )
        .map(|_| ())
    }

    /// Replaces an input's contents by keystrokes (select all, delete, type),
    /// so the page sees the same input events a person makes.
    ///
    /// # Errors
    ///
    /// The driver failed.
    pub fn replace_text(&self, e: &Element, text: &str) -> Result<(), DriverError> {
        // WebDriver key codes: Control, "a", release all, Backspace.
        self.type_text(e, "\u{E009}a\u{E000}\u{E003}")?;
        self.type_text(e, text)
    }

    /// Visible text.
    ///
    /// # Errors
    ///
    /// The driver failed.
    pub fn text(&self, e: &Element) -> Result<String, DriverError> {
        Ok(self
            .call(
                reqwest::Method::GET,
                &format!("/element/{}/text", e.0),
                None,
            )?
            .as_str()
            .unwrap_or_default()
            .to_owned())
    }

    /// Whether an element is enabled.
    ///
    /// # Errors
    ///
    /// The driver failed.
    pub fn enabled(&self, e: &Element) -> Result<bool, DriverError> {
        Ok(self
            .call(
                reqwest::Method::GET,
                &format!("/element/{}/enabled", e.0),
                None,
            )?
            .as_bool()
            .unwrap_or(false))
    }

    /// Runs `script` in the page and returns its value.
    ///
    /// # Errors
    ///
    /// The driver failed or the script threw.
    pub fn execute(&self, script: &str) -> Result<Value, DriverError> {
        self.call(
            reqwest::Method::POST,
            "/execute/sync",
            Some(json!({"script": script, "args": []})),
        )
    }

    /// The page's current HTML, for a failure message.
    ///
    /// # Errors
    ///
    /// The driver failed.
    pub fn source(&self) -> Result<String, DriverError> {
        Ok(self
            .call(reqwest::Method::GET, "/source", None)?
            .as_str()
            .unwrap_or_default()
            .to_owned())
    }

    /// Ends the session, closing the application.
    ///
    /// # Errors
    ///
    /// The driver failed.
    pub fn end(self) -> Result<(), DriverError> {
        self.http
            .delete(format!("{}/session/{}", self.base, self.id))
            .send()
            .map(|_| ())
            .map_err(|e| failed("delete session", &e))
    }
}
