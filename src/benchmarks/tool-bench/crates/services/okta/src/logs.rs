//! `/api/v1/logs`: the System Log with `since`/`until`/`filter`/`q`/`sortOrder`/`limit`
//! and `after` cursors. Events are the dataset's `logs` plus those appended by mutations.

use mock_core::world::WorldState;
use serde_json::Value;

use crate::{
    filter,
    util::{Reply, Req, Res, bad_request, list, page, parse_ts},
};

const DEFAULT_WINDOW_SECS: i64 = 7 * 24 * 3600;

pub fn list_logs(w: &mut WorldState, req: &Req) -> Res {
    let now = w.clock.now_unix();
    let bound = |key: &str, default: i64| -> Result<i64, Reply> {
        match req.q(key) {
            None => Ok(default),
            Some(v) => parse_ts(v).ok_or_else(|| {
                bad_request(
                    "E0000001",
                    &format!(
                        "Api validation failed: {key}. {key} must be a valid ISO 8601 timestamp"
                    ),
                )
            }),
        }
    };
    let since = bound("since", now - DEFAULT_WINDOW_SECS)?;
    let until = bound("until", now + 1)?;
    if since > until {
        return Err(bad_request(
            "E0000001",
            "Api validation failed: since. since must be before until",
        ));
    }
    let expr = match req.q("filter") {
        Some(f) => Some(
            filter::parse(f)
                .map_err(|e| bad_request("E0000001", &format!("Invalid filter: {e}")))?,
        ),
        None => None,
    };
    let terms: Vec<String> = req
        .q("q")
        .map(|q| q.split_whitespace().map(str::to_lowercase).collect())
        .unwrap_or_default();
    let mut events: Vec<Value> = list(w, "logs")
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| {
            e["published"]
                .as_str()
                .and_then(parse_ts)
                .is_some_and(|t| t >= since && t < until)
        })
        .filter(|e| expr.as_ref().is_none_or(|x| x.matches(e)))
        .filter(|e| {
            let text = e.to_string().to_lowercase();
            terms.iter().all(|t| text.contains(t.as_str()))
        })
        .collect();
    events.sort_by_key(|e| e["published"].as_str().and_then(parse_ts).unwrap_or(0));
    if req
        .q("sortOrder")
        .is_some_and(|o| o.eq_ignore_ascii_case("DESCENDING"))
    {
        events.reverse();
    }
    Ok(page(req, events, 100, 1000, |e| {
        e["uuid"].as_str().unwrap_or_default().to_owned()
    }))
}
