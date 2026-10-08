use std::sync::OnceLock;
use std::time::Duration;
use ureq::Agent;

/// The Moodle mobile web-service endpoints expect a mobile-app style agent.
pub const USER_AGENT: &str = "Mozilla/5.0 (MoodleMobile; Android)";

/// Shared HTTP client. Connect and first-byte timeouts keep a dead network from
/// hanging the CLI forever; there is deliberately no overall timeout so large
/// downloads on slow links can still finish.
pub fn agent() -> &'static Agent {
    static AGENT: OnceLock<Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let config = Agent::config_builder()
            .user_agent(USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .build();
        Agent::new_with_config(config)
    })
}

/// Run a request, retrying a couple of times when the network hiccups (connection reset,
/// timeout, DNS blip). HTTP error statuses are real answers and are never retried.
/// Only use this for requests that are safe to repeat.
pub fn with_retries<T>(
    mut request: impl FnMut() -> Result<T, ureq::Error>,
) -> Result<T, ureq::Error> {
    let mut attempt = 0;
    loop {
        match request() {
            Ok(value) => return Ok(value),
            Err(e) if attempt < 2 && !matches!(e, ureq::Error::StatusCode(_)) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(250 * attempt));
            }
            Err(e) => return Err(e),
        }
    }
}
