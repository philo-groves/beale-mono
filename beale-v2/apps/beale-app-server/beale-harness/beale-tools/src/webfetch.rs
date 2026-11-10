//! Bounded, text-only web retrieval. HTML is converted to text; scripts are never executed.

use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, Url, header, redirect::Policy};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

use crate::{ToolOutcome, success};

const MAX_RESPONSE_BYTES: usize = 2_000_000;
const MAX_TEXT_BYTES: usize = 128_000;
const SEARCH_URL: &str = "https://html.duckduckgo.com/html/";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WebFetchArgs {
    query: Option<String>,
    url: Option<String>,
}

enum Request {
    Search(String),
    Page(Url),
}

fn request(args: Value) -> Result<Request> {
    let args: WebFetchArgs = serde_json::from_value(args).context("invalid webfetch arguments")?;
    match (args.query, args.url) {
        (Some(query), None) => {
            let query = query.trim();
            ensure!(
                !query.is_empty() && query.len() <= 500,
                "invalid search query"
            );
            Ok(Request::Search(query.to_owned()))
        }
        (None, Some(url)) => {
            ensure!(
                !url.is_empty() && url.len() <= 4096 && url == url.trim(),
                "invalid URL"
            );
            let url = Url::parse(&url).context("invalid URL")?;
            validate_url(&url)?;
            Ok(Request::Page(url))
        }
        _ => bail!("supply exactly one of query or url"),
    }
}

fn validate_url(url: &Url) -> Result<()> {
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "only HTTP(S) URLs are supported"
    );
    ensure!(url.host_str().is_some(), "URL has no host");
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "URL credentials are unavailable"
    );
    Ok(())
}

pub(super) async fn fetch(args: &Value) -> Result<ToolOutcome> {
    let request = request(args.clone())?;
    let (url, label) = match request {
        Request::Search(query) => {
            let mut url = Url::parse(SEARCH_URL)?;
            url.query_pairs_mut().append_pair("q", &query);
            (url, format!("Search: {query}"))
        }
        Request::Page(url) => (url, "Page".to_owned()),
    };
    let client = Client::builder()
        .redirect(Policy::limited(5))
        .timeout(Duration::from_secs(20))
        .user_agent("Beale/2 webfetch")
        .build()
        .context("create web client")?;
    let mut response = client.get(url).send().await.context("fetch web content")?;
    ensure!(
        response.status().is_success(),
        "web request returned {}",
        response.status()
    );
    validate_url(response.url())?;
    let final_url = response.url().clone();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let html =
        content_type.starts_with("text/html") || content_type.starts_with("application/xhtml+xml");
    ensure!(
        html || content_type.starts_with("text/plain")
            || content_type.starts_with("application/json")
            || content_type.starts_with("application/xml")
            || content_type.starts_with("text/xml")
            || content_type.is_empty(),
        "web content is not supported text"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.context("read web content")? {
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_RESPONSE_BYTES,
            "web response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    let source = String::from_utf8_lossy(&bytes);
    let text = if html || (content_type.is_empty() && source.trim_start().starts_with('<')) {
        html_to_text(&source, &final_url)
    } else {
        bounded(&source, MAX_TEXT_BYTES)
    };
    ensure!(!text.trim().is_empty(), "web page has no readable text");
    Ok(success(format!("{label}\nURL: {final_url}\n\n{text}")))
}

fn bounded(value: &str, limit: usize) -> String {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut output = value[..end].to_owned();
    if end < value.len() {
        output.push_str("\n[content truncated]");
    }
    output
}

fn decode_entities(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        output.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        let Some(end) = rest.find(';').filter(|end| *end <= 12) else {
            output.push('&');
            continue;
        };
        let entity = &rest[..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                u32::from_str_radix(&entity[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if entity.starts_with('#') => {
                entity[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        if let Some(character) = decoded {
            output.push(character);
            rest = &rest[end + 1..];
        } else {
            output.push('&');
        }
    }
    output.push_str(rest);
    output
}

fn tag_end(input: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, byte) in input.as_bytes()[start..].iter().enumerate() {
        match (*byte, quote) {
            (b'\'' | b'"', None) => quote = Some(*byte),
            (value, Some(open)) if value == open => quote = None,
            (b'>', None) => return Some(start + offset),
            _ => {}
        }
    }
    None
}

fn attribute(tag: &str, wanted: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        while at < bytes.len() && (bytes[at].is_ascii_whitespace() || bytes[at] == b'/') {
            at += 1;
        }
        let start = at;
        while at < bytes.len()
            && !bytes[at].is_ascii_whitespace()
            && !matches!(bytes[at], b'=' | b'/' | b'>')
        {
            at += 1;
        }
        if start == at {
            at += 1;
            continue;
        }
        let name = &tag[start..at];
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= bytes.len() || bytes[at] != b'=' {
            continue;
        }
        at += 1;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let value = if at < bytes.len() && matches!(bytes[at], b'\'' | b'"') {
            let quote = bytes[at];
            at += 1;
            let begin = at;
            while at < bytes.len() && bytes[at] != quote {
                at += 1;
            }
            let value = &tag[begin..at];
            if at < bytes.len() {
                at += 1;
            }
            value
        } else {
            let begin = at;
            while at < bytes.len() && !bytes[at].is_ascii_whitespace() && bytes[at] != b'>' {
                at += 1;
            }
            &tag[begin..at]
        };
        if name.eq_ignore_ascii_case(wanted) {
            return Some(decode_entities(value));
        }
    }
    None
}

fn safe_link(base: &Url, href: &str) -> Option<Url> {
    let url = base.join(href).ok()?;
    validate_url(&url).ok()?;
    if url
        .host_str()
        .is_some_and(|host| host.ends_with("duckduckgo.com"))
        && let Some(destination) = url
            .query_pairs()
            .find(|(key, _)| key == "uddg")
            .map(|(_, value)| value.to_string())
        && let Ok(destination) = Url::parse(&destination)
        && validate_url(&destination).is_ok()
    {
        return Some(destination);
    }
    Some(url)
}

fn line_break(output: &mut String) {
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
}

fn html_to_text(input: &str, base: &Url) -> String {
    let mut output = String::new();
    let mut at = 0;
    let mut ignored = Vec::<String>::new();
    let mut anchor: Option<(usize, Url)> = None;
    while at < input.len() && output.len() < MAX_TEXT_BYTES {
        let Some(relative) = input[at..].find('<') else {
            if ignored.is_empty() {
                output.push_str(&decode_entities(&input[at..]));
            }
            break;
        };
        let start = at + relative;
        if ignored.is_empty() {
            output.push_str(&decode_entities(&input[at..start]));
        }
        if input[start..].starts_with("<!--") {
            at = input[start + 4..]
                .find("-->")
                .map_or(input.len(), |end| start + 4 + end + 3);
            continue;
        }
        let Some(end) = tag_end(input, start + 1) else {
            break;
        };
        let raw = input[start + 1..end].trim();
        at = end + 1;
        if raw.starts_with(['!', '?']) {
            continue;
        }
        let closing = raw.starts_with('/');
        let tag = raw
            .trim_start_matches('/')
            .split(|c: char| c.is_ascii_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if tag.is_empty() {
            continue;
        }
        let hidden = matches!(
            tag.as_str(),
            "script" | "style" | "template" | "noscript" | "svg" | "iframe" | "object"
        );
        if closing {
            if ignored.last().is_some_and(|name| name == &tag) {
                ignored.pop();
                continue;
            }
            if !ignored.is_empty() {
                continue;
            }
            if tag == "a"
                && let Some((start, url)) = anchor.take()
                && output.len() > start
            {
                output.push_str(&format!(" ({url})"));
            }
        } else {
            if hidden {
                ignored.push(tag);
                continue;
            }
            if !ignored.is_empty() {
                continue;
            }
            if tag == "a" {
                anchor = attribute(raw, "href")
                    .and_then(|href| safe_link(base, &href))
                    .map(|url| (output.len(), url));
            }
        }
        if matches!(
            tag.as_str(),
            "br" | "p"
                | "div"
                | "li"
                | "tr"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "section"
                | "article"
                | "header"
                | "footer"
                | "main"
        ) {
            line_break(&mut output);
        }
    }
    let mut normalized = String::new();
    for line in output.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !line.is_empty() {
            if !normalized.is_empty() {
                normalized.push('\n');
            }
            normalized.push_str(&line);
        }
    }
    bounded(&normalized, MAX_TEXT_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn requires_one_bounded_web_request() {
        assert!(matches!(
            request(json!({"query":"example research"})).unwrap(),
            Request::Search(_)
        ));
        assert!(matches!(
            request(json!({"url":"https://example.test/a"})).unwrap(),
            Request::Page(_)
        ));
        for args in [
            json!({}),
            json!({"query":"a","url":"https://example.test"}),
            json!({"url":"javascript:alert(1)"}),
            json!({"url":"file:///private"}),
            json!({"url":"https://user:pass@example.test"}),
            json!({"query":"a","extra":true}),
        ] {
            assert!(request(args).is_err());
        }
    }

    #[test]
    fn extracts_text_links_and_discards_active_html() {
        let base = Url::parse("https://example.test/a").unwrap();
        let text = html_to_text(
            "<title>Example &amp; Research</title><script>secret()</script><style>.hidden {}</style><h1>Guide</h1><p>Open <a href='/docs?q=1&amp;x=2'>the docs</a> or <a href='javascript:alert(1)'>skip</a>.</p>",
            &base,
        );
        assert!(text.contains("Example & Research"));
        assert!(text.contains("the docs (https://example.test/docs?q=1&x=2)"));
        assert!(!text.contains("secret()"));
        assert!(!text.contains("hidden"));
        assert!(!text.contains("javascript:"));
    }

    #[test]
    fn decodes_search_result_links() {
        let base = Url::parse("https://html.duckduckgo.com/html/").unwrap();
        let text = html_to_text(
            "<a class='result__a' href='//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.test%2Fpaper'>Example paper</a>",
            &base,
        );
        assert_eq!(text, "Example paper (https://example.test/paper)");
    }

    #[tokio::test]
    #[cfg_attr(
        windows,
        ignore = "restricted Windows sandbox denies loopback client connections"
    )]
    async fn fetches_local_http_page_as_text_without_script() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let count = stream.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..count]).starts_with("GET /guide HTTP/1.1"));
            let body = "<h1>Example guide</h1><script>run()</script><p>Safe text.</p>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let output = fetch(&json!({"url":format!("http://{address}/guide")}))
            .await
            .unwrap()
            .output;
        server.await.unwrap();
        assert!(output.contains("Example guide"));
        assert!(output.contains("Safe text."));
        assert!(!output.contains("run()"));
    }
}
