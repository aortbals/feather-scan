//! Smart file names from an OpenAI-compatible chat completions endpoint.

use std::time::Duration;

use serde_json::{Value, json};

use crate::config::LlmConfig;
use crate::error::{Error, Result};

const MAX_TEXT: usize = 12_000;

fn prompt(today: &str) -> String {
    format!(
        "generate a title for this PDF in the format 'YYYY-MM-DD - <descriptive title up to 80 characters>'. \
         Use exactly one date: the document's own date if it has one, otherwise today's date ({today}). \
         Do not use special characters like colons, slashes, commas, apostrophes, or other punctuation, as this is a filename. \
         Dashes and spaces are fine. Reply with the title only."
    )
}

fn endpoint(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        return base.to_string();
    }
    // Bare host like https://api.openai.com: assume the usual /v1 prefix.
    let has_path = base
        .split("://")
        .nth(1)
        .is_some_and(|rest| rest.contains('/'));
    if has_path {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

/// Asks the model for a title. Returns a filename-safe title (no extension).
pub fn generate(cfg: &LlmConfig, text: &str, today: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::Llm("no text recognized".into()));
    }
    let mut cut = text.len().min(MAX_TEXT);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let body = json!({
        "model": cfg.model,
        "messages": [
            {"role": "system", "content": prompt(today)},
            {"role": "user", "content": &text[..cut]},
        ],
    });
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .build(),
        )
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.post(&endpoint(&cfg.base_url));
    if let Some(key) = cfg.api_key.as_deref().filter(|k| !k.is_empty()) {
        req = req.header("Authorization", &format!("Bearer {key}"));
    }
    let mut resp = req
        .send_json(&body)
        .map_err(|e| Error::Llm(e.to_string()))?;
    let status = resp.status().as_u16();
    let v: Value = resp
        .body_mut()
        .read_json()
        .map_err(|e| Error::Llm(format!("HTTP {status}: {e}")))?;
    if status >= 300 {
        let msg = v["error"]["message"].as_str().unwrap_or("request failed");
        return Err(Error::Llm(format!("HTTP {status}: {msg}")));
    }
    let raw = v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default();
    let title = sanitize(raw, today);
    if title.len() < 12 {
        return Err(Error::Llm(format!("unusable title {raw:?}")));
    }
    Ok(title)
}

/// Makes a model reply safe as a file name and makes sure it starts with a date.
pub fn sanitize(raw: &str, today: &str) -> String {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let line = line.trim_matches(|c| c == '"' || c == '\'' || c == '`' || c == '*');
    let line = line.strip_suffix(".pdf").unwrap_or(line);
    let mut out = String::with_capacity(line.len());
    for c in line.chars() {
        let c = match c {
            '/' | '\\' | ':' | '|' => '-',
            ',' | '\'' | '"' | '?' | '*' | '<' | '>' | ';' | '!' | '`' | '’' | '‘' | '“' | '”' => {
                continue;
            }
            c if c.is_control() => ' ',
            c => c,
        };
        if c == ' ' && out.ends_with(' ') {
            continue;
        }
        out.push(c);
    }
    let mut out = out.trim().trim_matches('.').trim().to_string();
    if !starts_with_date(&out) {
        out = format!("{today} - {out}");
    }
    // Always "YYYY-MM-DD - Title", however the model separated them.
    let rest = out[10..].trim_start_matches([' ', '-', '_', '–', '—']).trim().to_string();
    out = if rest.is_empty() { out[..10].to_string() } else { format!("{} - {rest}", &out[..10]) };
    // "2026-08-17 - 2026-11-01 - Title": keep only the first date.
    while out.len() > 13 && starts_with_date(&out[13..]) && out[10..13] == *" - " {
        out.replace_range(13..26.min(out.len()), "");
        out = out.replacen("  ", " ", 1);
    }
    // Keep names a sensible length (date + 80 chars of title, plus slack).
    if out.chars().count() > 110 {
        out = out.chars().take(110).collect::<String>().trim().to_string();
    }
    out
}

fn starts_with_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 10
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes() {
        assert_eq!(
            sanitize(
                "\"2026-10-15 - Electric Bill: Acme Power, Account 123\"\n",
                "2026-11-01"
            ),
            "2026-10-15 - Electric Bill- Acme Power Account 123"
        );
        assert_eq!(
            sanitize("Electric bill", "2026-11-01"),
            "2026-11-01 - Electric bill"
        );
        assert_eq!(
            sanitize("2026-08-17 - 2026-11-01 - Tax notice", "2026-11-01"),
            "2026-08-17 - Tax notice"
        );
    }

    #[test]
    fn endpoints() {
        assert_eq!(
            endpoint("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint("https://api.openai.com"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint("http://localhost:11434/v1/"),
            "http://localhost:11434/v1/chat/completions"
        );
    }
}
