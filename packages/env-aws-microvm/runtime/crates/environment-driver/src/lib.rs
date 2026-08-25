use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::IpAddr;
use std::time::Duration;

pub const MAX_DISPATCH_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchRequest {
    pub operation_id: String,
    pub action: String,
    pub request: Value,
    pub deadline_at_ms: String,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct DriverError {
    pub status: StatusCode,
    pub message: String,
}

impl DriverError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait Driver: Send + Sync {
    async fn dispatch(&self, request: DispatchRequest) -> Result<Value, DriverError>;
}

#[derive(Clone)]
pub struct HttpRelayDriver {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    bearer: Option<String>,
    timeout: Duration,
}

impl HttpRelayDriver {
    pub fn new(
        endpoint: impl Into<String>,
        bearer: Option<String>,
        timeout: Duration,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(!timeout.is_zero(), "relay timeout must be positive");
        let endpoint = endpoint.into().parse::<reqwest::Url>()?;
        let loopback_http = endpoint.scheme() == "http"
            && endpoint
                .host_str()
                .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
                .is_some_and(|ip| ip.is_loopback());
        anyhow::ensure!(
            endpoint.scheme() == "https" || loopback_http,
            "relay URL must use HTTPS or literal loopback HTTP"
        );
        anyhow::ensure!(
            endpoint.username().is_empty()
                && endpoint.password().is_none()
                && endpoint.query().is_none()
                && endpoint.fragment().is_none(),
            "relay URL cannot contain credentials, query, or fragment"
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(timeout.min(Duration::from_secs(10)))
            .build()?;
        Ok(Self {
            client,
            endpoint,
            bearer,
            timeout,
        })
    }
}

#[async_trait]
impl Driver for HttpRelayDriver {
    async fn dispatch(&self, request: DispatchRequest) -> Result<Value, DriverError> {
        let deadline_at_ms = request
            .deadline_at_ms
            .parse::<u64>()
            .map_err(|_| DriverError::invalid("dispatch deadline is invalid"))?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let remaining = deadline_at_ms.saturating_sub(now_ms);
        if remaining == 0 {
            return Err(DriverError::unavailable("dispatch deadline elapsed"));
        }
        let mut builder = self
            .client
            .post(self.endpoint.clone())
            .header(header::CONTENT_TYPE, "application/json")
            .json(&request)
            .timeout(self.timeout.min(Duration::from_millis(remaining)));
        if let Some(bearer) = &self.bearer {
            builder = builder.bearer_auth(bearer);
        }
        let response = builder
            .send()
            .await
            .map_err(|_| DriverError::unavailable("Environment relay is unavailable"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(DriverError {
                status: if status.is_client_error() {
                    StatusCode::BAD_REQUEST
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                },
                message: format!("Environment relay returned {status}"),
            });
        }
        const MAX_RELAY_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|_| DriverError::unavailable("Environment relay response failed"))?;
            if body.len().saturating_add(chunk.len()) > MAX_RELAY_RESPONSE_BYTES {
                return Err(DriverError::invalid(
                    "Environment relay response is too large",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body)
            .map_err(|_| DriverError::invalid("Environment relay returned invalid JSON"))
    }
}

#[derive(Clone)]
struct DriverState {
    bearer: Arc<str>,
    drivers: Arc<HashMap<String, Arc<dyn Driver>>>,
}

pub fn router(
    bearer: impl Into<String>,
    drivers: impl IntoIterator<Item = (String, Arc<dyn Driver>)>,
) -> anyhow::Result<Router> {
    let bearer = bearer.into();
    anyhow::ensure!(
        !bearer.is_empty(),
        "Environment driver token cannot be empty"
    );
    let mut by_name = HashMap::new();
    for (name, driver) in drivers {
        anyhow::ensure!(
            !name.is_empty() && name.len() <= 64,
            "Environment driver name is invalid"
        );
        anyhow::ensure!(
            by_name.insert(name.clone(), driver).is_none(),
            "Environment driver {name} is registered more than once"
        );
    }
    anyhow::ensure!(
        !by_name.is_empty(),
        "at least one Environment driver is required"
    );
    let state = DriverState {
        bearer: bearer.into(),
        drivers: Arc::new(by_name),
    };
    Ok(Router::new()
        .route("/v1/dispatch", post(dispatch))
        .layer(DefaultBodyLimit::max(MAX_DISPATCH_BYTES))
        .with_state(state))
}

async fn dispatch(State(state): State<DriverState>, headers: HeaderMap, body: Bytes) -> Response {
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {}", state.bearer));
    if !authorized {
        return failure(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let request: DispatchRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "invalid dispatch request"),
    };
    if request.operation_id.is_empty()
        || request.operation_id.len() > 256
        || request.action.is_empty()
        || request.action.len() > 64
        || request.deadline_at_ms.parse::<u64>().is_err()
    {
        return failure(StatusCode::BAD_REQUEST, "invalid dispatch request");
    }
    let driver_name = request
        .request
        .get("binding")
        .and_then(|binding| binding.get("driver"))
        .and_then(Value::as_str);
    let Some(driver_name) = driver_name else {
        return failure(StatusCode::BAD_REQUEST, "dispatch binding has no driver");
    };
    let Some(driver) = state.drivers.get(driver_name) else {
        return failure(
            StatusCode::NOT_FOUND,
            "Environment driver is not configured",
        );
    };
    match driver.dispatch(request).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => failure(error.status, &error.message),
    }
}

fn failure(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt as _;

    struct Echo;

    #[async_trait]
    impl Driver for Echo {
        async fn dispatch(&self, request: DispatchRequest) -> Result<Value, DriverError> {
            Ok(serde_json::json!({
                "operation_id": request.operation_id,
                "action": request.action
            }))
        }
    }

    fn request(token: &str, driver: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/dispatch")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({
                    "operation_id":"operation-1",
                    "action":"submit",
                    "request":{"binding":{"driver":driver}},
                    "deadline_at_ms":"18446744073709551615"
                })
                .to_string(),
            ))
            .unwrap()
    }

    #[tokio::test]
    async fn dispatch_is_authenticated_and_routes_only_named_drivers() {
        let app = router(
            "secret",
            [("echo".into(), Arc::new(Echo) as Arc<dyn Driver>)],
        )
        .unwrap();
        assert_eq!(
            app.clone()
                .oneshot(request("wrong", "echo"))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(request("secret", "missing"))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            app.oneshot(request("secret", "echo"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
}
