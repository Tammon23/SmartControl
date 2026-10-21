use super::*;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tower::ServiceExt;

fn config(dir: &std::path::Path) -> Config {
    serde_json::from_value(json!({"demo":true,"clients":[{"name":"windows","token":"w".repeat(32)},{"name":"kindle","token":"k".repeat(32)}],"admin_token":"a".repeat(32),"device_id":"00000000-0000-0000-0000-000000000000","tokens_file":dir.join("tokens.json")})).unwrap()
}
async fn request(
    app: Arc<App>,
    method: &str,
    path: &str,
    key: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", "application/json")
        .body(if method == "GET" {
            Body::empty()
        } else {
            Body::from(body.to_string())
        })
        .unwrap();
    let response = router(app).oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn unauthorized_writes_never_change_state() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::new(config(dir.path())).await.unwrap();
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/api/bulb/power",
            "wrong",
            json!({"on":true})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (code, value) = request(app, "GET", "/api/bulb", &"w".repeat(32), Value::Null).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(value["power"], false);
}
#[tokio::test]
async fn malformed_inputs_do_not_mutate_the_bulb() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::new(config(dir.path())).await.unwrap();
    let key = "k".repeat(32);
    for (route, body) in [
        ("brightness", json!({"percent":101})),
        ("brightness", json!({"percent":-1})),
        ("power", json!({"on":"yes"})),
        ("colour", json!({"r":256,"g":0,"b":0})),
        ("preset", json!({"name":"unknown"})),
        ("temperature", json!({"kelvin":99999})),
    ] {
        assert_eq!(
            request(
                app.clone(),
                "POST",
                &format!("/api/bulb/{route}"),
                &key,
                body
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        request(app, "GET", "/api/bulb", &key, Value::Null).await.1["power"],
        false
    );
}
#[tokio::test]
async fn both_clients_share_state_and_colour_changes_are_consistent() {
    let dir = tempfile::tempdir().unwrap();
    let app = App::new(config(dir.path())).await.unwrap();
    let w = "w".repeat(32);
    let k = "k".repeat(32);
    let result = request(
        app.clone(),
        "POST",
        "/api/bulb/colour",
        &w,
        json!({"r":0,"g":128,"b":0}),
    )
    .await;
    assert_eq!(result.1["confirmed"], false);
    let state = request(app.clone(), "GET", "/api/bulb", &k, Value::Null)
        .await
        .1;
    assert_eq!(state["power"], true);
    assert_eq!(state["brightness"], 50);
    assert!((state["hue"].as_f64().unwrap() - 100.0 / 3.0).abs() < 0.01);
    request(
        app.clone(),
        "POST",
        "/api/bulb/brightness",
        &k,
        json!({"percent":37}),
    )
    .await;
    request(
        app.clone(),
        "POST",
        "/api/bulb/power",
        &w,
        json!({"on":false}),
    )
    .await;
    let state = request(app, "GET", "/api/bulb", &k, Value::Null).await.1;
    assert_eq!(state["power"], false);
    assert_eq!(state["brightness"], 37);
}

#[derive(Default)]
struct Mock {
    commands: Mutex<Vec<Value>>,
    refreshes: AtomicUsize,
    reject: AtomicBool,
}
async fn mock_upstream() -> (String, Arc<Mock>, tokio::task::JoinHandle<()>) {
    let mock = Arc::new(Mock::default());
    let routes=Router::new()
        .route("/devices/{id}/status",get(|State(_m):State<Arc<Mock>>,headers:HeaderMap|async move{
            if bearer(&headers)!=Some("new-access") {return (StatusCode::UNAUTHORIZED,Json(json!({})));}
            (StatusCode::OK,Json(json!({"components":{"main":{"switch":{"switch":{"value":"off","timestamp":"2026-10-20T12:00:00Z"}},"switchLevel":{"level":{"value":50}},"healthCheck":{"DeviceWatch-DeviceStatus":{"value":"online"}}}}})))
        }))
        .route("/devices/{id}/commands",post(|State(m):State<Arc<Mock>>,headers:HeaderMap,Json(body):Json<Value>|async move{
            if bearer(&headers)!=Some("new-access") {return (StatusCode::UNAUTHORIZED,Json(json!({})));}
            m.commands.lock().await.push(body.clone());
            if m.reject.load(Ordering::SeqCst) {return (StatusCode::SERVICE_UNAVAILABLE,Json(json!({"error":"contains private upstream detail"})));}
            let results:Vec<_>=body["commands"].as_array().unwrap().iter().map(|_|json!({"status":"ACCEPTED"})).collect();
            (StatusCode::OK,Json(json!({"results":results})))
        }))
        .route("/oauth/token",post(|State(m):State<Arc<Mock>>|async move{
            m.refreshes.fetch_add(1,Ordering::SeqCst);
            Json(json!({"access_token":"new-access","refresh_token":"rotated-refresh","expires_in":86400}))
        })).with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, routes).await.unwrap();
    });
    (url, mock, task)
}

#[tokio::test]
async fn oauth_refresh_is_persisted_and_only_one_refresh_is_used() {
    let dir = tempfile::tempdir().unwrap();
    let (url, mock, task) = mock_upstream().await;
    let mut c = config(dir.path());
    c.demo = false;
    c.smartthings_base = url.clone();
    c.oauth = Some(OAuthConfig {
        client_id: "own-client".into(),
        client_secret: "secret".into(),
        redirect_uri: "http://localhost/oauth/callback".into(),
        authorize_url: format!("{url}/authorize"),
        token_url: format!("{url}/oauth/token"),
    });
    tokio::fs::write(
        &c.tokens_file,
        serde_json::to_vec(&Tokens {
            access_token: "expired".into(),
            refresh_token: Some("old-refresh".into()),
            expires_at: 0,
        })
        .unwrap(),
    )
    .await
    .unwrap();
    let file = c.tokens_file.clone();
    let app = App::new(c).await.unwrap();
    let (a, b) = tokio::join!(app.read(true), app.read(true));
    assert!(a.is_ok() && b.is_ok());
    assert_eq!(mock.refreshes.load(Ordering::SeqCst), 1);
    let persisted: Tokens = serde_json::from_slice(&tokio::fs::read(file).await.unwrap()).unwrap();
    assert_eq!(persisted.refresh_token.as_deref(), Some("rotated-refresh"));
    assert_eq!(persisted.access_token, "new-access");
    task.abort();
}

#[tokio::test]
async fn upstream_commands_have_correct_shape_and_failures_are_not_success() {
    let dir = tempfile::tempdir().unwrap();
    let (url, mock, task) = mock_upstream().await;
    let mut c = config(dir.path());
    c.demo = false;
    c.smartthings_base = url;
    tokio::fs::write(
        &c.tokens_file,
        serde_json::to_vec(&Tokens {
            access_token: "new-access".into(),
            refresh_token: None,
            expires_at: now() + 86400,
        })
        .unwrap(),
    )
    .await
    .unwrap();
    let app = App::new(c).await.unwrap();
    let key = "w".repeat(32);
    let result = request(
        app.clone(),
        "POST",
        "/api/bulb/colour",
        &key,
        json!({"r":255,"g":0,"b":0}),
    )
    .await;
    assert_eq!(result.0, StatusCode::OK);
    assert_eq!(result.1["confirmed"], false);
    let sent = mock.commands.lock().await;
    assert_eq!(sent[0]["commands"][0]["capability"], "colorControl");
    assert_eq!(sent[0]["commands"][0]["arguments"][0]["hue"], 0.0);
    assert_eq!(sent[0]["commands"][2]["command"], "on");
    drop(sent);
    mock.reject.store(true, Ordering::SeqCst);
    let result = request(app, "POST", "/api/bulb/power", &key, json!({"on":false})).await;
    assert_eq!(result.0, StatusCode::BAD_GATEWAY);
    assert!(!result.1.to_string().contains("private upstream"));
    task.abort();
}

#[tokio::test]
async fn client_key_cannot_start_admin_oauth_and_invalid_state_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config(dir.path());
    c.oauth = Some(OAuthConfig {
        client_id: "own".into(),
        client_secret: "private".into(),
        redirect_uri: "http://localhost/oauth/callback".into(),
        authorize_url: default_auth_url(),
        token_url: default_token_url(),
    });
    let app = App::new(c).await.unwrap();
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/admin/oauth/start",
            &"k".repeat(32),
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, reply) = request(
        app.clone(),
        "POST",
        "/admin/oauth/start",
        &"a".repeat(32),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(reply["authorize_url"].as_str().unwrap().contains("state="));
    assert_eq!(
        request(
            app,
            "GET",
            "/oauth/callback?state=wrong&code=anything",
            "",
            Value::Null
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
