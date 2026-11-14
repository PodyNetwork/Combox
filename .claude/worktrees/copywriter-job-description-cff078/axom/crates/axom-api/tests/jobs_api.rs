//! Requires DATABASE_URL. The public jobs board: list, view, apply.

use axom_api::{router, AppState};
use axom_core::Uuid;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

const PDF: &[u8] = b"%PDF-1.7\n1 0 obj << /Type /Catalog >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n";

/// POST an application as the jobs board does: a multipart form with the
/// details as JSON and the CV file.
async fn apply(app: &axum::Router, uri: &str, details: Value, cv: Option<(&str, &[u8])>) -> (StatusCode, Value) {
    let b = "XAXOMBOUNDARYX";
    let mut body = format!("--{b}\r\nContent-Disposition: form-data; name=\"application\"\r\n\r\n{details}\r\n").into_bytes();
    if let Some((name, bytes)) = cv {
        body.extend(format!("--{b}\r\nContent-Disposition: form-data; name=\"cv\"; filename=\"{name}\"\r\nContent-Type: application/pdf\r\n\r\n").as_bytes());
        body.extend(bytes);
        body.extend(b"\r\n");
    }
    body.extend(format!("--{b}--\r\n").as_bytes());
    let req = Request::builder().method("POST").uri(uri).header("content-type", format!("multipart/form-data; boundary={b}")).body(Body::from(body)).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(uri).header("content-type", "application/json");
    let req = req.body(body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty)).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[sqlx::test(migrator = "axom_core::MIGRATOR")]
async fn public_board_lists_open_jobs_and_takes_applications(pool: PgPool) {
    let job = Uuid::new_v4();
    sqlx::query("INSERT INTO axom.board_jobs (id, title, skills, rate_usd, kpis) VALUES ($1,'Brand Designer','{visual_design}',25,'{on_time_rate target 0.9}')")
        .bind(job)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO axom.board_jobs (id, title, rate_usd, status) VALUES ($1,'Old job',10,'closed')").bind(Uuid::new_v4()).execute(&pool).await.unwrap();
    let app = router(AppState::new(pool.clone()));

    // Only open jobs are listed, without sign-in.
    let (s, b) = call(&app, "GET", "/jobs", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(b["jobs"][0]["title"], "Brand Designer");
    let (s, _) = call(&app, "GET", &format!("/jobs/{job}"), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(&app, "GET", &format!("/jobs/{}", Uuid::new_v4()), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let good = json!({ "name": "Dana", "email": "dana@example.com", "skills": ["visual design"], "message": "Hi", "accept_privacy": true });
    let url = format!("/jobs/{job}/apply");
    let cv = Some(("../../Dana CV<script>.pdf", PDF));
    // A CV is required, and it must be a PDF of at most 5 MB.
    let (s, b) = apply(&app, &url, good.clone(), None).await;
    assert_eq!((s, b["error"].as_str()), (StatusCode::BAD_REQUEST, Some("please attach your CV as a PDF")));
    let (s, b) = apply(&app, &url, good.clone(), Some(("cv.pdf", b"MZ\x90\x00 not a pdf"))).await;
    assert_eq!((s, b["error"].as_str()), (StatusCode::BAD_REQUEST, Some("your CV must be a PDF file")));
    let mut big = PDF.to_vec();
    big.resize(5 * 1024 * 1024 + 1, b' ');
    let (s, b) = apply(&app, &url, good.clone(), Some(("big.pdf", &big))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    let mut huge = PDF.to_vec();
    huge.resize(7 * 1024 * 1024, b' ');
    assert_eq!(apply(&app, &url, good.clone(), Some(("huge.pdf", &huge))).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    // The old JSON form (a link, no file) is refused.
    assert_ne!(call(&app, "POST", &url, Some(good.clone())).await.0, StatusCode::CREATED);

    let (s, b) = apply(&app, &url, good.clone(), cv).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let application = b["application_id"].as_str().unwrap().to_string();
    let (s, _) = apply(&app, &url, good.clone(), cv).await;
    assert_eq!(s, StatusCode::CONFLICT);

    let (s, b) = apply(&app, &url, json!({ "name": "X", "email": "not-an-email", "accept_privacy": true }), cv).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    let (s, _) = apply(&app, &url, json!({ "name": "X", "email": "x@example.com" }), cv).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "privacy notice must be accepted");
    let (s, _) = apply(&app, &format!("/jobs/{}/apply", Uuid::new_v4()), json!({ "name": "X", "email": "x@example.com", "accept_privacy": true }), cv).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Stored as sent, under a safe file name.
    let (name, content): (String, Vec<u8>) = sqlx::query_as("SELECT filename, content FROM axom.board_application_cvs WHERE application_id = $1::uuid").bind(&application).fetch_one(&pool).await.unwrap();
    assert_eq!(name, "Dana CVscript.pdf");
    assert_eq!(content, PDF);

    let (skills,): (Vec<String>,) = sqlx::query_as("SELECT skills FROM axom.board_applications WHERE email = 'dana@example.com'").fetch_one(&pool).await.unwrap();
    assert_eq!(skills, vec!["visual_design".to_string()]);
}
