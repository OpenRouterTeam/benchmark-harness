//! Tooling API extras: `executeAnonymous` and debug log generation.
//!
//! Anonymous Apex is syntax-checked (balanced brackets and quotes, terminated statements)
//! but not executed: `System.debug(...)` calls are written to the debug log (string
//! literals evaluated, other expressions echoed) and DML statements have no effect on data.
//! A log is created only when the API user has an unexpired `USER_DEBUG` trace flag, as in
//! a real org.

use mock_core::world::ServiceState;
use serde_json::{Value, json};

use crate::{
    db::current_user,
    util::{format_datetime, minted_id, parse_datetime},
};

pub struct Compile {
    pub compiled: bool,
    pub line: i64,
    pub column: i64,
    pub problem: Option<String>,
}

/// A light syntax check standing in for the Apex compiler.
pub fn check(code: &str) -> Compile {
    let mut stack: Vec<(char, usize, usize)> = Vec::new();
    let (mut line, mut col) = (1usize, 0usize);
    let mut chars = code.chars().peekable();
    let mut in_str = false;
    let fail = |l: usize, c: usize, msg: String| Compile {
        compiled: false,
        line: l as i64,
        column: c as i64,
        problem: Some(msg),
    };
    while let Some(c) = chars.next() {
        col += 1;
        if c == '\n' {
            line += 1;
            col = 0;
            if in_str {
                return fail(line - 1, col, "Unexpected token '''.".to_owned());
            }
            continue;
        }
        if in_str {
            if c == '\\' {
                chars.next();
                col += 1;
            } else if c == '\'' {
                in_str = false;
            }
            continue;
        }
        match c {
            '\'' => in_str = true,
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        line += 1;
                        col = 0;
                        break;
                    }
                }
            }
            '(' | '{' | '[' => stack.push((c, line, col)),
            ')' | '}' | ']' => {
                let want = match c {
                    ')' => '(',
                    '}' => '{',
                    _ => '[',
                };
                match stack.pop() {
                    Some((open, _, _)) if open == want => {}
                    _ => return fail(line, col, format!("Unexpected token '{c}'.")),
                }
            }
            _ => {}
        }
    }
    if in_str {
        return fail(line, col, "Unexpected token '''.".to_owned());
    }
    if let Some((_, l, c)) = stack.pop() {
        return fail(l, c, "Unexpected token '<EOF>'.".to_owned());
    }
    let trimmed = code.trim_end();
    if !trimmed.is_empty() && !trimmed.ends_with(';') && !trimmed.ends_with('}') {
        let last_line = trimmed.lines().count();
        let last_col = trimmed.lines().last().map_or(0, |l| l.chars().count());
        return fail(last_line, last_col + 1, "Missing ';' at '<EOF>'".to_owned());
    }
    Compile {
        compiled: true,
        line: -1,
        column: -1,
        problem: None,
    }
}

/// `System.debug(...)` messages with their line numbers.
fn debug_lines(code: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, line) in code.lines().enumerate() {
        let mut rest = line;
        while let Some(at) = rest.to_ascii_lowercase().find("system.debug(") {
            let after = &rest[at + "system.debug(".len()..];
            let mut depth = 1;
            let mut in_str = false;
            let mut end = after.len();
            let mut prev = ' ';
            for (j, c) in after.char_indices() {
                match c {
                    '\'' if prev != '\\' => in_str = !in_str,
                    '(' if !in_str => depth += 1,
                    ')' if !in_str => {
                        depth -= 1;
                        if depth == 0 {
                            end = j;
                            break;
                        }
                    }
                    _ => {}
                }
                prev = c;
            }
            out.push((i + 1, render_expr(after[..end].trim())));
            rest = &after[end.min(after.len())..];
        }
    }
    out
}

/// Concatenations of string literals are evaluated; anything else is echoed.
fn render_expr(expr: &str) -> String {
    let mut out = String::new();
    for part in expr.split('+') {
        let p = part.trim();
        if p.len() >= 2 && p.starts_with('\'') && p.ends_with('\'') {
            out.push_str(&p[1..p.len() - 1].replace("\\'", "'"));
        } else {
            return expr.to_owned();
        }
    }
    out
}

fn stamp(ms: i64, nanos: i64) -> String {
    let t = format_datetime(ms);
    format!("{}.{} ({nanos})", &t[11..19], (ms % 1000) / 100)
}

/// Writes an `ApexLog` for an anonymous execution if the API user is being traced.
pub fn maybe_log(
    state: &mut ServiceState,
    now_ms: i64,
    version: &str,
    code: &str,
    result: &Compile,
) {
    if !result.compiled {
        return;
    }
    // Several executions can share a logical second; later logs sort later.
    let logged = state.store.collection("ApexLog").map_or(0, |c| c.len()) as i64;
    let now_ms = now_ms + 1 + logged % 999;
    let Some(user) = current_user(&state.store).cloned() else {
        return;
    };
    let user_id = user["Id"].as_str().unwrap_or("").to_owned();
    let flag = state.store.collection("TraceFlag").and_then(|c| {
        c.values()
            .find(|f| {
                f["TracedEntityId"].as_str() == Some(user_id.as_str())
                    && f["ExpirationDate"]
                        .as_str()
                        .and_then(parse_datetime)
                        .is_some_and(|e| e > now_ms)
            })
            .cloned()
    });
    let Some(flag) = flag else {
        return;
    };
    let level = flag["DebugLevelId"]
        .as_str()
        .and_then(|id| {
            state
                .store
                .collection("DebugLevel")
                .and_then(|c| c.get(id))
                .cloned()
        })
        .unwrap_or(Value::Null);
    let lvl = |k: &str| level[k].as_str().unwrap_or("INFO").to_owned();
    let mut body = format!(
        "{version} APEX_CODE,{};APEX_PROFILING,{};CALLOUT,{};DB,{};NBA,{};SYSTEM,{};VALIDATION,{};VISUALFORCE,{};WAVE,{};WORKFLOW,{}\n",
        lvl("ApexCode"),
        lvl("ApexProfiling"),
        lvl("Callout"),
        lvl("Database"),
        lvl("Nba"),
        lvl("System"),
        lvl("Validation"),
        lvl("Visualforce"),
        lvl("Wave"),
        lvl("Workflow"),
    );
    for l in code.lines() {
        body.push_str(&format!("Execute Anonymous: {l}\n"));
    }
    let mut nanos = 1_187_000i64;
    let mut line = |body: &mut String, text: String| {
        nanos += 113_442;
        body.push_str(&format!("{}|{text}\n", stamp(now_ms, nanos)));
    };
    line(
        &mut body,
        format!(
            "USER_INFO|[EXTERNAL]|{}|{}|(GMT+00:00) Greenwich Mean Time (GMT)|GMT+00:00",
            &user_id[..15.min(user_id.len())],
            user["Username"].as_str().unwrap_or("")
        ),
    );
    line(&mut body, "EXECUTION_STARTED".to_owned());
    line(
        &mut body,
        "CODE_UNIT_STARTED|[EXTERNAL]|execute_anonymous_apex".to_owned(),
    );
    for (n, msg) in debug_lines(code) {
        line(&mut body, format!("USER_DEBUG|[{n}]|DEBUG|{msg}"));
    }
    line(&mut body, "CUMULATIVE_LIMIT_USAGE".to_owned());
    line(&mut body, "CUMULATIVE_LIMIT_USAGE_END".to_owned());
    line(
        &mut body,
        "CODE_UNIT_FINISHED|execute_anonymous_apex".to_owned(),
    );
    line(&mut body, "EXECUTION_FINISHED".to_owned());
    let n = state.ids.next_int("id/07L", 1);
    let id = minted_id("07L", n);
    let now = format_datetime(now_ms);
    let rec = json!({
        "Id": id,
        "LogUserId": user_id,
        "LogLength": body.len(),
        "LastModifiedDate": now,
        "Request": "API",
        "Operation": format!("/services/data/v{version}/tooling/executeAnonymous/"),
        "Application": "Unknown",
        "Status": "Success",
        "DurationMilliseconds": 18 + (code.len() % 40) as i64,
        "SystemModstamp": now,
        "StartTime": now,
        "Location": "Monitoring",
        "RequestIdentifier": format!("TID:{}", 9_000_000 + n),
        "Body": body,
    });
    state.store.collection_mut("ApexLog").insert(id, rec);
}
