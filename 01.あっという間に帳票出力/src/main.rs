use axum::{body::Bytes, extract::State, http::{header, StatusCode}, response::{Html, IntoResponse}, routing::{get, post}, Router};
use reports_web::{PrintData, ReportsWebEngine};
use std::{fs, sync::Arc};
use tower_http::services::ServeDir;

#[derive(Clone)] struct AppState { data: Arc<String>, engine: ReportsWebEngine }

fn create_print_data() -> anyhow::Result<String> {
 let definition = serde_json::from_str(&fs::read_to_string("definition/quick-report.prepdj")?)?;
 let mut report = PrintData::new(); report.set_definition(&definition)?; report.page_start()?;
 report.set_value("Title", "あっという間に帳票出力")?;
 report.set_value("CustomerName", "株式会社パオ")?; report.page_end()?;
 report.to_json(false)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
 let state = AppState { data: Arc::new(create_print_data()?), engine: ReportsWebEngine::new(&std::env::var("REPORTS_ENGINE_URL").unwrap_or_else(|_| "http://engine:3107".into()))? };
 let app = Router::new().route("/", get(|| async { Html(fs::read_to_string("/app/index.html").unwrap()) }))
  .route("/print-data", get(|State(s): State<AppState>| async move { ([(header::CONTENT_TYPE, "application/json")], (*s.data).clone()) }))
  .route("/pdf", post(pdf)).nest_service("/reports.web", ServeDir::new("/app/reports.web"))
  .nest_service("/demo/reports.web", ServeDir::new("/app/reports.web")).with_state(state);
 let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?; axum::serve(listener, app).await?; Ok(())
}

async fn pdf(State(s): State<AppState>, body: Bytes) -> impl IntoResponse {
 // reports-web: POST /render/pdf to the Reports.Web engine.
 match s.engine.render_pdf_json(body.to_vec()).await {
  Ok(pdf) => ([(header::CONTENT_TYPE, "application/pdf")], pdf).into_response(),
  Err(e) => (StatusCode::from_u16(e.status.unwrap_or(502)).unwrap_or(StatusCode::BAD_GATEWAY), e.message).into_response()
 }
}
