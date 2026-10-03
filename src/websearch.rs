//! Поиск в интернете без API-ключей: DuckDuckGo HTML + чтение страниц.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
}

async fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .unwrap_or_default()
}

/// Поиск через DuckDuckGo Lite. Без ключей, без капчи для 1-2 запросов.
pub async fn search(query: &str) -> Result<SearchResponse, String> {
    let client = client().await;
    let resp = client
        .post("https://lite.duckduckgo.com/lite/")
        .form(&[("q", query)])
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
        )
        .send()
        .await
        .map_err(|e| format!("нет доступа к поиску: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("поиск вернул статус {}", resp.status()));
    }

    let html = resp.text().await.map_err(|e| e.to_string())?;
    let results = parse_ddg_lite(&html);
    Ok(SearchResponse { results })
}

fn text_of(node: &scraper::ElementRef) -> String {
    node.text().collect::<String>().trim().to_string()
}

fn parse_ddg_lite(html: &str) -> Vec<SearchResult> {
    let doc = scraper::Html::parse_document(html);
    // DDG lite: таблица со ссылками class="result-link"
    let link_sel = match scraper::Selector::parse("a.result-link") {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let snippet_sel = scraper::Selector::parse("td.result-snippet").ok();
    let mut results: Vec<SearchResult> = vec![];
    let links: Vec<_> = doc.select(&link_sel).collect();
    let snippets: Vec<_> = match &snippet_sel {
        Some(s) => doc.select(s).collect(),
        None => vec![],
    };
    for (i, a) in links.into_iter().enumerate().take(6) {
        let title = text_of(&a);
        let href = a.value().attr("href").unwrap_or_default().to_string();
        let url = href
            .split("uddg=")
            .nth(1)
            .and_then(|s| s.split('&').next())
            .map(percent_decode)
            .unwrap_or(href);
        let snippet = snippets
            .get(i)
            .map(|s| text_of(s))
            .unwrap_or_default();
        if !title.is_empty() && url.starts_with("http") {
            results.push(SearchResult { title, url, snippet });
        }
    }
    results
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
