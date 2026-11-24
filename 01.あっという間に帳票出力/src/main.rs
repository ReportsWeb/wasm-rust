use axum::{body::Bytes, extract::{DefaultBodyLimit, State}, http::{header, StatusCode}, response::{Html, IntoResponse}, routing::{get, post}, Router};
use reports_web::PrintData;
use std::{fs, path::{Path, PathBuf}, sync::Arc};
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
 if !Path::new("definition/quick-report.prepdj").is_file() { println!("definition/quick-report.prepdj が見つかりません。このフォルダー（01.あっという間に帳票出力）で起動してください。"); return Ok(()); }
 let Some(reports_web) = local_run::find_reports_web() else { println!("Runtime/reports.web が見つかりません。ZIP を展開したフォルダーの並びのまま動かしてください。"); return Ok(()); };
 let in_docker = local_run::in_docker();
 let index = if in_docker { PathBuf::from("/app/index.html") } else { PathBuf::from("index.html") };
 let state = AppState { data: Arc::new(create_print_data()?), engine: local_run::prepare_engine(&reports_web).await,
  // Nodeの既定keep-alive切断（5秒）より先に待機接続を破棄し、再利用との競合を防ぐ。
  client: reqwest::Client::builder().pool_idle_timeout(std::time::Duration::from_secs(4)).connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(90)).build()? };
 let app = Router::new().route("/", get(move || async move { Html(fs::read_to_string(&index).unwrap()) }))
  .route("/print-data", get(|State(s): State<AppState>| async move { ([(header::CONTENT_TYPE, "application/json")], (*s.data).clone()) }))
  .route("/pdf", post(pdf)).nest_service("/reports.web", ServeDir::new(&reports_web))
  .nest_service("/demo/reports.web", ServeDir::new(&reports_web)).layer(DefaultBodyLimit::max(64 * 1024 * 1024)).with_state(state);
 // Docker の中では 8080 番。外（cargo run）では 127.0.0.1 の 19240 番（compose と同じ番号）。
 let port = std::env::var("PORT").unwrap_or_else(|_| if in_docker { "8080".into() } else { "19240".into() });
 let listener = tokio::net::TcpListener::bind(if in_docker { format!("0.0.0.0:{port}") } else { format!("127.0.0.1:{port}") }).await?;
 if !in_docker { let url = format!("http://127.0.0.1:{port}/"); println!("見本の帳票：{url}（止めるときは Ctrl+C）"); local_run::open_browser(&url); }
 axum::serve(listener, app).await?; Ok(())
}

// エンジンへの要求は90秒で打ち切る。止まっていれば502、時間切れは504。例外の文字列は応答へ出さない。
fn engine_failure(e: reqwest::Error) -> axum::response::Response {
 eprintln!("PDF engine request failed (timeout={})", e.is_timeout());
 let (status, message) = if e.is_timeout() { (StatusCode::GATEWAY_TIMEOUT, "PDF作成エンジンが時間内に応答しませんでした。") } else { (StatusCode::BAD_GATEWAY, "PDF作成エンジンに接続できませんでした。") };
 (status, [(header::CONTENT_TYPE, "application/json; charset=utf-8")], serde_json::json!({ "error": message }).to_string()).into_response()
}

async fn pdf(State(s): State<AppState>, body: Result<Bytes, axum::extract::rejection::BytesRejection>) -> impl IntoResponse {
 let body = match body {
  Ok(bytes) => bytes,
  Err(error) => {
   let status = error.status();
   let message = if status == StatusCode::PAYLOAD_TOO_LARGE { "印刷データは64MiB以内にしてください。" } else { "要求の本文を読み込めません。" };
   return (status, [(header::CONTENT_TYPE, "application/json; charset=utf-8")], serde_json::json!({"error":message}).to_string()).into_response();
  }
 };
 match reports_web::ReportsWebEngine::new(&s.engine).unwrap().render_pdf_json(body.to_vec()).await {
  Ok(pdf) => ([(header::CONTENT_TYPE, "application/pdf")], pdf).into_response(),
  Err(_) => (StatusCode::BAD_GATEWAY, "PDF engine request failed").into_response()
 }
}

// Docker を使わずに `cargo run` だけで動かすための準備（Docker の中では何もしない）。
// - 製品（Runtime/reports.web）は、ZIP の並びのまま ../../../Runtime/reports.web にある。
// - PDF を作るエンジンは、REPORTS_ENGINE_URL が無ければ 127.0.0.1:3107。居なければ、PC の Node.js で子として起動する。
mod local_run {
 use std::{io::IsTerminal, path::{Path, PathBuf}, process::{Command, Stdio}, time::Duration};

 pub fn in_docker() -> bool { Path::new("/app/reports.web/design/index.html").is_file() }

 pub fn find_reports_web() -> Option<PathBuf> {
  if let Ok(configured) = std::env::var("REPORTS_WEB_ROOT") { if !configured.is_empty() { return Some(PathBuf::from(configured)); } }
  if in_docker() { return Some(PathBuf::from("/app/reports.web")); }
  let mut dir = std::env::current_dir().ok()?;
  loop {
   let candidate = dir.join("Runtime").join("reports.web");
   if candidate.join("design").join("index.html").is_file() { return Some(candidate); }
   if !dir.pop() { return None; }
  }
 }

 async fn alive(url: &str) -> bool {
  let Ok(client) = reqwest::Client::builder().timeout(Duration::from_millis(1500)).build() else { return false };
  matches!(client.get(format!("{url}/health")).send().await, Ok(r) if r.status().is_success())
 }

 fn find_node() -> Option<PathBuf> {
  let name = if cfg!(windows) { "node.exe" } else { "node" };
  std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(name)).find(|path| path.is_file())
 }

 pub async fn prepare_engine(reports_web: &Path) -> String {
  if let Ok(configured) = std::env::var("REPORTS_ENGINE_URL") { if !configured.is_empty() { return configured.trim_end_matches('/').to_string(); } }
  if in_docker() { return "http://engine:3107".into(); }
  let url = "http://127.0.0.1:3107".to_string();
  if alive(&url).await { return url; }
  let Some(node) = find_node() else {
   println!("PDF の保存には Node.js か Docker が要ります（https://nodejs.org/）。プレビューはこのまま使えます。");
   return url;
  };
  let server = reports_web.join("server").join("server.mjs");
  let started = Command::new(node).arg("--max-old-space-size=1024").arg(&server)
   .env("REPORTS_ENGINE_ROOT", server.parent().unwrap_or(reports_web)).env("REPORTS_ENGINE_PORT", "3107").env("REPORTS_ENGINE_HOST", "127.0.0.1")
   .stdout(Stdio::null()).stderr(Stdio::null()).spawn();
  if let Ok(mut child) = started {
   for _ in 0..60 {
    // エンジンは同じ端末の子なので、Ctrl+C で一緒に止まる。
    if alive(&url).await { return url; }
    if matches!(child.try_wait(), Ok(Some(_))) { break; }
    std::thread::sleep(Duration::from_millis(500));
   }
  }
  println!("PDF 用のエンジンを起動できませんでした（3107 番が使用中かもしれません）。プレビューはこのまま使えます。");
  url
 }

 // 端末から起動したときだけ、ブラウザーで開く（REPORTS_NO_BROWSER=1 で止められる）。
 pub fn open_browser(url: &str) {
  if !std::io::stdout().is_terminal() || std::env::var_os("REPORTS_NO_BROWSER").is_some() { return; }
  let mut command = if cfg!(windows) { let mut c = Command::new("rundll32"); c.arg("url.dll,FileProtocolHandler"); c }
   else if cfg!(target_os = "macos") { Command::new("open") } else { Command::new("xdg-open") };
  let _ = command.arg(url).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
 }
}
