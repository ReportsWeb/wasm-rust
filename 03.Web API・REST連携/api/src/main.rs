mod catalog;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use catalog::{SAMPLES, SampleCatalog};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{env, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tower_http::services::{ServeDir, ServeFile};

const BASE: &str = "/demo/reports.web/samples/rust";
const MAX: usize = 32 * 1024 * 1024;

#[derive(Clone)]
struct AppState {
    catalog: SampleCatalog,
    resources: PathBuf,
    template: Arc<String>,
    preview_url: String,
    designer_url: String,
    asset_base: String,
    trusted_asset_bases: Arc<Vec<String>>,
    engine: reports_web::ReportsWebEngine,
    render_slot: Arc<Semaphore>,
}

#[derive(Deserialize, Default)]
struct Params {
    action: Option<String>,
    sample: Option<String>,
    name: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let resources = PathBuf::from(
        env::var("REPORTS_RESOURCE_ROOT").unwrap_or_else(|_| "/opt/reports/resources".into()),
    );
    let template_path = env::var("REPORTS_TEMPLATE").unwrap_or_else(|_| "public/index.html".into());
    let web_root = PathBuf::from(
        env::var("REPORTS_WEB_ROOT").unwrap_or_else(|_| "/opt/reports/web".into()),
    );
    let database_url = env::var("REPORTS_DB_URL").unwrap_or_else(|_| {
        "postgres://reports_web:reports-web-local-only@127.0.0.1:5434/reports_web_sample".into()
    });
    let port: u16 = env::var("PORT").unwrap_or_else(|_| "8094".into()).parse()?;
    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let state = AppState {
        catalog: SampleCatalog::new(resources.clone(), database_url),
        resources,
        template: Arc::new(tokio::fs::read_to_string(template_path).await?),
        preview_url: env::var("REPORTS_PREVIEW_URL")
            .unwrap_or_else(|_| "/demo/reports.web/preview/".into()),
        designer_url: env::var("REPORTS_DESIGNER_URL")
            .unwrap_or_else(|_| "/demo/reports.web/design/".into()),
        asset_base: env::var("REPORTS_PUBLIC_ASSET_BASE")
            .unwrap_or_else(|_| format!("http://127.0.0.1:{port}{BASE}/api?action=asset&name=")),
        trusted_asset_bases: Arc::new(
            env::var("REPORTS_TRUSTED_ASSET_BASES")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .collect(),
        ),
        engine: reports_web::ReportsWebEngine::with_timeout(
            &env::var("REPORTS_ENGINE_URL").unwrap_or_else(|_| "http://127.0.0.1:3107".into()),
            Duration::from_secs(90),
        )?,
        render_slot: Arc::new(Semaphore::new(1)),
    };
    let app = Router::new()
        .route("/", get(|| async { Redirect::temporary(&format!("{BASE}/")) }))
        .route("/health", get(health))
        .route(BASE, get(index))
        .route(&format!("{BASE}/"), get(index))
        .route(&format!("{BASE}/health"), get(health))
        .route(&format!("{BASE}/api"), get(api).post(server_pdf))
        .route(&format!("{BASE}/api.php"), get(api).post(server_pdf))
        .nest_service(
            "/demo/reports.web/preview",
            ServeDir::new(web_root.join("preview")),
        )
        .nest_service(
            "/demo/reports.web/design",
            ServeDir::new(web_root.join("design")),
        )
        .nest_service(
            "/demo/reports.web/assets",
            ServeDir::new(web_root.join("assets")),
        )
        .nest_service(
            "/demo/reports.web/barcode",
            ServeDir::new(web_root.join("barcode")),
        )
        .route_service(
            "/demo/reports.web/font-map.json",
            ServeFile::new(web_root.join("font-map.json")),
        )
        .route_service(
            "/demo/reports.web/fontmap.json",
            ServeFile::new(web_root.join("fontmap.json")),
        )
        .route_service(
            "/demo/reports.web/preview-help.html",
            ServeFile::new(web_root.join("preview-help.html")),
        )
        .layer(DefaultBodyLimit::max(MAX))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await?;
    println!("Reports Web Rust sample: http://127.0.0.1:{port}{BASE}/");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn health() -> Json<Value> {
    Json(json!({"status":"UP"}))
}

async fn index(
    State(s): State<AppState>,
    Query(p): Query<Params>,
) -> Result<Html<String>, AppError> {
    let selected = p
        .sample
        .as_deref()
        .filter(|x| SAMPLES.iter().any(|v| v.0 == *x))
        .unwrap_or("invoice");
    let options = SAMPLES
        .iter()
        .map(|(key, label)| {
            format!(
                "<option value=\"{key}\"{}>{label}</option>",
                if *key == selected { " selected" } else { "" }
            )
        })
        .collect::<String>();
    Ok(Html(
        s.template
            .replace("{{SAMPLES}}", &options)
            .replace("{{PREVIEW_URL}}", &html_attr(&s.preview_url))
            .replace(
                "{{DESIGNER_URL_JSON}}",
                &serde_json::to_string(&s.designer_url)?,
            ),
    ))
}

async fn api(State(s): State<AppState>, Query(p): Query<Params>) -> Result<Response, AppError> {
    let action = p.action.as_deref().unwrap_or("data");
    let sample = p.sample.as_deref().unwrap_or("invoice");
    match action {
        "catalog" => Ok(report_json(
            json!(
                SAMPLES
                    .iter()
                    .copied()
                    .collect::<std::collections::BTreeMap<_, _>>()
            ),
            None,
            "application/json",
        )),
        "asset" => {
            let name = p.name.as_deref().unwrap_or("");
            let bytes = asset(&s, name).await?;
            Ok(bytes_response(bytes, mime(name), None))
        }
        "definition" => {
            assert_sample(sample)?;
            let mut value = s.catalog.definition(sample).await?;
            externalize(&mut value, sample, &s.asset_base);
            inline_assets(&mut value, &s).await?;
            Ok(report_json(
                value,
                Some(&format!("{sample}.prepdj")),
                "application/vnd.pao.reports-definition+json",
            ))
        }
        "data" => {
            assert_sample(sample)?;
            let mut value = s.catalog.print_data(sample).await?;
            externalize_print_data(&mut value, sample, &s.asset_base);
            inline_assets(&mut value, &s).await?;
            Ok(report_json(
                value,
                Some(&format!("{sample}.prepej")),
                "application/vnd.pao.reports-printdata+json",
            ))
        }
        _ => Err(AppError::bad_request("未対応の操作です。")),
    }
}

async fn server_pdf(
    State(s): State<AppState>,
    Query(p): Query<Params>,
    body: Bytes,
) -> Result<Response, AppError> {
    if p.action.as_deref() != Some("server-pdf") {
        return Err(AppError::bad_request("未対応の操作です。"));
    }
    let sample = p.sample.as_deref().unwrap_or("invoice");
    assert_sample(sample)?;
    let _slot = s.render_slot.clone().try_acquire_owned().map_err(|_| {
        AppError(
            StatusCode::TOO_MANY_REQUESTS,
            "PDF作成中です。少し待ってからお試しください。".into(),
        )
    })?;
    let mut data: Value =
        serde_json::from_slice(&body).map_err(|_| AppError::bad_request("JSON形式が不正です。"))?;
    if !data
        .get("Pages")
        .and_then(Value::as_array)
        .is_some_and(|x| !x.is_empty())
        || data
            .get("Format")
            .is_some_and(|x| x != reports_web::FORMAT)
    {
        return Err(AppError::bad_request(
            "PREPEJ形式の印刷データではありません。",
        ));
    }
    inline_assets(&mut data, &s).await?;
    let payload = serde_json::to_vec(&data)?;
    if payload.len() > MAX {
        return Err(AppError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "展開後の印刷データが32MiBを超えます。".into(),
        ));
    }
    // reports-web: POST /render/pdf to the Reports.Web engine.
    let pdf = s.engine.render_pdf_json(payload).await.map_err(|_| {
        AppError(
            StatusCode::BAD_GATEWAY,
            "サーバーでPDFを作成できませんでした。".into(),
        )
    })?;
    let mut result = bytes_response(
        pdf,
        "application/pdf",
        Some(&format!("{}.pdf", safe_name(sample))),
    );
    result
        .headers_mut()
        .insert("X-Reports-Engine", "server-wasm".parse().unwrap());
    Ok(result)
}

fn report_json(value: Value, file: Option<&str>, mime: &str) -> Response {
    let bytes = serde_json::to_vec(&value).unwrap();
    bytes_response(bytes, mime, file)
}
fn bytes_response(bytes: impl Into<axum::body::Body>, mime: &str, file: Option<&str>) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, mime.parse().unwrap());
    headers.insert("X-Content-Type-Options", "nosniff".parse().unwrap());
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    if let Some(f) = file {
        headers.insert(
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"{f}\"").parse().unwrap(),
        );
    }
    (headers, bytes.into()).into_response()
}
fn assert_sample(sample: &str) -> Result<(), AppError> {
    if SAMPLES.iter().any(|v| v.0 == sample) {
        Ok(())
    } else {
        Err(AppError::bad_request("帳票を選択してください。"))
    }
}
fn safe_name(v: &str) -> String {
    v.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "_.-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}
fn html_attr(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn mime(name: &str) -> &'static str {
    if name.ends_with(".jpg") {
        "image/jpeg"
    } else {
        "image/png"
    }
}

async fn asset(s: &AppState, name: &str) -> Result<Vec<u8>, AppError> {
    if !matches!(name, "kakuin.png" | "estimate-header.jpg") {
        return Err(AppError::bad_request("未登録の画像資源です。"));
    }
    Ok(tokio::fs::read(s.resources.join("images").join(name)).await?)
}
fn externalize(v: &mut Value, sample: &str, base: &str) {
    if let Some(objects) = v.get_mut("Objects").and_then(Value::as_array_mut) {
        for o in objects {
            let name = o.get("Name").and_then(Value::as_str).unwrap_or("");
            let asset = if matches!(sample, "invoice" | "estimate") && name == "Image1" {
                Some("kakuin.png")
            } else if sample == "estimate" && name == "Image2" {
                Some("estimate-header.jpg")
            } else {
                None
            };
            if let Some(a) = asset {
                o["ImagePath"] = json!(format!("{base}{a}"));
                o["ImageDataBase64"] = json!("");
            }
        }
    }
}
fn externalize_print_data(v: &mut Value, sample: &str, base: &str) {
    externalize(&mut v["Definition"], sample, base);
    if let Some(pages) = v.get_mut("Pages").and_then(Value::as_array_mut) {
        for page in pages {
            externalize(&mut page["Definition"], sample, base);
            if sample == "invoice" {
                if let Some(values) = page.get_mut("Values").and_then(Value::as_array_mut) {
                    for value in values {
                        if value["Name"] == "Image1" {
                            value["Value"] = json!(format!("{base}kakuin.png"));
                        }
                    }
                }
            }
        }
    }
}

async fn inline_assets(v: &mut Value, s: &AppState) -> Result<(), AppError> {
    let own = s.asset_base.clone();
    let trusted = s.trusted_asset_bases.clone();
    inline_walk(v, s, &own, &trusted, 0).await
}
fn inline_walk<'a>(
    v: &'a mut Value,
    s: &'a AppState,
    own: &'a str,
    trusted: &'a [String],
    depth: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AppError>> + Send + 'a>> {
    Box::pin(async move {
        if depth > 100 {
            return Err(AppError::bad_request("印刷データの階層が深すぎます。"));
        }
        match v {
            Value::String(text) => {
                let name = std::iter::once(own)
                    .chain(trusted.iter().map(String::as_str))
                    .find_map(|base| text.strip_prefix(base))
                    .map(str::to_owned);
                if let Some(name) = name {
                    let bytes = asset(s, &name).await?;
                    *text = format!("data:{};base64,{}", mime(&name), STANDARD.encode(bytes));
                }
            }
            Value::Array(values) => {
                for value in values {
                    inline_walk(value, s, own, trusted, depth + 1).await?
                }
            }
            Value::Object(values) => {
                for (key, value) in values {
                    inline_walk(value, s, own, trusted, depth + 1).await?;
                    if key == "ImagePath"
                        && value
                            .as_str()
                            .is_some_and(|x| !x.is_empty() && !x.starts_with("data:"))
                    {
                        return Err(AppError::bad_request(
                            "外部画像は、このサンプルに登録した画像か埋め込み画像を使用してください。",
                        ));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    })
}

struct AppError(StatusCode, String);
impl AppError {
    fn bad_request(v: &str) -> Self {
        Self(StatusCode::BAD_REQUEST, v.into())
    }
}
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(value: E) -> Self {
        let e: anyhow::Error = value.into();
        Self(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
