//! Production [`HttpTransport`]. `reqwest` + rustls, no native-tls.

use std::future::Future;
use std::time::Duration;

use crate::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};

const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, TransportError> {
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| TransportError::Io(err.to_string()))?;
        Ok(Self { client })
    }
}

impl HttpTransport for ReqwestTransport {
    fn send(
        &self,
        req: &HttpRequest,
    ) -> impl Future<Output = Result<HttpResponse, TransportError>> + Send {
        self.send_inner(req)
    }
}

impl ReqwestTransport {
    async fn send_inner(&self, req: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let method = reqwest::Method::from_bytes(req.method.as_bytes())
            .map_err(|err| TransportError::Io(err.to_string()))?;
        let mut builder = self.client.request(method, &req.url);
        for (name, value) in &req.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = &req.body {
            builder = builder.body(body.clone());
        }
        let response = builder
            .send()
            .await
            .map_err(|err| TransportError::Io(err.to_string()))?;
        let status = response.status().as_u16();
        if let Some(len) = response.content_length()
            && len > MAX_RESPONSE_BYTES
        {
            return Err(TransportError::Io(format!(
                "response body {len} exceeds {MAX_RESPONSE_BYTES} byte cap"
            )));
        }
        let headers = response
            .headers()
            .iter()
            .map(|(k, v)| {
                let value = match v.to_str() {
                    Ok(s) => s.to_string(),
                    Err(_) => v.as_bytes().iter().map(|&b| char::from(b)).collect(),
                };
                (k.as_str().to_string(), value)
            })
            .collect();
        let body = response
            .bytes()
            .await
            .map_err(|err| TransportError::Io(err.to_string()))?
            .to_vec();
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(TransportError::Io(format!(
                "response body {} exceeds {MAX_RESPONSE_BYTES} byte cap",
                body.len()
            )));
        }
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rustls_client_constructs_without_native_tls() {
        let transport = ReqwestTransport::new().expect("client");
        let _ = transport.client;
    }
}
