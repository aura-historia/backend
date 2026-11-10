//! What filled an agent process's context, read from its `stream-json` output: the size of its
//! first model request, which holds the system prompt, the tool definitions and the task, split
//! into what was read from the prompt cache and what was written to it, and per review tool the
//! calls, the characters of their answers and the characters those answers were read again as
//! (each answer's characters times the model requests that followed it). Each answer stays in the
//! context and is read again on every later turn, so the re-read characters say which tool the
//! input tokens go to. Only tool names and counts are kept; what a tool answered never is. The
//! runner keeps only the end of a long stream, so the sizes cover what that end shows.

use super::envelope::bounded_reason;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

/// The MCP prefix of the review tools, left out of the names the runner records.
const REVIEW_TOOL_PREFIX: &str = "mcp__crawler_review__";
/// The name an answer is counted under when the stream holds no call with its id.
const UNKNOWN_TOOL: &str = "unknown";

/// What filled one process's context.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUse {
    /// The input tokens of the first model request, read from cache or not.
    #[serde(default)]
    pub opening_tokens: u64,
    /// Of the first request's input tokens, those read from the prompt cache: a prefix an earlier
    /// process wrote.
    #[serde(default)]
    pub opening_cache_read_tokens: u64,
    /// Of the first request's input tokens, those written to the prompt cache, at a higher price
    /// than a read.
    #[serde(default)]
    pub opening_cache_write_tokens: u64,
    /// The answers per tool, by the tool's name without its MCP prefix.
    #[serde(default)]
    pub tool_answers: BTreeMap<String, ToolAnswers>,
}

/// The answers one tool gave a process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolAnswers {
    pub calls: u64,
    /// The characters of every answer together.
    pub chars: u64,
    /// The characters of the longest answer.
    pub largest_chars: u64,
    /// The characters of every answer times the model requests that followed it: what the answers
    /// cost as context read again, which ranks the tools by input tokens better than `chars`.
    #[serde(default)]
    pub reread_chars: u64,
}

impl ToolAnswers {
    fn add(&mut self, other: Self) {
        self.calls += other.calls;
        self.chars += other.chars;
        self.largest_chars = self.largest_chars.max(other.largest_chars);
        self.reread_chars += other.reread_chars;
    }
}

impl ContextUse {
    /// What the process's `stdout` shows of its context: the first `assistant` message's usage
    /// and every `tool_result`, named by the `tool_use` of the same id.
    pub fn parse(stdout: &[u8]) -> Self {
        let text = String::from_utf8_lossy(stdout);
        let mut context = Self::default();
        let mut tool_names: HashMap<String, String> = HashMap::new();
        let mut requests = ModelRequests::default();
        // Each answer's tool, characters and the requests made before it, so its re-read weight
        // is known once the stream's last request is counted.
        let mut answers_at: Vec<(String, u64, u64)> = Vec::new();
        for message in text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        {
            let kind = message.get("type").and_then(Value::as_str);
            let Some(message) = message.get("message") else {
                continue;
            };
            match kind {
                Some("assistant") => {
                    let first_request = requests.count == 0;
                    requests.see(message);
                    if first_request {
                        context.read_opening(message.get("usage"));
                    }
                    for (id, name) in content_blocks(message, "tool_use").filter_map(tool_use) {
                        tool_names.insert(id, name);
                    }
                }
                Some("user") => {
                    for block in content_blocks(message, "tool_result") {
                        let name = block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .and_then(|id| tool_names.get(id))
                            .map_or(UNKNOWN_TOOL, String::as_str);
                        let chars = answer_chars(block.get("content"));
                        answers_at.push((name.to_owned(), chars, requests.count));
                    }
                }
                _ => {}
            }
        }
        for (name, chars, requests_before) in answers_at {
            context
                .tool_answers
                .entry(name)
                .or_default()
                .add(ToolAnswers {
                    calls: 1,
                    chars,
                    largest_chars: chars,
                    reread_chars: chars * requests.count.saturating_sub(requests_before),
                });
        }
        context
    }

    /// The first request's input tokens, in total and split by what the prompt cache did.
    fn read_opening(&mut self, usage: Option<&Value>) {
        self.opening_tokens = request_tokens(usage);
        self.opening_cache_read_tokens = usage_tokens(usage, "cache_read_input_tokens");
        self.opening_cache_write_tokens = usage_tokens(usage, "cache_creation_input_tokens");
    }
}

/// The model requests a stream shows so far. The CLI prints one `assistant` message per content
/// block of a response, all with the response's id, so a request is counted once per id; a message
/// without an id counts as a request of its own.
#[derive(Default)]
struct ModelRequests {
    seen: HashSet<String>,
    count: u64,
}

impl ModelRequests {
    fn see(&mut self, message: &Value) {
        let is_new = match message.get("id").and_then(Value::as_str) {
            Some(id) => self.seen.insert(id.to_owned()),
            None => true,
        };
        if is_new {
            self.count += 1;
        }
    }
}

fn content_blocks<'a>(message: &'a Value, kind: &'a str) -> impl Iterator<Item = &'a Value> {
    message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(move |block| block.get("type").and_then(Value::as_str) == Some(kind))
}

/// The id and the recorded name of a `tool_use` block.
fn tool_use(block: &Value) -> Option<(String, String)> {
    let id = block.get("id").and_then(Value::as_str)?;
    let name = block.get("name").and_then(Value::as_str)?;
    let name = name.strip_prefix(REVIEW_TOOL_PREFIX).unwrap_or(name);
    Some((id.to_owned(), bounded_reason(name)))
}

/// The characters of a `tool_result`'s content: a text, or the texts of its blocks.
fn answer_chars(content: Option<&Value>) -> u64 {
    let chars = match content {
        Some(Value::String(text)) => text.chars().count(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .map(|text| text.chars().count())
            .sum(),
        _ => 0,
    };
    u64::try_from(chars).unwrap_or(u64::MAX)
}

/// All input tokens of one request: uncached, read from cache and written to it.
fn request_tokens(usage: Option<&Value>) -> u64 {
    [
        "input_tokens",
        "cache_read_input_tokens",
        "cache_creation_input_tokens",
    ]
    .iter()
    .map(|key| usage_tokens(usage, key))
    .sum()
}

fn usage_tokens(usage: Option<&Value>, key: &str) -> u64 {
    usage
        .and_then(|usage| usage.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// The context use of many processes, per role, for the digest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextUseSummary {
    roles: BTreeMap<&'static str, RoleContextUse>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RoleContextUse {
    processes: u64,
    opening_tokens: u64,
    opening_cache_read_tokens: u64,
    opening_cache_write_tokens: u64,
    tool_answers: BTreeMap<String, ToolAnswers>,
}

impl ContextUseSummary {
    pub fn add(&mut self, role: &'static str, context: &ContextUse) {
        let summary = self.roles.entry(role).or_default();
        summary.processes += 1;
        summary.opening_tokens += context.opening_tokens;
        summary.opening_cache_read_tokens += context.opening_cache_read_tokens;
        summary.opening_cache_write_tokens += context.opening_cache_write_tokens;
        for (tool, answers) in &context.tool_answers {
            summary
                .tool_answers
                .entry(tool.clone())
                .or_default()
                .add(*answers);
        }
    }

    /// One table per role of the tools by the characters their answers were read again as, largest
    /// first, with the average opening request and how much of it the prompt cache served; nothing
    /// while no process recorded its context.
    pub fn write_markdown(&self, out: &mut String) {
        let roles: Vec<_> = self
            .roles
            .iter()
            .filter(|(_, role)| role.opening_tokens > 0 || !role.tool_answers.is_empty())
            .collect();
        if roles.is_empty() {
            return;
        }
        let _ = writeln!(out, "## Context use\n");
        for (role, summary) in roles {
            let processes = summary.processes.max(1);
            let _ = writeln!(
                out,
                "{role}: {} processes, opening request {} tokens on average, {} read from cache and {} written to it.\n",
                summary.processes,
                summary.opening_tokens / processes,
                summary.opening_cache_read_tokens / processes,
                summary.opening_cache_write_tokens / processes
            );
            if summary.tool_answers.is_empty() {
                continue;
            }
            let mut tools: Vec<_> = summary.tool_answers.iter().collect();
            tools.sort_by(|left, right| {
                right
                    .1
                    .reread_chars
                    .cmp(&left.1.reread_chars)
                    .then(left.0.cmp(right.0))
            });
            let _ = writeln!(
                out,
                "| Tool | Calls | Answer characters | Per process | Per call | Largest | Re-read characters |"
            );
            let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|");
            for (tool, answers) in tools {
                let _ = writeln!(
                    out,
                    "| {tool} | {} | {} | {} | {} | {} | {} |",
                    answers.calls,
                    answers.chars,
                    answers.chars / processes,
                    answers.chars / answers.calls.max(1),
                    answers.largest_chars,
                    answers.reread_chars
                );
            }
            out.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stream(lines: &[Value]) -> Vec<u8> {
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>()
            .into_bytes()
    }

    fn tool_call(message_id: &str, id: &str, name: &str, usage: Value) -> Value {
        json!({
            "type": "assistant",
            "message": {
                "id": message_id,
                "content": [{ "type": "tool_use", "id": id, "name": name, "input": {} }],
                "usage": usage
            }
        })
    }

    fn tool_result(id: &str, content: Value) -> Value {
        json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{ "type": "tool_result", "tool_use_id": id, "content": content }]
            }
        })
    }

    #[test]
    fn should_count_each_tool_s_answers_and_the_opening_request() {
        let stdout = stream(&[
            json!({ "type": "system", "subtype": "init", "tools": ["mcp__crawler_review__get_review"] }),
            tool_call(
                "m1",
                "t1",
                "mcp__crawler_review__get_review",
                json!({ "input_tokens": 3, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 9_000 }),
            ),
            tool_result("t1", json!([{ "type": "text", "text": "ä".repeat(40) }])),
            tool_call(
                "m2",
                "t2",
                "mcp__crawler_review__inspect_page",
                json!({ "input_tokens": 1, "cache_read_input_tokens": 9_003, "cache_creation_input_tokens": 50 }),
            ),
            tool_result("t2", json!("x".repeat(100))),
            tool_call("m3", "t3", "mcp__crawler_review__inspect_page", json!({})),
            tool_result(
                "t3",
                json!([{ "type": "text", "text": "y".repeat(30) }, { "type": "text", "text": "z" }]),
            ),
            tool_result("t-cut-off", json!("w".repeat(7))),
            json!({ "type": "result", "subtype": "success" }),
        ]);

        let context = ContextUse::parse(&stdout);

        assert_eq!(context.opening_tokens, 9_003);
        assert_eq!(context.opening_cache_read_tokens, 0);
        assert_eq!(context.opening_cache_write_tokens, 9_000);
        assert_eq!(
            context.tool_answers,
            BTreeMap::from([
                (
                    "get_review".to_owned(),
                    ToolAnswers {
                        calls: 1,
                        chars: 40,
                        largest_chars: 40,
                        reread_chars: 80
                    }
                ),
                (
                    "inspect_page".to_owned(),
                    ToolAnswers {
                        calls: 2,
                        chars: 131,
                        largest_chars: 100,
                        reread_chars: 100
                    }
                ),
                (
                    UNKNOWN_TOOL.to_owned(),
                    ToolAnswers {
                        calls: 1,
                        chars: 7,
                        largest_chars: 7,
                        reread_chars: 0
                    }
                ),
            ])
        );
    }

    #[test]
    fn should_count_a_response_once_however_many_messages_it_prints() {
        let stdout = stream(&[
            tool_call("m1", "t1", "mcp__crawler_review__inspect_page", json!({})),
            tool_call("m1", "t2", "mcp__crawler_review__get_review", json!({})),
            tool_result("t1", json!("x".repeat(10))),
            tool_result("t2", json!("y".repeat(20))),
            tool_call("m2", "t3", "mcp__crawler_review__get_review", json!({})),
            tool_result("t3", json!("z".repeat(5))),
            json!({ "type": "assistant", "message": { "id": "m3", "content": [] } }),
        ]);

        let context = ContextUse::parse(&stdout);

        let reread = |tool: &str| context.tool_answers[tool].reread_chars;
        assert_eq!(reread("inspect_page"), 10 * 2);
        assert_eq!(reread("get_review"), 20 * 2 + 5);
    }

    #[test]
    fn should_read_nothing_from_output_without_messages() {
        assert_eq!(
            ContextUse::parse(b"not json\n{\"type\":\"result\"}\n"),
            ContextUse::default()
        );
    }

    #[test]
    fn should_keep_only_identifier_characters_of_a_tool_name() {
        let stdout = stream(&[
            tool_call("m1", "t1", "mcp__other__read <script>", json!({})),
            tool_result("t1", json!("x")),
        ]);

        let context = ContextUse::parse(&stdout);

        assert_eq!(
            context.tool_answers.keys().collect::<Vec<_>>(),
            ["mcp__other__readscript"]
        );
    }

    #[test]
    fn should_tabulate_each_role_s_tools_by_re_read_characters() {
        let mut summary = ContextUseSummary::default();
        let context = |inspect: u64, review: u64| ContextUse {
            opening_tokens: 10_000,
            opening_cache_read_tokens: 6_000,
            opening_cache_write_tokens: 4_000,
            tool_answers: BTreeMap::from([
                (
                    "inspect_page".to_owned(),
                    ToolAnswers {
                        calls: 2,
                        chars: inspect,
                        largest_chars: inspect / 2,
                        reread_chars: inspect * 3,
                    },
                ),
                (
                    "get_review".to_owned(),
                    ToolAnswers {
                        calls: 1,
                        chars: review,
                        largest_chars: review,
                        reread_chars: review * 5,
                    },
                ),
            ]),
        };
        summary.add("author", &context(60_000, 8_000));
        summary.add("author", &context(20_000, 4_000));
        summary.add("reviewer", &ContextUse::default());

        let mut markdown = String::new();
        summary.write_markdown(&mut markdown);

        assert_eq!(
            markdown,
            "## Context use\n\n\
             author: 2 processes, opening request 10000 tokens on average, 6000 read from cache and 4000 written to it.\n\n\
             | Tool | Calls | Answer characters | Per process | Per call | Largest | Re-read characters |\n\
             |---|---:|---:|---:|---:|---:|---:|\n\
             | inspect_page | 4 | 80000 | 40000 | 20000 | 30000 | 240000 |\n\
             | get_review | 2 | 12000 | 6000 | 6000 | 8000 | 60000 |\n\n"
        );
        let mut empty = String::new();
        ContextUseSummary::default().write_markdown(&mut empty);
        assert_eq!(empty, "");
    }
}
