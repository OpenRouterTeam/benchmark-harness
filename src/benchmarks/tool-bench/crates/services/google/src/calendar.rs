//! Calendar tools over `calendar_calendars` and `calendar_events`.
//!
//! Event times are stored the way the Calendar API returns them: `start`/`end` are
//! `{dateTime, timeZone}` with `dateTime` rendered in the event's zone
//! (`2026-11-03T10:00:00-05:00`), or `{date}` for all-day events.

use serde_json::{Map, Value, json};

use crate::{
    tz,
    util::{
        Ctx, R, USER, arg, arg_bool, arg_i64, arg_json, arg_list, b64, format_in_zone, http_error,
        parse_datetime, s, weekday_name,
    },
};

const BASE: &str = "https://www.googleapis.com/calendar/v3";

fn calendar(ctx: &Ctx, id: &str) -> Option<Value> {
    let id = if id == "primary" {
        primary_id(ctx)
    } else {
        id.to_owned()
    };
    ctx.get("calendar_calendars", &id)
}

fn primary_id(ctx: &Ctx) -> String {
    ctx.all("calendar_calendars")
        .into_iter()
        .find(|c| c["primary"].as_bool().unwrap_or(false))
        .map(|c| s(&c, "id").to_owned())
        .unwrap_or_else(|| USER.to_owned())
}

fn cal_zone(ctx: &Ctx, id: &str) -> String {
    calendar(ctx, id)
        .map(|c| s(&c, "timeZone").to_owned())
        .filter(|z| !z.is_empty())
        .unwrap_or_else(|| "America/New_York".to_owned())
}

fn resolve(ctx: &Ctx, id: &str) -> String {
    if id == "primary" {
        primary_id(ctx)
    } else {
        id.to_owned()
    }
}

fn not_found(tool: &str, url: &str) -> String {
    format!(
        "API error in {tool}: {}",
        http_error(404, url, "Not Found", "notFound")
    )
}

/// Seconds of an event boundary (dateTime, or midnight of date in the event zone).
pub fn boundary_secs(ev: &Value, field: &str, fallback_zone: &str) -> Option<i64> {
    let b = &ev[field];
    if let Some(dt) = b["dateTime"].as_str() {
        return parse_datetime(dt, Some(fallback_zone)).map(|(t, _)| t);
    }
    let d = b["date"].as_str()?;
    let zone = b["timeZone"].as_str().unwrap_or(fallback_zone);
    parse_datetime(d, Some(zone)).map(|(t, _)| t)
}

/// `_format_event_time`: local stamp plus zone and weekday evidence.
pub fn render_time(ev: &Value, field: &str) -> String {
    let b = &ev[field];
    if let Some(dt) = b["dateTime"].as_str() {
        let zone = b["timeZone"].as_str();
        let Some((t, _)) = parse_datetime(dt, zone.or(Some("UTC"))) else {
            return dt.to_owned();
        };
        let (stamp, days) = match zone {
            Some(z) => {
                let off = tz::offset_at(z, t).0;
                (format_in_zone(t, z), (t + off).div_euclid(86_400))
            }
            None => {
                // No zone: echo Google's value; weekday from its own offset.
                let (local, _) =
                    parse_datetime(&dt[..19.min(dt.len())], Some("UTC")).unwrap_or((t, false));
                (dt.to_owned(), local.div_euclid(86_400))
            }
        };
        let (wd, iso) = weekday_name(days);
        let mut ev_parts = Vec::new();
        if let Some(z) = zone {
            ev_parts.push(z.to_owned());
        }
        ev_parts.push(format!("weekday: {wd}"));
        ev_parts.push(format!("ISO weekday: {iso}"));
        return format!("{stamp} [{}]", ev_parts.join("; "));
    }
    if let Some(d) = b["date"].as_str() {
        let Some((y, m, dd)) = tz::parse_date(d) else {
            return d.to_owned();
        };
        let (wd, iso) = weekday_name(tz::days_from_civil(y, m, dd));
        let mut parts = vec![format!("weekday: {wd}"), format!("ISO weekday: {iso}")];
        if field == "end" {
            parts.push("exclusive all-day end".into());
        }
        return format!("{d} [{}]", parts.join("; "));
    }
    "Unavailable".into()
}

fn meeting_link(ev: &Value) -> String {
    if let Some(eps) = ev["conferenceData"]["entryPoints"].as_array() {
        for e in eps {
            if s(e, "entryPointType") == "video" && !s(e, "uri").is_empty() {
                return s(e, "uri").to_owned();
            }
        }
    }
    s(ev, "hangoutLink").to_owned()
}

fn person(p: &Value) -> Option<String> {
    let name = s(p, "displayName").trim();
    let email = s(p, "email").trim();
    match (name.is_empty(), email.is_empty()) {
        (false, false) => Some(format!("{name} <{email}>")),
        (false, true) => Some(name.to_owned()),
        (true, false) => Some(format!("<{email}>")),
        _ => None,
    }
}

fn detail_lines(ev: &Value, prefix: &str, indent: &str, attachments: bool) -> String {
    let mut lines = vec![
        format!(
            "{prefix}Description: {}",
            ev.get("description")
                .and_then(Value::as_str)
                .unwrap_or("No Description")
        ),
        format!(
            "{prefix}Location: {}",
            ev.get("location")
                .and_then(Value::as_str)
                .unwrap_or("No Location")
        ),
        format!(
            "{prefix}Color ID: {}",
            ev.get("colorId").and_then(Value::as_str).unwrap_or("None")
        ),
    ];
    if let Some(r) = ev.get("recurringEventId").and_then(Value::as_str) {
        lines.push(format!("{prefix}Recurring Event ID: {r}"));
    }
    if let Some(r) = ev
        .get("recurrence")
        .filter(|r| r.as_array().is_some_and(|a| !a.is_empty()))
    {
        lines.push(format!(
            "{prefix}Recurrence: {}",
            serde_json::to_string(r)
                .expect("JSON")
                .replace("\",\"", "\", \"")
        ));
    }
    if let Some(t) = ev
        .get("eventType")
        .and_then(Value::as_str)
        .filter(|t| *t != "default")
    {
        lines.push(format!("{prefix}Event Type: {t}"));
    }
    if let Some(st) = ev
        .get("status")
        .and_then(Value::as_str)
        .filter(|t| *t != "confirmed")
    {
        lines.push(format!("{prefix}Status: {st}"));
    }
    if let Some(c) = person(&ev["creator"]) {
        lines.push(format!("{prefix}Creator: {c}"));
    }
    if let Some(o) = person(&ev["organizer"]) {
        lines.push(format!("{prefix}Organizer: {o}"));
    }
    let ml = meeting_link(ev);
    if !ml.is_empty() {
        lines.push(format!("{prefix}Meeting Link: {ml}"));
    }
    let atts: Vec<Value> = ev["attendees"].as_array().cloned().unwrap_or_default();
    let emails = if atts.is_empty() {
        "None".to_owned()
    } else {
        atts.iter()
            .map(|x| s(x, "email"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    lines.push(format!("{prefix}Attendees: {emails}"));
    let details = if atts.is_empty() {
        "None".to_owned()
    } else {
        atts.iter()
            .map(|x| {
                let mut p = vec![format!(
                    "{}: {}",
                    s(x, "email"),
                    x.get("responseStatus")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                )];
                if x["organizer"].as_bool().unwrap_or(false) {
                    p.push("(organizer)".into());
                }
                if x["optional"].as_bool().unwrap_or(false) {
                    p.push("(optional)".into());
                }
                let note = s(x, "comment")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if !note.is_empty() {
                    p.push(format!(
                        "(note: {})",
                        serde_json::to_string(&note).expect("JSON")
                    ));
                }
                p.join(" ")
            })
            .collect::<Vec<_>>()
            .join(&format!("\n{indent}"))
    };
    lines.push(format!("{prefix}Attendee Details: {details}"));
    if attachments {
        let list: Vec<Value> = ev["attachments"].as_array().cloned().unwrap_or_default();
        let text = if list.is_empty() {
            "None".to_owned()
        } else {
            list.iter()
                .map(|x| {
                    format!(
                        "{}\n{indent}File URL: {}\n{indent}File ID: {}\n{indent}MIME Type: {}",
                        non(s(x, "title"), "Untitled"),
                        non(s(x, "fileUrl"), "No URL"),
                        non(s(x, "fileId"), "No ID"),
                        non(s(x, "mimeType"), "Unknown")
                    )
                })
                .collect::<Vec<_>>()
                .join(&format!("\n{indent}"))
        };
        lines.push(format!("{prefix}Attachments: {text}"));
    }
    lines.iter().map(|l| format!("{l}\n")).collect()
}

fn non<'a>(v: &'a str, d: &'a str) -> &'a str {
    if v.is_empty() { d } else { v }
}

fn saved_times(ev: &Value, zone: &str) -> String {
    let mut lines = vec![
        format!("Saved start: {}", render_time(ev, "start")),
        format!("Saved end: {}", render_time(ev, "end")),
    ];
    if ev["start"]["dateTime"].is_string() && ev["end"]["dateTime"].is_string() {
        if let (Some(a), Some(b)) = (
            boundary_secs(ev, "start", zone),
            boundary_secs(ev, "end", zone),
        ) {
            let secs = b - a;
            let mins = secs as f64 / 60.0;
            let mins_text = if mins.fract() == 0.0 {
                format!("{}", mins as i64)
            } else {
                format!("{mins}")
            };
            lines.push(format!(
                "Elapsed duration: {mins_text} minutes ({secs} seconds)"
            ));
        }
    } else if let (Some(a), Some(b)) = (ev["start"]["date"].as_str(), ev["end"]["date"].as_str())
        && let (Some(x), Some(y)) = (tz::parse_date(a), tz::parse_date(b))
    {
        let days = tz::days_from_civil(y.0, y.1, y.2) - tz::days_from_civil(x.0, x.1, x.2);
        lines.push(format!(
            "All-day span: {days} {} (end date exclusive)",
            if days == 1 { "day" } else { "days" }
        ));
    }
    format!("\n{}", lines.join("\n"))
}

fn html_link(ev: &Value) -> String {
    let eid = b64(format!("{} {}", s(ev, "id"), s(ev, "calendarId")).as_bytes());
    format!(
        "https://www.google.com/calendar/event?eid={}",
        eid.trim_end_matches('=')
    )
}

pub fn event_out(ev: &Value) -> Value {
    let mut e = ev.clone();
    if e.get("htmlLink").is_none() {
        e["htmlLink"] = json!(html_link(ev));
    }
    e
}

// ───────────────────────────── reads ─────────────────────────────

pub fn list_calendars(ctx: &mut Ctx, a: &Value) -> R {
    let items = ctx.all("calendar_calendars");
    let max = arg_i64(a, "max_results")
        .map(|m| m.max(1) as usize)
        .unwrap_or(100);
    let offset: usize = arg(a, "page_token")
        .and_then(|t| t.parse().ok())
        .unwrap_or(0);
    let page: Vec<&Value> = items.iter().skip(offset).take(max).collect();
    let next = (items.len() > offset + max).then(|| (offset + max).to_string());
    if page.is_empty() {
        return Ok(format!("No calendars found for {USER}."));
    }
    let lines: Vec<String> = page
        .iter()
        .map(|c| {
            let mut l = format!(
                "- \"{}\"{} (ID: {})",
                non(s(c, "summary"), "No Summary"),
                if c["primary"].as_bool().unwrap_or(false) {
                    " (Primary)"
                } else {
                    ""
                },
                s(c, "id")
            );
            if !s(c, "timeZone").is_empty() {
                l.push_str(&format!(" (Time zone: {})", s(c, "timeZone")));
            }
            l
        })
        .collect();
    let mut out = format!(
        "Successfully listed {} calendars for {USER}:\n{}",
        page.len(),
        lines.join("\n")
    );
    if let Some(n) = next {
        out.push_str(&format!("\n\nNext page token: {n}"));
    }
    Ok(out)
}

fn normalize_bound(text: &str) -> Option<i64> {
    let t = text.trim().trim_matches('"').trim_matches('\'');
    if t.is_empty() || t.eq_ignore_ascii_case("null") || t.eq_ignore_ascii_case("none") {
        return None;
    }
    parse_datetime(t, Some("UTC")).map(|(x, _)| x)
}

pub fn events_in(
    ctx: &Ctx,
    cal: &str,
    min: Option<i64>,
    max: Option<i64>,
    q: Option<&str>,
) -> Vec<Value> {
    let zone = cal_zone(ctx, cal);
    let cal_id = resolve(ctx, cal);
    let q = q.map(str::to_lowercase);
    let mut list: Vec<(i64, Value)> = ctx
        .all("calendar_events")
        .into_iter()
        .filter(|e| s(e, "calendarId") == cal_id && s(e, "status") != "cancelled")
        .filter_map(|e| {
            let st = boundary_secs(&e, "start", &zone)?;
            let en = boundary_secs(&e, "end", &zone).unwrap_or(st);
            if min.is_some_and(|m| en <= m) || max.is_some_and(|m| st >= m) {
                return None;
            }
            if let Some(q) = &q {
                let atts: Vec<String> = e["attendees"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|x| format!("{} {}", s(x, "email"), s(x, "displayName")))
                            .collect()
                    })
                    .unwrap_or_default();
                let hay = format!(
                    "{} {} {} {}",
                    s(&e, "summary"),
                    s(&e, "description"),
                    s(&e, "location"),
                    atts.join(" ")
                )
                .to_lowercase();
                if !q.split_whitespace().all(|w| hay.contains(w)) {
                    return None;
                }
            }
            Some((st, e))
        })
        .collect();
    list.sort_by_key(|(st, _)| *st);
    list.into_iter().map(|(_, e)| e).collect()
}

pub fn get_events(ctx: &mut Ctx, a: &Value) -> R {
    let cal = arg(a, "calendar_id").unwrap_or("primary").to_owned();
    let detailed = arg_bool(a, "detailed").unwrap_or(false);
    let include_att = arg_bool(a, "include_attachments").unwrap_or(false);
    let single = arg_bool(a, "single_events").unwrap_or(true);
    if calendar(ctx, &cal).is_none() {
        return Err(not_found(
            "get_events",
            &format!("{BASE}/calendars/{cal}/events?alt=json"),
        ));
    }
    let cal_id = resolve(ctx, &cal);
    let mut pagination = String::new();
    let items: Vec<Value> = if let Some(eid) = arg(a, "event_id") {
        let ev = ctx
            .get("calendar_events", eid)
            .filter(|e| s(e, "calendarId") == cal_id)
            .ok_or_else(|| {
                not_found(
                    "get_events",
                    &format!("{BASE}/calendars/{cal}/events/{eid}?alt=json"),
                )
            })?;
        vec![ev]
    } else {
        let time_min = arg(a, "time_min").and_then(normalize_bound);
        if arg(a, "page_token").is_some() && time_min.is_none() && single {
            return Err("[get_events] page_token requires time_min. Pass the Pagination time_min from the previous response to preserve the original range.".into());
        }
        let effective_min = time_min.or(if single { Some(ctx.now) } else { None });
        let time_max = arg(a, "time_max").and_then(normalize_bound);
        let all = events_in(ctx, &cal, effective_min, time_max, arg(a, "query"));
        let max = arg_i64(a, "max_results").unwrap_or(25).max(1) as usize;
        let offset: usize = arg(a, "page_token")
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        if all.len() > offset + max {
            pagination = format!("Next page token: {}", offset + max);
            if let Some(m) = effective_min {
                pagination.push_str(&format!("\nPagination time_min: {}", tz::rfc3339(m)));
            }
        }
        all.into_iter().skip(offset).take(max).collect()
    };
    if items.is_empty() {
        if !pagination.is_empty() {
            return Ok(format!(
                "No events on this page in calendar '{cal}' for {USER} for the specified time range, and more pages remain.\n{pagination}"
            ));
        }
        return Ok(format!(
            "No events found in calendar '{cal}' for {USER} for the specified time range."
        ));
    }
    if let Some(eid) = arg(a, "event_id")
        && detailed
    {
        let ev = event_out(&items[0]);
        let mut out = format!(
            "Event Details:\n- Title: {}\n- Starts: {}\n- Ends: {}\n",
            non(s(&ev, "summary"), "No Title"),
            render_time(&ev, "start"),
            render_time(&ev, "end")
        );
        out.push_str(&detail_lines(&ev, "- ", "  ", include_att));
        out.push_str(&format!(
            "- Event ID: {eid}\n- Link: {}",
            s(&ev, "htmlLink")
        ));
        return Ok(out);
    }
    let lines: Vec<String> = items
        .iter()
        .map(|e| {
            let ev = event_out(e);
            let title = non(s(&ev, "summary"), "No Title");
            let head = format!(
                "- \"{title}\" (Starts: {}, Ends: {})",
                render_time(&ev, "start"),
                render_time(&ev, "end")
            );
            if detailed {
                format!(
                    "{head}\n{}  ID: {} | Link: {}",
                    detail_lines(&ev, "  ", "    ", include_att),
                    s(&ev, "id"),
                    s(&ev, "htmlLink")
                )
            } else {
                let mut l = head;
                if s(&ev, "status") == "cancelled" {
                    l.push_str(" Status: cancelled");
                }
                let ml = meeting_link(&ev);
                if !ml.is_empty() {
                    l.push_str(&format!(" Meeting: {ml}"));
                }
                l.push_str(&format!(
                    " ID: {} | Link: {}",
                    s(&ev, "id"),
                    s(&ev, "htmlLink")
                ));
                l
            }
        })
        .collect();
    if arg(a, "event_id").is_some() {
        return Ok(format!(
            "Successfully retrieved event from calendar '{cal}' for {USER}:\n{}",
            lines.join("\n")
        ));
    }
    let mut out = format!(
        "Successfully retrieved {} events from calendar '{cal}' for {USER}:\n{}",
        items.len(),
        lines.join("\n")
    );
    if !pagination.is_empty() {
        out.push_str(&format!("\n\n{pagination}"));
    }
    Ok(out)
}

// ───────────────────────────── writes ─────────────────────────────

/// `_build_time_boundary` plus the API's own normalization into the event zone.
fn boundary(value: &str, zone: Option<&str>, cal_zone: &str) -> Result<Value, String> {
    if !value.contains('T') {
        if tz::parse_date(value).is_none() {
            return Err(format!(
                "Invalid RFC3339 timestamp {}. Use a value such as '2026-09-17T11:35:00' or '2026-09-17T11:35:00-07:00'.",
                crate::util::py_str(value)
            ));
        }
        return Ok(json!({"date": value}));
    }
    let Some((t, explicit)) = parse_datetime(value, zone) else {
        if zone.is_none() && parse_datetime(value, Some("UTC")).is_some() {
            return Err("Missing time zone definition for start time.".into());
        }
        return Err(format!(
            "Invalid RFC3339 timestamp {}. Use a value such as '2026-09-17T11:35:00' or '2026-09-17T11:35:00-07:00'.",
            crate::util::py_str(value)
        ));
    };
    let _ = explicit;
    let z = zone.unwrap_or(cal_zone);
    Ok(json!({"dateTime": format_in_zone(t, z), "timeZone": z}))
}

fn check_zone(z: Option<&str>) -> Result<(), String> {
    match z {
        Some(z)
            if !matches!(
                z,
                "UTC"
                    | "America/New_York"
                    | "America/Chicago"
                    | "America/Denver"
                    | "America/Los_Angeles"
                    | "Europe/London"
                    | "Europe/Berlin"
                    | "Europe/Paris"
                    | "Asia/Kolkata"
            ) && !z.contains('/') =>
        {
            Err(format!(
                "Unrecognized IANA timezone {}. Use a zone name such as 'America/New_York' or 'Europe/Amsterdam'.",
                crate::util::py_str(z)
            ))
        }
        _ => Ok(()),
    }
}

fn attendees(a: &Value) -> Result<Option<Vec<Value>>, String> {
    let Some(raw) = arg_json(a, "attendees") else {
        return Ok(None);
    };
    let list =
        match raw {
            Value::Array(l) => l,
            Value::String(x) => x.split(',').map(|e| json!(e.trim())).collect(),
            _ => return Err(
                "Each attendee must be an email string or an object with an 'email' key; got dict"
                    .into(),
            ),
        };
    let mut out = Vec::new();
    for x in list {
        match x {
            Value::String(e) => out.push(json!({"email": e, "responseStatus": "needsAction"})),
            Value::Object(ref o) if o.contains_key("email") => {
                let mut v = x.clone();
                if v.get("responseStatus").is_none() {
                    v["responseStatus"] = json!("needsAction");
                }
                out.push(v);
            }
            other => {
                return Err(format!(
                    "Each attendee must be an email string or an object with an 'email' key; got {}",
                    match other {
                        Value::Number(_) => "int",
                        Value::Bool(_) => "bool",
                        Value::Object(_) => "dict",
                        _ => "NoneType",
                    }
                ));
            }
        }
    }
    Ok(Some(out))
}

const COLORS: [&str; 11] = [
    "lavender",
    "sage",
    "grape",
    "flamingo",
    "banana",
    "tangerine",
    "peacock",
    "graphite",
    "blueberry",
    "basil",
    "tomato",
];

fn color_id(a: &Value) -> Result<Option<String>, String> {
    match arg(a, "color_id") {
        None => Ok(None),
        Some(c) => {
            if let Some(i) = COLORS.iter().position(|n| n.eq_ignore_ascii_case(c)) {
                return Ok(Some((i + 1).to_string()));
            }
            match c.parse::<usize>() {
                Ok(n) if (1..=11).contains(&n) => Ok(Some(n.to_string())),
                _ => Err(format!(
                    "Invalid color_id '{c}'. Use a name (lavender, sage, grape, flamingo, banana, tangerine, peacock, graphite, blueberry, basil, tomato) or an ID 1-11."
                )),
            }
        }
    }
}

fn apply_common(ev: &mut Value, a: &Value) -> Result<(), String> {
    for (k, f) in [
        ("summary", "summary"),
        ("description", "description"),
        ("location", "location"),
    ] {
        if let Some(v) = a.get(k).and_then(Value::as_str) {
            ev[f] = json!(v);
        }
    }
    if let Some(att) = attendees(a)? {
        ev["attendees"] = json!(att);
    }
    if let Some(c) = color_id(a)? {
        ev["colorId"] = json!(c);
    }
    if let Some(r) = arg_list(a, "recurrence") {
        ev["recurrence"] = json!(r);
    }
    for (k, f) in [
        ("transparency", "transparency"),
        ("visibility", "visibility"),
    ] {
        if let Some(v) = arg(a, k) {
            ev[f] = json!(v);
        }
    }
    for (k, f) in [
        ("guests_can_modify", "guestsCanModify"),
        ("guests_can_invite_others", "guestsCanInviteOthers"),
        ("guests_can_see_other_guests", "guestsCanSeeOtherGuests"),
    ] {
        if let Some(v) = arg_bool(a, k) {
            ev[f] = json!(v);
        }
    }
    if let Some(r) = arg_json(a, "reminders") {
        ev["reminders"] = json!({"useDefault": false, "overrides": r});
    } else if arg_bool(a, "use_default_reminders") == Some(false) {
        ev["reminders"] = json!({"useDefault": false});
    }
    if let Some(att) = arg_list(a, "attachments") {
        ev["attachments"] = json!(att.iter().map(|f| {
            let id = f.rsplit('/').find(|p| p.len() > 20).unwrap_or(f).to_owned();
            json!({"fileId": id, "fileUrl": format!("https://drive.google.com/open?id={id}"), "title": id, "mimeType": "application/octet-stream"})
        }).collect::<Vec<_>>());
    }
    Ok(())
}

fn conference(ctx: &mut Ctx, ev: &mut Value, a: &Value) -> Result<Option<&'static str>, String> {
    let provider = arg(a, "conference_provider");
    let raw = arg_json(a, "conference_data");
    let meet = arg_bool(a, "add_google_meet");
    if (provider.is_some() || raw.is_some()) && meet == Some(true) {
        return Err(
            "add_google_meet cannot be combined with conference_data or conference_provider."
                .into(),
        );
    }
    if provider.is_some() && raw.is_some() {
        return Err("conference_data and conference_provider are mutually exclusive.".into());
    }
    if let Some(p) = provider {
        let uri = arg(a, "conference_uri").ok_or_else(|| {
            "conference_uri is required when conference_provider is set.".to_owned()
        })?;
        ev["conferenceData"] = json!({"conferenceSolution": {"key": {"type": "addOn"}, "name": p}, "entryPoints": [{"entryPointType": "video", "uri": uri, "passcode": arg(a, "conference_passcode")}], "conferenceId": arg(a, "conference_id")});
        return Ok(Some("Conference"));
    }
    if let Some(r) = raw {
        ev["conferenceData"] = r;
        return Ok(Some("Conference"));
    }
    match meet {
        Some(true) => {
            let code = ctx.hex_id("meet", 10);
            let code: String = code
                .chars()
                .map(|c| ((c as u8 % 26) + b'a') as char)
                .collect();
            let uri = format!(
                "https://meet.google.com/{}-{}-{}",
                &code[..3],
                &code[3..7],
                &code[7..10]
            );
            ev["conferenceData"] = json!({"entryPoints": [{"entryPointType": "video", "uri": uri, "label": uri.trim_start_matches("https://")}], "conferenceSolution": {"key": {"type": "hangoutsMeet"}, "name": "Google Meet"}});
            ev["hangoutLink"] = json!(uri);
            Ok(Some("Google Meet"))
        }
        Some(false) => {
            if let Some(m) = ev.as_object_mut() {
                m.remove("conferenceData");
                m.remove("hangoutLink");
            }
            Ok(None)
        }
        None => Ok(None),
    }
}

pub fn manage_event(ctx: &mut Ctx, a: &Value) -> R {
    let action = s(a, "action").trim().to_lowercase();
    if let Some(su) = arg(a, "send_updates")
        && !["all", "externalOnly", "none"].contains(&su)
    {
        return Err(format!(
            "Invalid send_updates '{su}'. Must be one of: ['all', 'externalOnly', 'none']"
        ));
    }
    if arg_bool(a, "this_and_following").unwrap_or(false) && action != "update" {
        return Err("this_and_following applies only to the update action.".into());
    }
    let cal = arg(a, "calendar_id").unwrap_or("primary").to_owned();
    let zone_arg = arg(a, "timezone");
    let start_zone = arg(a, "start_timezone").or(zone_arg);
    let end_zone = arg(a, "end_timezone").or(zone_arg);
    match action.as_str() {
        "create" => {
            let (Some(summary), Some(st), Some(en)) =
                (arg(a, "summary"), arg(a, "start_time"), arg(a, "end_time"))
            else {
                return Err(
                    "summary, start_time, and end_time are required for create action".into(),
                );
            };
            check_zone(start_zone)?;
            check_zone(end_zone)?;
            if calendar(ctx, &cal).is_none() {
                return Err(not_found(
                    "manage_event",
                    &format!(
                        "{BASE}/calendars/{cal}/events?conferenceDataVersion=0&sendUpdates=all&alt=json"
                    ),
                ));
            }
            let cz = cal_zone(ctx, &cal);
            let start = boundary(st, start_zone, &cz).map_err(|e| api400(&cal, &e))?;
            let end = boundary(en, end_zone, &cz).map_err(|e| api400(&cal, &e))?;
            let cal_id = resolve(ctx, &cal);
            let id = ctx.hex_id("event", 26);
            let now = ctx.rfc3339_ms();
            let mut ev = json!({
                "id": id, "calendarId": cal_id, "status": "confirmed", "summary": summary, "start": start, "end": end,
                "organizer": {"email": cal_id, "self": true}, "creator": {"email": USER, "self": true},
                "created": now, "updated": now, "eventType": "default", "reminders": {"useDefault": true},
                "iCalUID": format!("{id}@google.com"), "sequence": 0,
            });
            apply_common(&mut ev, a)?;
            if let (Some(x), Some(y)) = (
                boundary_secs(&ev, "start", &cz),
                boundary_secs(&ev, "end", &cz),
            ) && y < x
            {
                return Err(api400(&cal, "The specified time range is empty."));
            }
            let label = conference(ctx, &mut ev, a)?;
            ev["htmlLink"] = json!(html_link(&ev));
            ctx.put("calendar_events", ev.clone());
            let mut msg = format!(
                "Successfully created event '{summary}' for {USER}. Link: {}",
                s(&ev, "htmlLink")
            );
            msg.push_str(&saved_times(&ev, &cz));
            if let Some(l) = label {
                let ml = meeting_link(&ev);
                if !ml.is_empty() {
                    msg.push_str(&format!(" {l}: {ml}"));
                }
            }
            Ok(msg)
        }
        "update" => {
            let eid = arg(a, "event_id")
                .ok_or_else(|| "event_id is required for update action".to_owned())?
                .to_owned();
            check_zone(start_zone)?;
            check_zone(end_zone)?;
            let cal_id = resolve(ctx, &cal);
            let mut ev = ctx.get("calendar_events", &eid).filter(|e| s(e, "calendarId") == cal_id).ok_or_else(|| {
                format!("Event not found during verification. The event with ID '{eid}' could not be found in calendar '{cal}'. This may be due to incorrect ID format or the event no longer exists.")
            })?;
            let cz = cal_zone(ctx, &cal);
            let mut changed = false;
            if let Some(st) = arg(a, "start_time") {
                ev["start"] = boundary(
                    st,
                    start_zone
                        .or(ev["start"]["timeZone"].as_str())
                        .map(str::to_owned)
                        .as_deref(),
                    &cz,
                )
                .map_err(|e| api400(&cal, &e))?;
                changed = true;
            }
            if let Some(en) = arg(a, "end_time") {
                ev["end"] = boundary(
                    en,
                    end_zone
                        .or(ev["end"]["timeZone"].as_str())
                        .map(str::to_owned)
                        .as_deref(),
                    &cz,
                )
                .map_err(|e| api400(&cal, &e))?;
                changed = true;
            }
            let before = ev.clone();
            apply_common(&mut ev, a)?;
            let label = conference(ctx, &mut ev, a)?;
            changed |= before != ev || arg_bool(a, "add_google_meet").is_some();
            if !changed {
                return Err("No fields provided to modify the event.".into());
            }
            if let (Some(x), Some(y)) = (
                boundary_secs(&ev, "start", &cz),
                boundary_secs(&ev, "end", &cz),
            ) && y < x
            {
                return Err(api400(&cal, "The specified time range is empty."));
            }
            ev["updated"] = json!(ctx.rfc3339_ms());
            ev["sequence"] = json!(ev["sequence"].as_i64().unwrap_or(0) + 1);
            ev["htmlLink"] = json!(html_link(&ev));
            ctx.put("calendar_events", ev.clone());
            let mut msg = format!(
                "Successfully modified event '{}' (ID: {eid}) for {USER}. Link: {}",
                s(&ev, "summary"),
                s(&ev, "htmlLink")
            );
            msg.push_str(&saved_times(&ev, &cz));
            match (label, arg_bool(a, "add_google_meet")) {
                (Some(l), _) => {
                    let ml = meeting_link(&ev);
                    if !ml.is_empty() {
                        msg.push_str(&format!(" {l}: {ml}"));
                    }
                }
                (None, Some(false)) => msg.push_str(" (Google Meet removed)"),
                _ => {}
            }
            Ok(msg)
        }
        "delete" => {
            let eid = arg(a, "event_id")
                .ok_or_else(|| "event_id is required for delete action".to_owned())?
                .to_owned();
            let cal_id = resolve(ctx, &cal);
            let mut ev = ctx.get("calendar_events", &eid).filter(|e| s(e, "calendarId") == cal_id && s(e, "status") != "cancelled").ok_or_else(|| {
                format!("Event not found during verification. The event with ID '{eid}' could not be found in calendar '{cal}'. This may be due to incorrect ID format or the event no longer exists.")
            })?;
            ev["status"] = json!("cancelled");
            ev["updated"] = json!(ctx.rfc3339_ms());
            ctx.put("calendar_events", ev);
            Ok(format!(
                "Successfully deleted event (ID: {eid}) from calendar '{cal}' for {USER}."
            ))
        }
        "rsvp" => {
            let eid = arg(a, "event_id")
                .ok_or_else(|| "event_id is required for rsvp action".to_owned())?
                .to_owned();
            let response = arg(a, "response")
                .ok_or_else(|| "response is required for rsvp action".to_owned())?
                .to_owned();
            if !["accepted", "declined", "tentative", "needsAction"].contains(&response.as_str()) {
                return Err(format!(
                    "Invalid response '{response}'. Must be one of: ['accepted', 'declined', 'needsAction', 'tentative']"
                ));
            }
            let cal_id = resolve(ctx, &cal);
            let mut ev = ctx
                .get("calendar_events", &eid)
                .filter(|e| s(e, "calendarId") == cal_id)
                .ok_or_else(|| {
                    not_found(
                        "manage_event",
                        &format!("{BASE}/calendars/{cal}/events/{eid}?alt=json"),
                    )
                })?;
            let atts = ev["attendees"].as_array().cloned().unwrap_or_default();
            if atts.is_empty() {
                return Err("This event has no attendee list; cannot update RSVP.".into());
            }
            if ev["organizer"]["self"].as_bool().unwrap_or(false) {
                return Err("You are the organizer of this event. Organizers cannot respond to their own invitations.".into());
            }
            let idx = atts
                .iter()
                .position(|x| x["self"].as_bool().unwrap_or(false) || s(x, "email") == USER)
                .ok_or_else(|| format!("{USER} was not found in the event's attendee list."))?;
            ev["attendees"][idx]["responseStatus"] = json!(response);
            if let Some(c) = a.get("rsvp_comment").and_then(Value::as_str) {
                ev["attendees"][idx]["comment"] = json!(c);
            }
            ev["updated"] = json!(ctx.rfc3339_ms());
            ctx.put("calendar_events", ev.clone());
            Ok(format!(
                "Successfully updated RSVP for '{}' (ID: {eid}) to '{response}' for {USER}.",
                non(s(&ev, "summary"), "Unknown event")
            ))
        }
        other => Err(format!(
            "Invalid action '{other}'. Must be 'create', 'update', 'delete', or 'rsvp'."
        )),
    }
}

fn api400(cal: &str, message: &str) -> String {
    format!(
        "API error in manage_event: {}",
        http_error(
            400,
            &format!("{BASE}/calendars/{cal}/events?alt=json"),
            message,
            "invalid"
        )
    )
}

/// Out-of-office and focus-time events share one implementation.
pub fn manage_status_event(ctx: &mut Ctx, a: &Value, kind: &str) -> R {
    let (event_type, label, default_summary, props_key) = if kind == "ooo" {
        (
            "outOfOffice",
            "Out of Office",
            "Out of Office",
            "outOfOfficeProperties",
        )
    } else {
        (
            "focusTime",
            "Focus Time",
            "Focus Time",
            "focusTimeProperties",
        )
    };
    let tool = if kind == "ooo" {
        "manage_out_of_office"
    } else {
        "manage_focus_time"
    };
    let action = s(a, "action").trim().to_lowercase();
    let cal = arg(a, "calendar_id").unwrap_or("primary").to_owned();
    let cal_id = resolve(ctx, &cal);
    let cz = cal_zone(ctx, &cal);
    let zone = arg(a, "timezone")
        .map(str::to_owned)
        .unwrap_or_else(|| cz.clone());
    let to_dt = |v: &str| -> Result<Value, String> {
        let (t, _) =
            parse_datetime(v, Some(&zone)).ok_or_else(|| format!("Invalid date/time '{v}'."))?;
        Ok(json!({"dateTime": format_in_zone(t, &zone), "timeZone": zone}))
    };
    let decline = arg(a, "auto_decline_mode")
        .unwrap_or("declineAllConflictingInvitations")
        .to_owned();
    match action.as_str() {
        "create" => {
            let (Some(st), Some(en)) = (arg(a, "start_time"), arg(a, "end_time")) else {
                return Err("start_time and end_time are required for create action".into());
            };
            let id = ctx.hex_id("event", 26);
            let summary = arg(a, "summary").unwrap_or(default_summary).to_owned();
            let mut props = json!({"autoDeclineMode": decline});
            if let Some(m) = arg(a, "decline_message") {
                props["declineMessage"] = json!(m);
            }
            if kind == "focus" {
                props["chatStatus"] = json!(arg(a, "chat_status").unwrap_or("doNotDisturb"));
            }
            let now = ctx.rfc3339_ms();
            let mut ev = json!({
                "id": id, "calendarId": cal_id, "status": "confirmed", "summary": summary, "eventType": event_type,
                "start": to_dt(st)?, "end": to_dt(en)?, props_key: props, "transparency": "opaque",
                "organizer": {"email": cal_id, "self": true}, "creator": {"email": USER, "self": true}, "created": now, "updated": now,
            });
            if let Some(d) = arg(a, "description") {
                ev["description"] = json!(d);
            }
            if let Some(r) = arg_list(a, "recurrence") {
                ev["recurrence"] = json!(r);
            }
            ev["htmlLink"] = json!(html_link(&ev));
            ctx.put("calendar_events", ev.clone());
            let mut out = format!(
                "Successfully created {label} event for {USER}.\n- Summary: {summary}\n- Start: {}\n- End: {}\n- Auto-decline: {decline}\n- Decline message: {}\n",
                s(&ev["start"], "dateTime"),
                s(&ev["end"], "dateTime"),
                arg(a, "decline_message").unwrap_or("(none)")
            );
            if kind == "focus" {
                out.push_str(&format!(
                    "- Chat status: {}\n",
                    arg(a, "chat_status").unwrap_or("(default)")
                ));
            }
            out.push_str(&format!("- Event ID: {id}\n- Link: {}", s(&ev, "htmlLink")));
            Ok(out)
        }
        "list" => {
            let min = arg(a, "time_min")
                .and_then(normalize_bound)
                .unwrap_or(ctx.now);
            let max = arg(a, "time_max").and_then(normalize_bound);
            let items: Vec<Value> = events_in(ctx, &cal, Some(min), max, None)
                .into_iter()
                .filter(|e| s(e, "eventType") == event_type)
                .take(arg_i64(a, "max_results").unwrap_or(10).max(1) as usize)
                .collect();
            if items.is_empty() {
                return Ok(format!(
                    "No {} events found for {USER}.",
                    if kind == "ooo" {
                        "out-of-office"
                    } else {
                        "Focus Time"
                    }
                ));
            }
            let mut lines = vec![format!(
                "Found {} {} event(s) for {USER}:\n",
                items.len(),
                if kind == "ooo" {
                    "out-of-office"
                } else {
                    "Focus Time"
                }
            )];
            for (i, e) in items.iter().enumerate() {
                let st = e["start"]
                    .get("date")
                    .or(e["start"].get("dateTime"))
                    .and_then(Value::as_str)
                    .unwrap_or("N/A");
                let en = e["end"]
                    .get("date")
                    .or(e["end"].get("dateTime"))
                    .and_then(Value::as_str)
                    .unwrap_or("N/A");
                lines.push(format!(
                    "{}. \"{}\" ({st} to {en})",
                    i + 1,
                    non(s(e, "summary"), default_summary)
                ));
                lines.push(format!(
                    "   Auto-decline: {}",
                    e[props_key]
                        .get("autoDeclineMode")
                        .and_then(Value::as_str)
                        .unwrap_or("N/A")
                ));
                if let Some(m) = e[props_key]
                    .get("declineMessage")
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty())
                {
                    lines.push(format!("   Decline message: {m}"));
                }
                if kind == "focus"
                    && let Some(c) = e[props_key].get("chatStatus").and_then(Value::as_str)
                {
                    lines.push(format!("   Chat status: {c}"));
                }
                lines.push(format!("   Event ID: {}", s(e, "id")));
                lines.push(String::new());
            }
            Ok(lines.join("\n"))
        }
        "update" => {
            let eid = arg(a, "event_id")
                .ok_or_else(|| "event_id is required for update action".to_owned())?
                .to_owned();
            let mut ev = ctx
                .get("calendar_events", &eid)
                .filter(|e| s(e, "calendarId") == cal_id && s(e, "eventType") == event_type)
                .ok_or_else(|| {
                    not_found(
                        tool,
                        &format!("{BASE}/calendars/{cal}/events/{eid}?alt=json"),
                    )
                })?;
            let before = ev.clone();
            if let Some(st) = arg(a, "start_time") {
                ev["start"] = to_dt(st)?;
            }
            if let Some(en) = arg(a, "end_time") {
                ev["end"] = to_dt(en)?;
            }
            if let Some(su) = arg(a, "summary") {
                ev["summary"] = json!(su);
            }
            if let Some(m) = arg(a, "auto_decline_mode") {
                ev[props_key]["autoDeclineMode"] = json!(m);
            }
            if let Some(m) = arg(a, "decline_message") {
                ev[props_key]["declineMessage"] = json!(m);
            }
            if let Some(c) = arg(a, "chat_status") {
                ev[props_key]["chatStatus"] = json!(c);
            }
            if before == ev {
                return Ok(format!("No changes specified for {label} event '{eid}'."));
            }
            ev["updated"] = json!(ctx.rfc3339_ms());
            ctx.put("calendar_events", ev.clone());
            Ok(format!(
                "Successfully updated {label} event (ID: {eid}) for {USER}.\n- Summary: {}\n- Start: {}\n- End: {}\n- Link: {}",
                non(s(&ev, "summary"), default_summary),
                s(&ev["start"], "dateTime"),
                s(&ev["end"], "dateTime"),
                html_link(&ev)
            ))
        }
        "delete" => {
            let eid = arg(a, "event_id")
                .ok_or_else(|| "event_id is required for delete action".to_owned())?
                .to_owned();
            let mut ev = ctx
                .get("calendar_events", &eid)
                .filter(|e| s(e, "calendarId") == cal_id)
                .ok_or_else(|| {
                    not_found(
                        tool,
                        &format!("{BASE}/calendars/{cal}/events/{eid}?alt=json"),
                    )
                })?;
            ev["status"] = json!("cancelled");
            ctx.put("calendar_events", ev);
            Ok(format!(
                "Successfully deleted {label} event (ID: {eid}) from calendar '{cal}' for {USER}."
            ))
        }
        other => Err(format!(
            "Invalid action '{other}'. Must be 'create', 'list', 'update', or 'delete'."
        )),
    }
}

pub fn freebusy(ctx: &mut Ctx, a: &Value) -> R {
    let min = normalize_bound(s(a, "time_min")).ok_or_else(|| {
        format!(
            "API error in query_freebusy: {}",
            http_error(
                400,
                &format!("{BASE}/freeBusy?alt=json"),
                "Bad Request",
                "badRequest"
            )
        )
    })?;
    let max = normalize_bound(s(a, "time_max")).ok_or_else(|| {
        format!(
            "API error in query_freebusy: {}",
            http_error(
                400,
                &format!("{BASE}/freeBusy?alt=json"),
                "Bad Request",
                "badRequest"
            )
        )
    })?;
    if max <= min {
        return Err(format!(
            "API error in query_freebusy: {}",
            http_error(
                400,
                &format!("{BASE}/freeBusy?alt=json"),
                "The specified time range is empty.",
                "timeRangeEmpty"
            )
        ));
    }
    let ids = arg_list(a, "calendar_ids")
        .filter(|x| !x.is_empty())
        .unwrap_or_else(|| vec!["primary".into()]);
    let mut lines = vec![
        format!("Free/Busy information for {USER}:"),
        format!("Time range: {} to {}", tz::rfc3339(min), tz::rfc3339(max)),
        String::new(),
    ];
    let known_people: Map<String, Value> = ctx
        .get("calendar_freebusy", "people")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    for id in ids {
        lines.push(format!("Calendar: {id}"));
        let busy: Vec<(i64, i64)> = if calendar(ctx, &id).is_some() {
            let zone = cal_zone(ctx, &id);
            events_in(ctx, &id, Some(min), Some(max), None)
                .iter()
                .filter(|e| s(e, "transparency") != "transparent")
                .filter(|e| {
                    !e["attendees"].as_array().is_some_and(|at| {
                        at.iter().any(|x| {
                            (x["self"].as_bool().unwrap_or(false) || s(x, "email") == USER)
                                && s(x, "responseStatus") == "declined"
                        })
                    })
                })
                .filter_map(|e| {
                    Some((
                        boundary_secs(e, "start", &zone)?.max(min),
                        boundary_secs(e, "end", &zone)?.min(max),
                    ))
                })
                .collect()
        } else if let Some(periods) = known_people.get(&id).and_then(Value::as_array) {
            periods
                .iter()
                .filter_map(|p| {
                    Some((
                        normalize_bound(s(p, "start"))?,
                        normalize_bound(s(p, "end"))?,
                    ))
                })
                .filter(|(st, en)| *en > min && *st < max)
                .map(|(st, en)| (st.max(min), en.min(max)))
                .collect()
        } else {
            lines.push("  Errors:".into());
            lines.push("    - global: notFound".into());
            lines.push(String::new());
            continue;
        };
        // Merge overlapping periods, as the API does.
        let mut sorted = busy;
        sorted.sort();
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (st, en) in sorted {
            match merged.last_mut() {
                Some(last) if st <= last.1 => last.1 = last.1.max(en),
                _ => merged.push((st, en)),
            }
        }
        if merged.is_empty() {
            lines.push("  Status: Free (no busy periods)".into());
        } else {
            lines.push(format!("  Busy periods: {}", merged.len()));
            for (st, en) in merged {
                lines.push(format!("    - {} to {}", tz::rfc3339(st), tz::rfc3339(en)));
            }
        }
        lines.push(String::new());
    }
    Ok(lines.join("\n"))
}

pub fn create_calendar(ctx: &mut Ctx, a: &Value) -> R {
    let summary = s(a, "summary").to_owned();
    let id = format!("c_{}@group.calendar.google.com", ctx.hex_id("calendar", 52));
    let mut c = json!({"id": id, "summary": summary, "timeZone": arg(a, "timezone").unwrap_or("UTC"), "accessRole": "owner", "primary": false});
    if let Some(d) = arg(a, "description") {
        c["description"] = json!(d);
    }
    ctx.put("calendar_calendars", c);
    Ok(format!("Created calendar '{summary}' (ID: {id})"))
}
