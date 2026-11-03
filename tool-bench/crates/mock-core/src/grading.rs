//! Scores a trial from the world's final state and journal. No LLM judges: every check is
//! a JSON pointer, a call pattern, or a text match.

use std::collections::BTreeMap;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::journal::JournalEntry;

/// What a scenario expects (generated from `scenario.toml` into the task's `tests/expect.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Expect {
    #[serde(default)]
    pub state: Vec<StateCheck>,
    #[serde(default)]
    pub required_calls: Vec<CallMatch>,
    #[serde(default)]
    pub forbidden_calls: Vec<CallMatch>,
    #[serde(default)]
    pub answer: Option<AnswerCheck>,
}

/// A check on the final state snapshot (`/_bench/state`), addressed by RFC 6901 pointer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateCheck {
    pub pointer: String,
    /// The value equals this exactly.
    #[serde(default)]
    pub equals: Option<Value>,
    /// The value exists (true) or is absent (false).
    #[serde(default)]
    pub exists: Option<bool>,
    /// Array: some element matches this (subset match). String: contains this text.
    #[serde(default)]
    pub contains: Option<Value>,
    /// Some element of the array, or some value of the object, loosely matches this:
    /// objects by key subset, strings by substring, other values by equality. Use it for
    /// records whose keys an agent's actions decide (e.g. the n-th comment on a ticket).
    #[serde(default)]
    pub any: Option<Value>,
    /// With `any` on an object: only entries whose key starts with this.
    #[serde(default)]
    pub key_prefix: Option<String>,
}

/// A pattern over journal entries that came from the tools container.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CallMatch {
    pub service: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub path_regex: Option<String>,
    #[serde(default)]
    pub query_contains: Option<String>,
    /// The request body contains this JSON (objects: subset; arrays: each element found).
    #[serde(default)]
    pub body: Option<Value>,
    /// The raw request body text contains this (for GraphQL, form bodies).
    #[serde(default)]
    pub body_contains: Option<String>,
    #[serde(default = "one")]
    pub min_count: usize,
}

fn one() -> usize {
    1
}

/// Checks on the agent's final answer (`/app/answer.md`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnswerCheck {
    /// Each string must appear, case-insensitively.
    #[serde(default)]
    pub contains: Vec<String>,
    #[serde(default)]
    pub regex: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Pass,
    Fail,
    /// The harness or provider rejected the request for length before any tool ran.
    ContextOverflow,
}

impl Outcome {
    pub fn code(self) -> f64 {
        match self {
            Self::Pass => 0.0,
            Self::Fail => 1.0,
            Self::ContextOverflow => 2.0,
        }
    }
}

pub struct GradeInput<'a> {
    pub expect: &'a Expect,
    /// `WorldState::snapshot()` at the end of the agent phase.
    pub state: Value,
    pub journal: &'a [JournalEntry],
    pub answer: Option<&'a str>,
    /// Peer IPs of the tools container; anything else reaching the world is an MCP bypass.
    pub tools_ips: Vec<String>,
    /// Agent logs contained a context-length rejection.
    pub context_overflow: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Grade {
    pub reward: f64,
    pub outcome: Outcome,
    pub state_ok: bool,
    pub required_calls_ok: bool,
    pub forbidden_calls_ok: bool,
    pub answer_ok: bool,
    /// World requests made through MCP servers.
    pub api_calls: usize,
    /// World requests that did not come from the tools container.
    pub mcp_bypass: usize,
}

impl Grade {
    /// Harbor `reward.json`: a flat map of numbers.
    pub fn metrics(&self) -> BTreeMap<&'static str, f64> {
        let b = |v: bool| if v { 1.0 } else { 0.0 };
        BTreeMap::from([
            ("reward", self.reward),
            ("outcome_code", self.outcome.code()),
            ("state_ok", b(self.state_ok)),
            ("required_calls_ok", b(self.required_calls_ok)),
            ("forbidden_calls_ok", b(self.forbidden_calls_ok)),
            ("answer_ok", b(self.answer_ok)),
            ("api_calls", self.api_calls as f64),
            ("mcp_bypass", self.mcp_bypass as f64),
        ])
    }
}

pub fn grade(input: &GradeInput) -> Grade {
    let (via_tools, bypass): (Vec<&JournalEntry>, Vec<&JournalEntry>) = input
        .journal
        .iter()
        .partition(|e| input.tools_ips.contains(&e.source));

    let state_ok = input
        .expect
        .state
        .iter()
        .all(|c| state_check(&input.state, c));
    let required_calls_ok = input
        .expect
        .required_calls
        .iter()
        .all(|m| via_tools.iter().filter(|e| call_matches(e, m)).count() >= m.min_count);
    let forbidden_calls_ok = !input
        .expect
        .forbidden_calls
        .iter()
        .any(|m| input.journal.iter().any(|e| call_matches(e, m)));
    let answer_ok = input
        .expect
        .answer
        .as_ref()
        .is_none_or(|a| answer_check(input.answer, a));

    let passed = state_ok && required_calls_ok && forbidden_calls_ok && answer_ok;
    let outcome = if passed {
        Outcome::Pass
    } else if input.context_overflow && via_tools.is_empty() {
        Outcome::ContextOverflow
    } else {
        Outcome::Fail
    };
    Grade {
        reward: if passed { 1.0 } else { 0.0 },
        outcome,
        state_ok,
        required_calls_ok,
        forbidden_calls_ok,
        answer_ok,
        api_calls: via_tools.len(),
        mcp_bypass: bypass.len(),
    }
}

fn state_check(state: &Value, check: &StateCheck) -> bool {
    let found = state.pointer(&check.pointer);
    if let Some(should_exist) = check.exists
        && found.is_some() != should_exist
    {
        return false;
    }
    if let Some(expected) = &check.equals
        && found != Some(expected)
    {
        return false;
    }
    if let Some(needle) = &check.any {
        let prefix = check.key_prefix.as_deref().unwrap_or("");
        let ok = match found {
            Some(Value::Array(items)) => items.iter().any(|i| loose(needle, i)),
            Some(Value::Object(map)) => map
                .iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .any(|(_, v)| loose(needle, v)),
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    if let Some(needle) = &check.contains {
        let ok = match (found, needle) {
            (Some(Value::Array(items)), _) => items.iter().any(|i| subset(needle, i)),
            (Some(Value::String(s)), Value::String(n)) => s.contains(n.as_str()),
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

fn call_matches(entry: &JournalEntry, m: &CallMatch) -> bool {
    entry.service == m.service
        && m.method
            .as_ref()
            .is_none_or(|x| x.eq_ignore_ascii_case(&entry.method))
        && m.path.as_ref().is_none_or(|p| *p == entry.path)
        && m.path_regex
            .as_ref()
            .is_none_or(|r| Regex::new(r).is_ok_and(|re| re.is_match(&entry.path)))
        && m.query_contains.as_ref().is_none_or(|q| {
            entry
                .query
                .as_deref()
                .is_some_and(|eq| eq.contains(q.as_str()))
        })
        && m.body
            .as_ref()
            .is_none_or(|b| subset(b, &entry.request_body))
        && m.body_contains
            .as_ref()
            .is_none_or(|t| body_text(&entry.request_body).contains(t.as_str()))
}

fn body_text(body: &Value) -> String {
    match body {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `expected` is contained in `actual`: objects by key subset, arrays by element-wise
/// containment, everything else by equality.
pub fn subset(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => e
            .iter()
            .all(|(k, v)| a.get(k).is_some_and(|av| subset(v, av))),
        (Value::Array(e), Value::Array(a)) => e.iter().all(|ev| a.iter().any(|av| subset(ev, av))),
        _ => expected == actual,
    }
}

/// Like [`subset`], but strings match by substring.
fn loose(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => e
            .iter()
            .all(|(k, v)| a.get(k).is_some_and(|av| loose(v, av))),
        (Value::Array(e), Value::Array(a)) => e.iter().all(|ev| a.iter().any(|av| loose(ev, av))),
        (Value::String(e), Value::String(a)) => a.contains(e.as_str()),
        _ => expected == actual,
    }
}

fn answer_check(answer: Option<&str>, check: &AnswerCheck) -> bool {
    let Some(answer) = answer else {
        return false;
    };
    let lower = answer.to_lowercase();
    check
        .contains
        .iter()
        .all(|c| lower.contains(&c.to_lowercase()))
        && check
            .regex
            .as_ref()
            .is_none_or(|r| Regex::new(r).is_ok_and(|re| re.is_match(answer)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::journal::JournalEntry;

    fn entry(seq: u64, source: &str, method: &str, path: &str, body: Value) -> JournalEntry {
        JournalEntry {
            seq,
            time: "2026-03-14T09:00:00Z".into(),
            service: "github".into(),
            source: source.into(),
            method: method.into(),
            host: None,
            scheme: "http".into(),
            content_type: None,
            path: path.into(),
            query: Some("state=open".into()),
            request_body: body,
            status: 200,
            response_body: Value::Null,
        }
    }

    fn state() -> Value {
        json!({"services": {"github": {"issues": {"octo-org/app#3": {"state": "closed", "labels": [{"name": "bug"}], "title": "Flaky login test"}}}}})
    }

    fn expect(v: Value) -> Expect {
        serde_json::from_value(v).unwrap()
    }

    const TOOLS: &str = "10.0.0.5";

    fn input<'a>(
        expect: &'a Expect,
        journal: &'a [JournalEntry],
        answer: Option<&'a str>,
    ) -> GradeInput<'a> {
        GradeInput {
            expect,
            state: state(),
            journal,
            answer,
            tools_ips: vec![TOOLS.to_owned()],
            context_overflow: false,
        }
    }

    #[test]
    fn passes_when_every_check_holds() {
        let e = expect(json!({
            "state": [
                {"pointer": "/services/github/issues/octo-org~1app#3/state", "equals": "closed"},
                {"pointer": "/services/github/issues/octo-org~1app#3/labels", "contains": {"name": "bug"}},
                {"pointer": "/services/github/issues/octo-org~1app#9", "exists": false}
            ],
            "required_calls": [{"service": "github", "method": "PATCH", "path": "/api/v3/repos/octo-org/app/issues/3", "body": {"state": "closed"}}],
            "forbidden_calls": [{"service": "github", "method": "DELETE"}],
            "answer": {"contains": ["flaky LOGIN"]}
        }));
        let journal = [entry(
            1,
            TOOLS,
            "PATCH",
            "/api/v3/repos/octo-org/app/issues/3",
            json!({"state": "closed", "state_reason": "completed"}),
        )];
        let g = grade(&input(
            &e,
            &journal,
            Some("Closed issue #3 (Flaky login test)."),
        ));
        assert_eq!(g.reward, 1.0);
        assert_eq!(g.outcome, Outcome::Pass);
        assert_eq!(
            (
                g.state_ok,
                g.required_calls_ok,
                g.forbidden_calls_ok,
                g.answer_ok
            ),
            (true, true, true, true)
        );
        assert_eq!(g.api_calls, 1);
        assert_eq!(g.mcp_bypass, 0);
    }

    #[test]
    fn each_failed_component_fails_the_trial() {
        let e = expect(json!({
            "state": [{"pointer": "/services/github/issues/octo-org~1app#3/state", "equals": "open"}],
            "required_calls": [{"service": "github", "method": "POST", "path_regex": "^/api/v3/repos/[^/]+/app/issues$", "min_count": 1}],
            "forbidden_calls": [{"service": "github", "method": "PATCH"}]
        }));
        let journal = [entry(
            1,
            TOOLS,
            "PATCH",
            "/api/v3/repos/octo-org/app/issues/3",
            Value::Null,
        )];
        let g = grade(&input(&e, &journal, None));
        assert_eq!(g.reward, 0.0);
        assert_eq!(g.outcome, Outcome::Fail);
        assert_eq!(
            (g.state_ok, g.required_calls_ok, g.forbidden_calls_ok),
            (false, false, false)
        );
        assert!(g.answer_ok, "no answer check means answer_ok");
    }

    #[test]
    fn calls_not_from_the_tools_container_are_bypasses_and_do_not_count() {
        let e = expect(json!({"required_calls": [{"service": "github", "method": "PATCH"}]}));
        let journal = [entry(
            1,
            "10.0.0.9",
            "PATCH",
            "/api/v3/repos/octo-org/app/issues/3",
            Value::Null,
        )];
        let g = grade(&input(&e, &journal, None));
        assert_eq!(g.mcp_bypass, 1);
        assert!(
            !g.required_calls_ok,
            "a curl from the agent container is not a tool call"
        );
        assert_eq!(g.outcome, Outcome::Fail);
    }

    #[test]
    fn context_overflow_without_any_tool_activity_is_its_own_outcome() {
        let e = expect(json!({"required_calls": [{"service": "github"}]}));
        let mut i = input(&e, &[], None);
        i.context_overflow = true;
        let g = grade(&i);
        assert_eq!(g.outcome, Outcome::ContextOverflow);
        assert_eq!(g.reward, 0.0);
        assert_eq!(g.metrics()["outcome_code"], 2.0);
    }

    #[test]
    fn answer_regex_and_missing_answer() {
        let e = expect(json!({"answer": {"regex": "(?i)total:\\s*\\$?1,?204"}}));
        assert!(grade(&input(&e, &[], Some("Total: $1,204"))).answer_ok);
        assert!(!grade(&input(&e, &[], None)).answer_ok);
    }

    #[test]
    fn query_and_body_text_matchers() {
        let e = expect(
            json!({"required_calls": [{"service": "github", "query_contains": "state=open", "body_contains": "closed"}]}),
        );
        let journal = [entry(1, TOOLS, "PATCH", "/x", json!({"state": "closed"}))];
        assert!(grade(&input(&e, &journal, None)).required_calls_ok);
    }

    #[test]
    fn any_matches_some_entry_by_key_prefix_with_substring_strings() {
        let state = json!({"services": {"zendesk": {"comments": {
            "4217/1": {"public": true, "body": "Original request"},
            "4217/2": {"public": false, "body": "internal: refund RF-88213 issued"},
            "4217/3": {"public": true, "body": "Your refund RF-88213 has been issued."},
            "9999/1": {"public": true, "body": "RF-11111"}
        }}}});
        let check = |any: Value, prefix: &str| {
            let e: Expect = serde_json::from_value(json!({"state": [{"pointer": "/services/zendesk/comments", "any": any, "key_prefix": prefix}]})).unwrap();
            let mut i = input(&e, &[], None);
            i.state = state.clone();
            grade(&i).state_ok
        };
        assert!(check(json!({"public": true, "body": "RF-88213"}), "4217/"));
        assert!(
            !check(json!({"public": true, "body": "RF-11111"}), "4217/"),
            "prefix scopes the search"
        );
        assert!(!check(
            json!({"public": false, "body": "has been issued"}),
            "4217/"
        ));
    }

    #[test]
    fn metrics_are_all_numeric() {
        let e = expect(json!({}));
        let m = grade(&input(&e, &[], None)).metrics();
        for key in [
            "reward",
            "state_ok",
            "required_calls_ok",
            "forbidden_calls_ok",
            "answer_ok",
            "api_calls",
            "mcp_bypass",
            "outcome_code",
        ] {
            assert!(m.contains_key(key), "{key}");
        }
    }
}
