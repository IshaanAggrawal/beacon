//! Optional LLM layer: turns the evidence JSON into a two-sentence note.
//!
//! The LLM never decides anything. It only rewords evidence that the Rust
//! code already computed, and if it's unavailable, slow, rate-limited or
//! says something that contradicts the verdict, we use the template instead.
//! The binary works the same with or without it.

use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

use super::evidence::Evidence;
use super::explain;
use super::verdict::Tier;

const DEFAULT_BASE_URL: &str = "https://api.groq.com/openai/v1";

#[derive(Clone)]
pub struct GroqConfig {
    api_key: String,
    pub model: String,
    pub base_url: String,
    pub timeout: Duration,
}

// Never print the key, even in debug output.
impl std::fmt::Debug for GroqConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroqConfig")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl GroqConfig {
    pub fn new(api_key: &str, model: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            base_url: DEFAULT_BASE_URL.into(),
            timeout: Duration::from_secs(8),
        }
    }

    /// Reads `GROQ_API_KEY`, `GROQ_MODEL` and optionally `GROQ_BASE_URL`.
    /// Returns None unless both the key and the model are set: the model is
    /// deliberately not hardcoded.
    pub fn from_env() -> Option<Self> {
        let get = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|v| clean_env_value(&v))
                .filter(|v| !v.is_empty())
        };
        let mut cfg = Self::new(&get("GROQ_API_KEY")?, &get("GROQ_MODEL")?);
        if let Some(url) = get("GROQ_BASE_URL") {
            cfg.base_url = url.trim_end_matches('/').to_string();
        }
        Some(cfg)
    }
}

/// Hosting dashboards store values exactly as typed, so a value copied from a
/// `.env` file often arrives wrapped in quotes. A quoted key gets rejected as
/// invalid, which is easy to miss, so strip one pair of matching quotes.
fn clean_env_value(raw: &str) -> String {
    let v = raw.trim();
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|v| v.strip_suffix(q)) {
            return inner.trim().to_string();
        }
    }
    v.to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Author {
    Llm {
        model: String,
    },
    /// `why` says why the LLM wasn't used, if one was configured.
    Template {
        why: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Explanation {
    pub text: String,
    pub author: Author,
}

// The model gets a correct draft and pre-digested facts, and is asked to
// reword, not to reason. Given the raw evidence JSON, a live model picked
// the wrong main reason and invented single-source claims.
const SYSTEM_PROMPT: &str = "You help open-source maintainers triage pull requests. You get a verdict, a few facts, and a correct two-sentence draft. Rewrite the draft so it reads naturally to a busy maintainer: exactly two sentences, plain English. Keep the verdict, every project name, every percentage, any campaign details and any single-source warning. Do not add facts, numbers or reasons that are not in the input. Do not mention scores, JSON, counterfactuals or algorithms. Never speculate about whether anything was written by AI, and never treat being new as suspicious. /no_think";

fn prompt_facts(e: &Evidence) -> Value {
    let sources = |list: &[super::evidence::Source]| -> Vec<Value> {
        list.iter()
            .map(|s| {
                json!({
                    "project": if s.project == e.root { "you" } else { s.project.as_str() },
                    "counts_for": explain::share(s.weight),
                    "reason": s.reason,
                })
            })
            .collect()
    };
    let flips: Vec<&str> = e
        .counterfactual
        .flips
        .iter()
        .map(|f| f.without.as_str())
        .collect();
    json!({
        "verdict": e.tier.label(),
        "contributor": e.subject,
        "your_project": e.root,
        "draft": explain::summary(e),
        "vouched_for_by": sources(&e.vouches.sources),
        "denounced_by": sources(&e.denouncements.sources),
        "campaign": e.campaign.as_ref().map(|c| format!(
            "{} near-identical PRs from {} accounts across {} repos within {} hours",
            c.prs, c.accounts, c.repos, c.hours.max(1)
        )),
        "verdict_hangs_on": if flips.is_empty() { Value::Null } else { json!(flips) },
    })
}

/// Explains with the LLM if configured and healthy, otherwise with the template.
pub fn explain(e: &Evidence, cfg: Option<&GroqConfig>) -> Explanation {
    let Some(cfg) = cfg else {
        return Explanation {
            text: explain::summary(e),
            author: Author::Template { why: None },
        };
    };
    match ask(e, cfg) {
        Ok(text) => Explanation {
            text,
            author: Author::Llm {
                model: cfg.model.clone(),
            },
        },
        Err(why) => Explanation {
            text: explain::summary(e),
            author: Author::Template { why: Some(why) },
        },
    }
}

fn ask(e: &Evidence, cfg: &GroqConfig) -> Result<String, String> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(cfg.timeout))
            .http_status_as_error(false)
            .build(),
    );
    let body = json!({
        "model": cfg.model,
        "temperature": 0.2,
        "max_tokens": 400,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": prompt_facts(e).to_string() },
        ],
    });
    let mut resp = agent
        .post(format!("{}/chat/completions", cfg.base_url))
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .send_json(&body)
        .map_err(|err| format!("couldn't reach the LLM ({err})"))?;

    let status = resp.status().as_u16();
    match status {
        200 => {}
        429 => return Err("the LLM is rate-limited right now".into()),
        401 | 403 => return Err("the LLM rejected the API key".into()),
        404 => return Err(format!("the LLM doesn't know the model '{}'", cfg.model)),
        s => return Err(format!("the LLM returned HTTP {s}")),
    }
    let json: Value = resp
        .body_mut()
        .read_json()
        .map_err(|_| "the LLM's reply wasn't valid JSON".to_string())?;
    let raw = json["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("the LLM's reply had no text")?;
    clean(raw, e)
}

/// Strips reasoning blocks and checks the note says what the evidence says.
fn clean(raw: &str, e: &Evidence) -> Result<String, String> {
    let mut text = raw.to_string();
    while let (Some(start), Some(end)) = (text.find("<think>"), text.find("</think>")) {
        if end < start {
            break;
        }
        text.replace_range(start..end + "</think>".len(), "");
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");

    if text.is_empty() || text.len() > 700 {
        return Err("the LLM's reply was empty or too long".into());
    }
    let lower = text.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));

    let agrees = match e.tier {
        Tier::Quarantined => has(&["quarantin"]),
        Tier::Trusted => has(&["trusted", "fast-track"]),
        Tier::CleanNewcomer => has(&["newcomer", "new contributor", "new to you"]),
    };
    if !agrees {
        return Err("the LLM's note didn't match the verdict".into());
    }
    if e.campaign.is_some() && !has(&["campaign", "coordinated", "wave"]) {
        return Err("the LLM's note left out the campaign".into());
    }
    let single = has(&[
        "single source",
        "rests on",
        "depends on",
        "hangs on",
        "leans on",
    ]);
    if !e.counterfactual.flips.is_empty() && !single && !has(&["review before"]) {
        return Err("the LLM's note left out the single-source warning".into());
    }
    if e.counterfactual.flips.is_empty() && has(&["single source"]) {
        return Err("the LLM's note talked about a single source when there isn't one".into());
    }
    // The main speaker should be named, unless it's the maintainer's own project.
    let main = match e.tier {
        Tier::Trusted => e.vouches.sources.first(),
        _ if e.campaign.is_some() => None,
        _ => e.denouncements.sources.first(),
    };
    if let Some(src) = main.filter(|s| s.project != e.root) {
        if !lower.contains(&src.project.to_lowercase()) {
            return Err(format!("the LLM's note didn't name {}", src.project));
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triage::evidence::{Counterfactual, Side};
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn evidence(tier: Tier) -> Evidence {
        Evidence {
            subject: "github:x".into(),
            root: "ghostty".into(),
            tier,
            vouches: Side {
                total: 0.0,
                sources: vec![],
            },
            denouncements: Side {
                total: 0.0,
                sources: vec![],
            },
            campaign: None,
            counterfactual: Counterfactual {
                checked: 0,
                flips: vec![],
                micros: 1,
            },
        }
    }

    /// A one-shot local HTTP server that answers with `status` and `body`.
    fn serve_once(status: &str, body: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                read_request(&mut stream);
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    /// Reads a whole HTTP request: headers, then exactly Content-Length bytes.
    /// Answering before the body is read and then closing makes Windows reset
    /// the connection, which the client sees as "connection aborted".
    fn read_request(stream: &mut std::net::TcpStream) {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        let header_end = loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => data.extend_from_slice(&buf[..n]),
            }
            if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let headers = String::from_utf8_lossy(&data[..header_end]).to_ascii_lowercase();
        let length: usize = headers
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        while data.len() < header_end + length {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => data.extend_from_slice(&buf[..n]),
            }
        }
    }

    fn cfg(base_url: String) -> GroqConfig {
        let mut c = GroqConfig::new("test-key", "test-model");
        c.base_url = base_url;
        c.timeout = Duration::from_secs(3);
        c
    }

    fn reply(content: &str) -> String {
        json!({ "choices": [{ "message": { "content": content } }] }).to_string()
    }

    #[test]
    fn no_key_means_template_with_no_complaint() {
        let e = evidence(Tier::CleanNewcomer);
        let x = explain(&e, None);
        assert_eq!(x.author, Author::Template { why: None });
        assert_eq!(x.text, explain::summary(&e));
    }

    #[test]
    fn rate_limiting_falls_back_to_the_template() {
        let url = serve_once("429 Too Many Requests", r#"{"error":"slow down"}"#);
        let e = evidence(Tier::CleanNewcomer);
        let x = explain(&e, Some(&cfg(url)));
        assert_eq!(x.text, explain::summary(&e));
        assert!(
            matches!(x.author, Author::Template { why: Some(w) } if w.contains("rate-limited"))
        );
    }

    #[test]
    fn unreachable_server_falls_back() {
        let e = evidence(Tier::Trusted);
        let x = explain(&e, Some(&cfg("http://127.0.0.1:9".into())));
        assert!(matches!(x.author, Author::Template { why: Some(_) }));
    }

    #[test]
    fn a_good_reply_is_used_with_reasoning_stripped() {
        let url = serve_once(
            "200 OK",
            &reply(
                "<think>let me see</think>Quarantine github:x: zig and bun denounced them. Review the PR first.",
            ),
        );
        let x = explain(&evidence(Tier::Quarantined), Some(&cfg(url)));
        assert_eq!(
            x.author,
            Author::Llm {
                model: "test-model".into()
            }
        );
        assert_eq!(
            x.text,
            "Quarantine github:x: zig and bun denounced them. Review the PR first."
        );
    }

    #[test]
    fn a_reply_that_contradicts_the_verdict_is_rejected() {
        let url = serve_once("200 OK", &reply("This contributor looks great, merge it."));
        let x = explain(&evidence(Tier::Quarantined), Some(&cfg(url)));
        assert!(
            matches!(x.author, Author::Template { why: Some(w) } if w.contains("didn't match"))
        );
    }

    #[test]
    fn a_reply_that_drops_the_campaign_is_rejected() {
        use crate::triage::evidence::CampaignEvidence;
        let mut e = evidence(Tier::Quarantined);
        e.campaign = Some(CampaignEvidence {
            prs: 4,
            accounts: 4,
            repos: 4,
            hours: 3,
            text_links: 0,
            layout_links: 6,
            young_share: 0.0,
        });
        // What a live model actually said for this case.
        let url = serve_once(
            "200 OK",
            &reply(
                "The PR is quarantined because the sole vouch from mirror-00 carries a weight of 0.0001%. Proceed with standard review.",
            ),
        );
        let x = explain(&e, Some(&cfg(url)));
        assert!(matches!(x.author, Author::Template { why: Some(w) } if w.contains("campaign")));
    }

    #[test]
    fn an_invented_single_source_claim_is_rejected() {
        let url = serve_once(
            "200 OK",
            &reply(
                "github:x is a clean newcomer. This does not rest on a single source, so review normally.",
            ),
        );
        let x = explain(&evidence(Tier::CleanNewcomer), Some(&cfg(url)));
        assert!(
            matches!(x.author, Author::Template { why: Some(w) } if w.contains("single source"))
        );
    }

    #[test]
    fn the_prompt_carries_a_draft_and_formatted_numbers() {
        let mut e = evidence(Tier::CleanNewcomer);
        e.denouncements
            .sources
            .push(crate::triage::evidence::Source {
                project: "mirror-00".into(),
                weight: 0.000_001,
                path: vec![],
                reason: None,
                age_days: Some(1),
                strength: 1.0,
                provisional: false,
                under_appeal: false,
                actor: None,
            });
        let facts = prompt_facts(&e);
        assert_eq!(facts["draft"], explain::summary(&e));
        assert_eq!(facts["denounced_by"][0]["counts_for"], "under 1%");
        assert!(facts["verdict_hangs_on"].is_null());
    }

    #[test]
    fn quotes_pasted_into_a_dashboard_are_stripped() {
        assert_eq!(clean_env_value("\"qwen/qwen3.8-27b\""), "qwen/qwen3.8-27b");
        assert_eq!(clean_env_value(" 'gsk_abc' "), "gsk_abc");
        assert_eq!(clean_env_value("gsk_abc"), "gsk_abc");
        assert_eq!(clean_env_value("\"unbalanced"), "\"unbalanced");
    }

    #[test]
    fn the_key_never_shows_up_in_debug_output() {
        let c = GroqConfig::new("gsk_secret_value", "m");
        assert!(!format!("{c:?}").contains("gsk_secret_value"));
    }
}
