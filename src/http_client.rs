use reqwest::Client;
use std::time::Duration;

/// Configuration options for the asynchronous HTTP client.
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// Overall request timeout.
    pub timeout: Duration,

    /// Connection establishment timeout.
    pub connect_timeout: Duration,

    /// User-Agent header value sent with outgoing requests.
    pub user_agent: String,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(45),
            connect_timeout: Duration::from_secs(30),
            user_agent: format!("AgenticCommerce/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// Creates a reusable, asynchronous `reqwest::Client` configured with rustls and custom timeouts.
pub fn create_http_client(config: &HttpClientConfig) -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(config.timeout)
        .connect_timeout(config.connect_timeout)
        .user_agent(&config.user_agent)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_http_client_default() {
        let config = HttpClientConfig::default();
        let client = create_http_client(&config);
        assert!(client.is_ok(), "HTTP client should build successfully");
    }
}
