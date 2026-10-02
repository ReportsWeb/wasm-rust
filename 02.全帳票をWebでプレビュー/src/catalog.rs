use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Datelike, FixedOffset, Local};
use reports_web::PrintData;
use serde_json::{Map, Value, json};
use std::{cmp::Ordering, path::PathBuf};
use tokio_postgres::NoTls;

pub const SAMPLES: [(&str, &str); 8] = [
    ("quick-start", "あっという間に帳票出力"),
    ("multiples-of-ten", "10のサンプル"),
    ("postal", "郵便番号一覧（定義切替）"),
    ("estimate", "見積書（表紙＋明細）"),
    ("invoice", "請求書"),
    ("products", "商品大小分類（途中で小計）"),
    ("business-card", "名刺"),
    ("design-showcase", "デザイン機能見本"),
];

#[derive(Clone)]
pub struct SampleCatalog {
    resources: PathBuf,
    database_url: String,
}

impl SampleCatalog {
    pub fn new(resources: PathBuf, database_url: String) -> Self {
        Self {
            resources,
            database_url,
        }
    }

    pub async fn definition(&self, sample: &str) -> Result<Value> {
        assert_sample(sample)?;
        let file = if sample == "postal" {
            "postal-1.prepdj".to_owned()
        } else {
            format!("{sample}.prepdj")
        };
        self.load_definition(&file).await
    }

    pub async fn print_data(&self, sample: &str) -> Result<Value> {
        assert_sample(sample)?;
        let result = match sample {
            "multiples-of-ten" => self.multiples().await?,
            "postal" => self.postal().await?,
            "estimate" => self.estimate().await?,
            "invoice" => self.invoice().await?,
            "products" => self.products().await?,
            _ => self.simple(sample).await?,
        };
        result.to_value()
    }

    async fn load_definition(&self, name: &str) -> Result<Value> {
        let path = self.resources.join("definitions").join(name);
        let value: Value = serde_json::from_slice(
            &tokio::fs::read(&path)
                .await
                .with_context(|| format!("Cannot read {}", path.display()))?,
        )?;
        reports_web::assert_definition(&value)?;
        if value
            .get("CoordinateUnit")
            .and_then(Value::as_str)
            .unwrap_or("mm")
            != "mm"
        {
            bail!("Original definition must use mm")
        }
        Ok(value)
    }

    async fn simple(&self, sample: &str) -> Result<PrintData> {
        let d = self.definition(sample).await?;
        let mut p = PrintData::new();
        p.set_definition(&d)?.page_start()?;
        if sample == "quick-start" {
            p.set_value("Text2", "Webブラウザで作った\n印刷データです。")?;
        }
        p.page_end()?;
        Ok(p)
    }

    async fn multiples(&self) -> Result<PrintData> {
        let d = self.definition("multiples-of-ten").await?;
        let mut p = PrintData::new();
        p.set_definition(&d)?;
        for page in 1..=4 {
            p.page_start()?
                .set_value("日付", now(false))?
                .set_value("頁数", format!("Page - {page}"))?
                .set_value("フォントサイズ", "フォントサイズ\n 変更後")?
                .change_attributes("フォントサイズ", json!({"fontSize":12}), 0)?;
            // 2ページ目だけ、ページ上部の線「Line3」を非表示にする。空文字と drawing=false を指定する。
            if page == 2 {
                p.set_value_at("Line3", "", 0, false)?;
            }
            for line in 0..15 {
                let i = (page - 1) * 15 + line + 1;
                p.set_value_at("行番号", i, line, true)?
                    .set_value_at("10倍数", i * 10, line, true)?
                    .set_value_at("横線", "", line, true)?;
                // 100で割り切れる値だけ、この行の文字色を青にする。
                if (i * 10) % 100 == 0 { p.change_attributes("10倍数", json!({"foreground":"#FF0000FF"}), line)?; }
            }
            p.page_end()?;
        }
        Ok(p)
    }

    async fn postal(&self) -> Result<PrintData> {
        let rows = self.rows("postal", 1).await?;
        let first = self.load_definition("postal-1.prepdj").await?;
        let second = self.load_definition("postal-2.prepdj").await?;
        let mut p = PrintData::new();
        p.set_definition(&first)?;
        for (page, chunk) in rows.chunks(32).enumerate() {
            p.page_start_with(Some(if page < 5 { &first } else { &second }))?
                .set_value("ページ", format!("Page-{}", page + 1))?
                .set_value("日時", now(false))?;
            for (i, row) in chunk.iter().enumerate() {
                let v = values(row);
                p.set_value_at("郵便番号", at(&v, 0), i, true)?
                    .set_value_at("市区町村", at(&v, 1), i, true)?
                    .set_value_at("住所", at(&v, 2), i, true)?
                    .set_value_at("横罫線", "", i, true)?;
                if page >= 5 && i % 2 == 1 {
                    p.set_value_at("網掛け", "", i, true)?;
                }
            }
            if page < 5 {
                let v = values(&chunk[0]);
                p.set_value(
                    "QR",
                    format!("{} {}{}", at(&v, 0), at(&v, 1), at(&v, 2))
                        .trim()
                        .to_string(),
                )?;
            }
            p.page_end()?;
        }
        Ok(p)
    }

    async fn estimate(&self) -> Result<PrintData> {
        let headers = self.rows("estimate", 1).await?;
        let details = self.rows("estimate", 2).await?;
        let cover = self.load_definition("estimate-cover.prepdj").await?;
        let body = self.load_definition("estimate.prepdj").await?;
        let mut p = PrintData::new();
        p.set_definition(&cover)?;
        for h in &headers {
            p.page_start_with(Some(&cover))?
                .set_value("お客様名", s(h, "お客様名"))?
                .set_value("担当者名", s(h, "担当者名"))?
                .page_end()?
                .page_start_with(Some(&body))?
                .set_value("見積番号", s(h, "見積番号"))?
                .set_value("お客様名", s(h, "お客様名"))?
                .set_value("担当者名", s(h, "担当者名"))?
                .set_value("見積日", japanese_date(&s(h, "見積日")))?
                .set_value(
                    "ヘッダ合計",
                    format!("\\ {}", number(h.get("合計金額").unwrap_or(&Value::Null))),
                )?
                .set_value(
                    "消費税額",
                    number(h.get("消費税額").unwrap_or(&Value::Null)),
                )?
                .set_value(
                    "フッタ合計",
                    number(h.get("合計金額").unwrap_or(&Value::Null)),
                )?;
            for i in 0..=6 {
                for n in [
                    "品番白",
                    "品名白",
                    "数量白",
                    "単価白",
                    "金額白",
                    "品番青",
                    "品名青",
                    "数量青",
                    "単価青",
                    "金額青",
                ] {
                    p.set_value_at(n, "", i, true)?;
                }
            }
            for (i, r) in matching(&details, "見積番号", &s(h, "見積番号"))
                .into_iter()
                .enumerate()
            {
                p.set_value_at("品番", s(r, "品番"), i, true)?
                    .set_value_at("品名", s(r, "品名"), i, true)?
                    .set_value_at("数量", s(r, "数量"), i, true)?
                    .set_value_at(
                        "単価",
                        number(r.get("単価").unwrap_or(&Value::Null)),
                        i,
                        true,
                    )?
                    .set_value_at(
                        "金額",
                        number(r.get("金額").unwrap_or(&Value::Null)),
                        i,
                        true,
                    )?;
            }
            p.page_end()?;
        }
        Ok(p)
    }

    async fn invoice(&self) -> Result<PrintData> {
        let headers = self.rows("invoice", 1).await?;
        let details = self.rows("invoice", 2).await?;
        let d = self.definition("invoice").await?;
        let mut p = PrintData::new();
        p.set_definition(&d)?;
        let px = 96.0 / 25.4;
        for h in &headers {
            let current = matching(&details, "請求番号", &s(h, "請求番号"));
            let max_h = usize::max(4, repeat(object(&d, "hLine")?) - 1);
            let max_v = usize::max(1, repeat(object(&d, "vLine")?) - 1);
            p.page_start()?
                .set_value("txtNo", s(h, "請求番号"))?
                .set_value("txtCustomer", s(h, "お客様名"))?
                .set_value("txtDate", now(true))?
                .set_value("Image1", self.image_data("kakuin.png").await?)?;
            let adjust = [-5.0, 44.0, -20.0, -10.0, -9.0];
            let mut column_x = Vec::new();
            let mut next_x = 0.0;
            for j in 0..max_v {
                let base = object(&d, &format!("field{}", j + 1))?;
                let x = if j == 0 { f(base, "X") * px } else { next_x };
                column_x.push(x);
                next_x = x + (f(base, "Width") + adjust[j]) * px;
            }
            for i in 0..max_h {
                p.set_value_at("hLine", "", i, true)?
                    .set_value_at("LineRect", "", i, true)?;
                if i == 0 {
                    p.change_attributes("hLine", json!({"borderWidth":0.5*px}), i)?;
                }
                if i == 1 {
                    p.change_attributes("hLine", json!({"strokeStyle":"Double"}), i)?;
                }
                let color = if i == 0 {
                    "#FFFFDAB9"
                } else if i < max_h - 3 {
                    if i % 2 == 1 { "#FFFFFFFF" } else { "#FF87CEFA" }
                } else {
                    "#FFFFFFB4"
                };
                p.change_attributes("LineRect",json!({"background":color,"fillEnabled":true,"fillStyle":"Solid","borderColor":"#FFFFFFFF"}),i)?;
                for j in 0..max_v {
                    if j < 3 && i > current.len() {
                        continue;
                    }
                    let base = object(&d, &format!("field{}", j + 1))?;
                    p.set_value_at(&format!("field{}",j+1),"",i,true)?.change_attributes(&format!("field{}",j+1),json!({"x":column_x[j],"width":(f(base,"Width")+adjust[j])*px,"bold":i==0,"fontSize":if i==0 {f(base,"FontSizePt")} else {12.0},"horizontalAlignment":if i==0 {"Center"} else if j==1 {"Left"} else if j==0 {"Center"} else {"Right"}}),i)?;
                }
            }
            for j in 0..=max_v {
                p.set_value_at("vLine", "", j, true)?.change_attributes(
                    "vLine",
                    json!({"x":if j<max_v {column_x[j]} else {next_x}}),
                    j,
                )?;
                if j == 0 || j == max_v {
                    p.change_attributes("vLine", json!({"borderWidth":0.5*px}), j)?;
                }
            }
            for (j, label) in ["品番", "品名", "数量", "単価", "金額"].iter().enumerate()
            {
                p.set_value_at(&format!("field{}", j + 1), *label, 0, true)?;
            }
            let mut total = 0_i64;
            for (row, r) in current.into_iter().enumerate() {
                let row = row + 1;
                let amount = invoice_integer(&s(r, "数量"))?.checked_mul(invoice_integer(&s(r, "単価"))?).context("invoice amount overflow")?;
                total = total.checked_add(amount).context("invoice total overflow")?;
                p.set_value_at("field1", s(r, "品番"), row, true)?
                    .set_value_at("field2", s(r, "品名"), row, true)?
                    .set_value_at("field3", s(r, "数量"), row, true)?
                    .set_value_at(
                        "field4",
                        invoice_number(r.get("単価").unwrap_or(&Value::Null))?,
                        row,
                        true,
                    )?
                    .set_value_at("field5", invoice_number(&json!(amount))?, row, true)?;
            }
            let tax = total as f64 * 0.05;
            for (k, (label, amount)) in [("小計", total as f64), ("消費税", tax), ("合計", total as f64 + tax)]
                .into_iter()
                .enumerate()
            {
                let row = max_h - 3 + k;
                p.set_value_at("field4", label, row, true)?
                    .set_value_at("field5", invoice_number(&json!(amount))?, row, true)?
                    .change_attributes(
                        "field4",
                        json!({"fontSize":16,"bold":true,"horizontalAlignment":"Center"}),
                        row,
                    )?;
            }
            p.set_value("txtTotal", invoice_number(&json!(total as f64 + tax))?)?
                .change_attributes("hLine", json!({"strokeStyle":"Double"}), max_h - 3)?
                .set_value_at("hLine", "", max_h, true)?
                .change_attributes("hLine", json!({"borderWidth":0.5*px}), max_h)?
                .page_end()?;
        }
        Ok(p)
    }

    async fn products(&self) -> Result<PrintData> {
        let mut big = std::collections::HashMap::new();
        for r in self.rows("products", 1).await? {
            big.insert(s(&r, "大分類コード"), s(&r, "大分類名称"));
        }
        let mut small = std::collections::HashMap::new();
        for r in self.rows("products", 2).await? {
            small.insert(
                format!("{}:{}", s(&r, "大分類コード"), s(&r, "小分類コード")),
                s(&r, "小分類名称"),
            );
        }
        let mut stream: Vec<Map<String, Value>> = Vec::new();
        let (mut prev_big, mut prev_small) = (None::<String>, None::<String>);
        let (mut prev_big_name, mut prev_small_name) = (String::new(), String::new());
        let (mut big_count, mut small_count) = (0, 0);
        for r in self.rows("products", 3).await? {
            let big_key = s(&r, "大分類コード");
            let small_key = format!("{}:{}", big_key, s(&r, "小分類コード"));
            let bn = big.get(&big_key).cloned().unwrap_or_default();
            let sn = small
                .get(&small_key)
                .cloned()
                .unwrap_or_default();
            if prev_small.as_ref().is_some_and(|x| x != &small_key) {
                stream.push(subtotal(
                    "small",
                    &prev_small_name,
                    small_count,
                ));
                small_count = 0;
            }
            if prev_big.as_ref().is_some_and(|x| x != &big_key) {
                stream.push(subtotal("big", &prev_big_name, big_count));
                big_count = 0;
            }
            stream.push(map(&[
                (
                    "大分類",
                    if prev_big.as_deref() == Some(&big_key) {
                        ""
                    } else {
                        &bn
                    },
                ),
                (
                    "小分類",
                    if prev_small.as_deref() == Some(&small_key) {
                        ""
                    } else {
                        &sn
                    },
                ),
                ("品番", &s(&r, "品番")),
                ("品名", &s(&r, "品名")),
                ("kind", "detail"),
            ]));
            prev_big = Some(big_key);
            prev_small = Some(small_key);
            prev_big_name = bn;
            prev_small_name = sn;
            big_count += 1;
            small_count += 1;
        }
        if prev_small.is_some() {
            stream.push(subtotal("small", &prev_small_name, small_count));
        }
        if prev_big.is_some() {
            stream.push(subtotal("big", &prev_big_name, big_count));
        }
        let d = self.definition("products").await?;
        let mut p = PrintData::new();
        p.set_definition(&d)?;
        for chunk in stream.chunks(20) {
            p.page_start()?;
            for (i, r) in chunk.iter().enumerate() {
                for n in ["大分類", "小分類", "品番", "品名"] {
                    p.set_value_at(n, r[n].as_str().unwrap_or("").to_owned(), i, true)?;
                }
                for n in ["枠_大分類", "枠_小分類", "枠_品番", "枠_品名"] {
                    p.set_value_at(n, "", i, true)?;
                    if r["kind"] != "detail" {
                        p.change_attributes(n,json!({"background":if r["kind"]=="small"{"#FFFFFFE0"}else{"#FFFFB6C1"},"fillEnabled":true,"fillStyle":"Solid"}),i)?;
                    }
                }
            }
            p.page_end()?;
        }
        Ok(p)
    }

    async fn rows(&self, sample: &str, sheet: i32) -> Result<Vec<Value>> {
        let (client, connection) = tokio_postgres::connect(&self.database_url, NoTls)
            .await
            .context("PostgreSQL connection failed")?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let rows=client.query("SELECT row_data::text FROM reports_framework_rows WHERE sample_key=$1 AND sheet_no=$2 ORDER BY row_no",&[&sample,&sheet]).await?;
        let mut result = rows
            .into_iter()
            .map(|r| serde_json::from_str::<Value>(r.get::<_, &str>(0)))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if sample == "products" && sheet == 3 {
            result.sort_by(|a, b| {
                numeric(s(a, "大分類コード"), s(b, "大分類コード"))
                    .then_with(|| numeric(s(a, "小分類コード"), s(b, "小分類コード")))
            });
        }
        Ok(result)
    }

    async fn image_data(&self, name: &str) -> Result<String> {
        Ok(format!(
            "data:image/png;base64,{}",
            STANDARD.encode(tokio::fs::read(self.resources.join("images").join(name)).await?)
        ))
    }
}

fn assert_sample(s: &str) -> Result<()> {
    if SAMPLES.iter().any(|x| x.0 == s) {
        Ok(())
    } else {
        bail!("Unknown sample: {s}")
    }
}
fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .map(|x| {
            if let Some(s) = x.as_str() {
                s.to_owned()
            } else {
                x.to_string()
            }
        })
        .unwrap_or_default()
}
fn values(v: &Value) -> Vec<String> {
    v.as_object()
        .map(|m| {
            m.values()
                .map(|x| {
                    if let Some(s) = x.as_str() {
                        s.to_owned()
                    } else {
                        x.to_string()
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}
fn at(v: &[String], i: usize) -> String {
    v.get(i).cloned().unwrap_or_default()
}
fn matching<'a>(v: &'a [Value], k: &str, value: &str) -> Vec<&'a Value> {
    v.iter().filter(|x| s(x, k) == value).collect()
}
fn object<'a>(v: &'a Value, name: &str) -> Result<&'a Value> {
    v["Objects"]
        .as_array()
        .and_then(|a| a.iter().find(|x| s(x, "Name") == name))
        .context(format!("Missing report object: {name}"))
}
fn repeat(v: &Value) -> usize {
    v.get("Repeat")
        .or_else(|| v.get("RepeatCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize
}
fn f(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

// Reports.NET invoice: ToInt64 rounds operands to even; tax remains f64 until display.
fn invoice_integer(v: &str) -> Result<i64> {
 let n = v.parse::<f64>().context("invalid invoice numeric value")?.round_ties_even();
 if !n.is_finite() || n < -9223372036854775808.0 || n >= 9223372036854775808.0 { bail!("invoice Int64 overflow or non-finite value") }
 Ok(n as i64)
}
fn invoice_number(v: &Value) -> Result<String> {
 let n = match v { Value::String(s) => s.parse::<f64>().context("invalid invoice number")?, _ => v.as_f64().context("invalid invoice number")? };
 if !n.is_finite() { bail!("non-finite invoice number") }
 let digits = format!("{:.0}", n.round_ties_even().abs());
 let mut out = String::new();
 for (idx, ch) in digits.chars().enumerate() { if idx > 0 && (digits.len()-idx)%3 == 0 { out.push(','); } out.push(ch); }
 Ok(format!("{}{}", if n.is_sign_negative() { "-" } else { "" }, out))
}

#[cfg(test)]
mod invoice_numeric_tests {
 use super::*;
 #[test]
 fn reports_net_rounding_format_and_overflow_contract() {
  assert_eq!(invoice_integer("2.25").unwrap(),2);
  assert_eq!(invoice_integer("2.75").unwrap(),3);
  assert_eq!(invoice_integer("1901.5").unwrap(),1902);
  assert_eq!(invoice_number(&json!(1900.5)).unwrap(),"1,900");
  assert_eq!(invoice_number(&json!(1901.5)).unwrap(),"1,902");
  assert_eq!(invoice_number(&json!(0.5)).unwrap(),"0");
  assert_eq!(invoice_number(&json!(1.5)).unwrap(),"2");
  assert_eq!(invoice_number(&json!(-0.4)).unwrap(),"-0");
  assert!(invoice_integer("NaN").is_err());
  assert!(invoice_integer("9223372036854775808").is_err());
  assert!(i64::MAX.checked_mul(2).is_none());
  assert!(i64::MAX.checked_add(1).is_none());
 }
}

fn i(v: &str) -> i64 {
    v.parse().unwrap_or(0)
}
fn number(v: &Value) -> String {
    let n = v
        .as_i64()
        .or_else(|| v.as_str().and_then(|x| x.parse().ok()))
        .unwrap_or_else(|| v.as_f64().unwrap_or(0.0).round() as i64);
    let sign = if n < 0 { "-" } else { "" };
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (idx, ch) in digits.chars().enumerate() {
        if idx > 0 && (digits.len() - idx) % 3 == 0 {
            out.push(',')
        }
        out.push(ch)
    }
    format!("{sign}{out}")
}
fn japanese_date(v: &str) -> String {
    let p = v.get(..10).unwrap_or(v).split('-').collect::<Vec<_>>();
    if p.len() == 3 {
        format!(
            "{}年{}月{}日",
            p[0],
            p[1].parse::<u32>().unwrap_or(0),
            p[2].parse::<u32>().unwrap_or(0)
        )
    } else {
        v.to_owned()
    }
}
fn now(japanese: bool) -> String {
    let offset = FixedOffset::east_opt(9 * 3600).unwrap();
    let d: DateTime<FixedOffset> = Local::now().with_timezone(&offset);
    if japanese {
        format!("{}年{}月{}日", d.year(), d.month(), d.day())
    } else {
        d.format("%Y/%m/%d %H:%M:%S").to_string()
    }
}
fn subtotal(kind: &str, name: &str, count: usize) -> Map<String, Value> {
    map(&[
        ("大分類", ""),
        (
            "小分類",
            &format!(
                "{}({name})小計",
                if kind == "small" {
                    "小分類"
                } else {
                    "大分類"
                }
            ),
        ),
        ("品番", &format!("{count} 冊")),
        ("品名", ""),
        ("kind", kind),
    ])
}
fn map(values: &[(&str, &str)]) -> Map<String, Value> {
    values
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect()
}
fn numeric(a: String, b: String) -> Ordering {
    a.parse::<f64>()
        .unwrap_or(0.0)
        .partial_cmp(&b.parse::<f64>().unwrap_or(0.0))
        .unwrap_or(Ordering::Equal)
}
