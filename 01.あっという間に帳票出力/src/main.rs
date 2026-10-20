use axum::{body::Bytes, extract::{DefaultBodyLimit, State}, http::{header, StatusCode}, response::{Html, IntoResponse}, routing::{get, post}, Router};
use reports_web::PrintData;
use std::{fs, sync::Arc};
use tower_http::services::ServeDir;

#[derive(Clone)] struct AppState { data: Arc<String>, engine: String, client: reqwest::Client }

fn create_print_data() -> anyhow::Result<String> {
 let definition = serde_json::from_str(&fs::read_to_string("definition/quick-report.prepdj")?)?;
 let mut report = PrintData::new(); report.set_definition(&definition)?; report.page_start()?;
 report.set_value("Title", "あっという間に帳票出力")?;
 report.set_value("CustomerName", "株式会社パオ")?; report.page_end()?;
 report.to_json(false)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
 // reports-web 1.0 のHTTPクライアントは32MiB固定のため、この64MiBの中継には直接HTTPを使う。
 let client=reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(90)).redirect(reqwest::redirect::Policy::none()).build()?;
 let state = AppState { data: Arc::new(create_print_data()?), engine: std::env::var("REPORTS_ENGINE_URL").unwrap_or_else(|_| "http://engine:3107".into()), client };
 let app = Router::new().route("/", get(|| async { Html(fs::read_to_string("/app/index.html").unwrap()) }))
  .route("/print-data", get(|State(s): State<AppState>| async move { ([(header::CONTENT_TYPE, "application/json")], (*s.data).clone()) }))
  .route("/pdf", post(pdf)).nest_service("/reports.web", ServeDir::new("/app/reports.web"))
  .nest_service("/demo/reports.web", ServeDir::new("/app/reports.web")).layer(DefaultBodyLimit::max(64 * 1024 * 1024)).with_state(state);
 let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?; axum::serve(listener, app).await?; Ok(())
}

async fn pdf(State(s): State<AppState>, body: Bytes) -> impl IntoResponse {
 match s.client.post(format!("{}/render/pdf",s.engine.trim_end_matches('/'))).header("content-type","application/json").body(body).send().await {
  Ok(response) => { let status=response.status();let mut headers=axum::http::HeaderMap::new();
   for name in ["content-type","x-reports-image-diagnostics"] {if let Some(value)=response.headers().get(name){headers.insert(axum::http::HeaderName::from_static(name),value.clone());}}
   match response.bytes().await {Ok(bytes)=>(status,headers,bytes).into_response(),Err(error)=>(StatusCode::BAD_GATEWAY,error.to_string()).into_response()}
  },
  Err(error)=>(StatusCode::BAD_GATEWAY,error.to_string()).into_response()
 }
}
