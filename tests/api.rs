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
    match api.pair("1234", "tv", None).await {
        Err(ApiError::Server(msg)) => assert_eq!(msg, "bad pin"),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn pairs_with_a_pending_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/pin"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "pairings": [{"id": "p1", "name": "Steam Deck", "address": "10.0.0.5"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pin"))
        .and(body_partial_json(
            json!({"pairing_id": "p1", "pin": "1234", "name": "deck"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": false})))
        .expect(1)
        .mount(&server)
        .await;

    let api = client(&server);
    let pending = api.pending_pairings().await.unwrap();
    assert_eq!(pending[0].label(), "Steam Deck — 10.0.0.5");
    // A bare `status: false` gets a helpful message rather than "api/pin failed".
    match api.pair("1234", "deck", Some("p1")).await {
        Err(ApiError::Server(msg)) => assert!(msg.contains("PIN"), "{msg}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn never_follows_redirects_with_credentials() {
    let server = MockServer::start().await;
    let elsewhere = MockServer::start().await;
    Mock::given(path("/api/password"))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("Location", format!("{}/steal", elsewhere.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(path("/steal"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": true})))
        .expect(0)
        .mount(&elsewhere)
        .await;

    let api = client(&server);
    let creds = Credentials {
        username: "admin".into(),
        password: "pw".into(),
    };
    let new = Credentials {
        username: "admin".into(),
        password: "secret".into(),
    };
    match api.change_credentials(&creds, &new).await {
        Err(ApiError::Redirected { location, .. }) => assert!(location.ends_with("/steal")),
        other => panic!("unexpected {other:?}"),
    }
    // `elsewhere` verifies on drop that it received nothing.
}

#[tokio::test]
async fn rejects_unexpected_success_bodies() {
    let server = MockServer::start().await;
    Mock::given(path("/api/config"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>Sign in</html>"))
        .mount(&server)
        .await;
    Mock::given(path("/api/apps/close"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let api = client(&server);
    assert!(matches!(
        api.save_config(&serde_json::Map::new()).await,
        Err(ApiError::Unexpected { .. })
    ));
    // An empty 200 is still fine.
    api.close_app().await.unwrap();
}
