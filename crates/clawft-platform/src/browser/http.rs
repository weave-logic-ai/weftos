//! Browser HTTP client implementation using the web-sys fetch API.
//!
//! Wraps the browser's `fetch()` via [`web_sys`] and [`wasm_bindgen_futures`]
//! to implement the platform [`HttpClient`] trait. Requests are dispatched
//! using `WorkerGlobalScope::fetch` (web workers) or `Window::fetch`
//! (main thread).

use async_trait::async_trait;
use std::collections::HashMap;

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestInit, RequestMode, Response};

use crate::http::{HttpClient, HttpResponse};

/// HTTP client for browser/WASM targets using the fetch API.
pub struct BrowserHttpClient;

impl BrowserHttpClient {
    /// Create a new browser HTTP client.
    pub fn new() -> Self {
        Self
    }
}

impl Default for BrowserHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a [`JsValue`] error into a boxed std error.
fn js_err(value: JsValue) -> Box<dyn std::error::Error + Send + Sync> {
    let msg = if let Some(s) = value.as_string() {
        s
    } else {
        format!("{:?}", value)
    };
    msg.into()
}

#[async_trait(?Send)]
impl HttpClient for BrowserHttpClient {
    async fn request(
        &self,
        method: &str,
        url: &str,
        headers: &HashMap<String, String>,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, Box<dyn std::error::Error + Send + Sync>> {
        // Build request init options.
        let opts = RequestInit::new();
        opts.set_method(method);
        opts.set_mode(RequestMode::Cors);

        // Attach body if present.
        if let Some(body_bytes) = body {
            let uint8_array = js_sys::Uint8Array::from(body_bytes);
            opts.set_body(&uint8_array);
        }

        // Build the Request object.
        let request = Request::new_with_str_and_init(url, &opts).map_err(js_err)?;

        // Set headers on the request.
        let req_headers: Headers = request.headers();
        for (key, value) in headers {
            req_headers.set(key, value).map_err(js_err)?;
        }

        // Dispatch fetch. Try WorkerGlobalScope first, then Window.
        let promise = if let Some(worker) = js_sys::global().dyn_ref::<web_sys::WorkerGlobalScope>()
        {
            worker.fetch_with_request(&request)
        } else if let Some(window) = web_sys::window() {
            window.fetch_with_request(&request)
        } else {
            return Err("no global fetch available (not a Window or WorkerGlobalScope)".into());
        };

        // Await the fetch promise.
        let resp_value = JsFuture::from(promise).await.map_err(js_err)?;
        let resp: Response = resp_value.dyn_into().map_err(js_err)?;

        // Extract status code.
        let status = resp.status();

        // Extract response headers.
        let mut resp_headers = HashMap::new();
        let header_entries = resp.headers();
        // web_sys Headers doesn't have a direct iterator, so we use the
        // JS iterator protocol via js_sys.
        let js_iter = js_sys::try_iter(&header_entries)
            .map_err(js_err)?
            .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                "headers not iterable".into()
            })?;
        for entry in js_iter {
            let entry = entry.map_err(js_err)?;
            let pair = js_sys::Array::from(&entry);
            if pair.length() >= 2 {
                if let (Some(k), Some(v)) = (pair.get(0).as_string(), pair.get(1).as_string()) {
                    resp_headers.insert(k, v);
                }
            }
        }

        // Read body as ArrayBuffer then convert to Vec<u8>.
        let body_promise = resp.array_buffer().map_err(js_err)?;
        let body_value = JsFuture::from(body_promise).await.map_err(js_err)?;
        let body_array = js_sys::Uint8Array::new(&body_value);
        let body_bytes = body_array.to_vec();

        Ok(HttpResponse {
            status,
            headers: resp_headers,
            body: body_bytes,
        })
    }
}
