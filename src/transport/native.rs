//! The native transport: reqwest over rustls.

use std::{sync::Arc, time::Duration};

use rustls_platform_verifier::BuilderVerifierExt;

use super::{Method, Request, Response, Transport, TransportError};

/// How long one request may take before it counts as a transport failure.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The native transport: `reqwest` over `rustls`, with the `ring` crypto
/// provider and the platform's root certificates, run on `tokio`.
#[derive(Debug, Clone)]
pub struct NativeTransport {
    http: reqwest::Client,
}

impl NativeTransport {
    /// Builds the transport.
    ///
    /// It sets up its own TLS configuration rather than installing a
    /// process-wide default, so it does not interfere with the caller's.
    ///
    /// # Errors
    ///
    /// Returns an error if TLS cannot be configured, for example when the
    /// platform's root certificates cannot be read, or if the HTTP client
    /// cannot be built.
    pub fn new() -> Result<Self, TransportError> {
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(TransportError::new)?
        .with_platform_verifier()
        .map_err(TransportError::new)?
        .with_no_client_auth();
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(TransportError::new)?;
        Ok(Self { http })
    }
}

impl Transport for NativeTransport {
    async fn send(&self, request: Request<'_>) -> Result<Response, TransportError> {
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Put => reqwest::Method::PUT,
            Method::Delete => reqwest::Method::DELETE,
        };
        let mut builder = self
            .http
            .request(method, &request.url)
            .bearer_auth(request.key.expose())
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = request.body {
            builder = builder
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body);
        }
        let response = builder.send().await.map_err(TransportError::new)?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let body = response
            .bytes()
            .await
            .map_err(TransportError::new)?
            .to_vec();
        Ok(Response {
            status,
            retry_after,
            body,
        })
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

#[cfg(test)]
mod tests {
    use super::NativeTransport;

    /// reqwest accepts the preconfigured TLS backend only at build time, and
    /// rejects a configuration of the wrong type there, so building is the
    /// check.
    #[test]
    fn builds_with_its_own_tls_configuration() {
        let built = NativeTransport::new();
        assert!(
            built.is_ok(),
            "the native transport should build: {:?}",
            built.err()
        );
    }
}
