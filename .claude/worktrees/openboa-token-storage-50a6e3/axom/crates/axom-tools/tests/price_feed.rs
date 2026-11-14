//! Price feed against a local server: API key header, parsing, error mapping.

use axom_tools::price::{HttpPriceFeed, PriceFeed};
use axom_tools::ToolError;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use rust_decimal_macros::dec;

async fn server() -> String {
    let app = axum::Router::new().route(
        "/simple/price",
        axum::routing::get(|h: HeaderMap| async move {
            match h.get("x-cg-demo-api-key").and_then(|v| v.to_str().ok()) {
                Some("demo-key") => axum::Json(serde_json::json!({ "near": { "usd": 3.42 } })).into_response(),
                _ => (StatusCode::FORBIDDEN, "key required").into_response(),
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/simple/price", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

#[tokio::test]
async fn sends_api_key_and_parses_price() {
    let url = server().await;
    let without = HttpPriceFeed::new(&url, "/near/usd");
    assert!(matches!(without.quote().await, Err(ToolError::Permanent(_))), "403 without a key");
    let with = HttpPriceFeed::new(&url, "/near/usd").with_api_key("x-cg-demo-api-key", "demo-key");
    let q = with.quote().await.unwrap();
    assert_eq!(q.usd_per_near, dec!(3.42));
    assert!((chrono::Utc::now() - q.fetched_at).num_seconds() < 5);
}
