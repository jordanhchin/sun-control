//! API client tests against a mock Sunshine server.

use serde_json::json;
use sun_control::api::tls::Trust;
use sun_control::api::{ApiError, Credentials, SunshineClient};
use wiremock::matchers::{basic_auth, body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> SunshineClient {
    let creds = Credentials {
        username: "admin".into(),
        password: "pw".into(),
    };
    let url = format!("{}/", server.uri()).parse().unwrap();
    SunshineClient::new(url, Trust::Any, Some(creds)).unwrap()
}

#[tokio::test]
async fn reads_config_and_strips_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/config"))
        .and(basic_auth("admin", "pw"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "true", "platform": "linux", "version": "2025.924.1",
            "sunshine_name": "den", "brand_new_key": "1"
        })))
        .mount(&server)
        .await;

    let cfg = client(&server).config().await.unwrap();
    assert_eq!(cfg.version.as_deref(), Some("2025.924.1"));
    assert_eq!(cfg.values.len(), 2);
}

#[tokio::test]
async fn app_round_trip_keeps_unknown_fields() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/apps"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "env": {"PATH": "x"},
            "apps": [{"name": "Desktop", "image-path": "desktop.png", "future": true}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/apps"))
        .and(body_partial_json(
            json!({"name": "Desktop", "future": true, "index": 0}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": true})))
        .expect(1)
        .mount(&server)
        .await;

    let api = client(&server);
    let list = api.apps().await.unwrap();
    assert!(list.extra.contains_key("env"));
    api.save_app(&list.apps[0], Some(0)).await.unwrap();
}

#[tokio::test]
async fn maps_errors() {
    let server = MockServer::start().await;
    Mock::given(path("/api/config"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(path("/api/pin"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status": "false", "error": "bad pin"})),
        )
        .mount(&server)
        .await;

    let api = client(&server);
    assert!(matches!(api.config().await, Err(ApiError::Unauthorized)));
    assert!(matches!(api.logs().await, Err(ApiError::Unsupported(_))));
    match api.pair("1234", "tv").await {
        Err(ApiError::Server(msg)) => assert_eq!(msg, "bad pin"),
        other => panic!("unexpected {other:?}"),
    }
}
