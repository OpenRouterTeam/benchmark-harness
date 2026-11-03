//! Slack Lists: typed columns and records.
//!
//! Stored as `lists/<id>`: `{id, name, description, icon, owner, co_owners, columns: [{key,
//! name, type, options: {choices: [{value, label, color}]}}], records: [{id, fields: {key:
//! value}, created_by, created}], next_col, next_rec}`.
//! Values: text/email/phone/date → string, number/currency/rating → number, checkbox → bool,
//! select/multi_select → array of option values, user/channel → array of IDs.

use serde_json::{Map, Value, json};

use crate::{
    model::{CALLER, Ctx, display_name},
    tools::{ToolResult, err},
    tz,
    util::{arg_bool, arg_i64, arg_str, cursor, parse_cursor, s},
};

const COLORS: &[&str] = &[
    "blue", "green", "yellow", "red", "purple", "orange", "cyan", "pink", "indigo", "brown", "gray",
];

const CREATE_TYPES: &[&str] = &[
    "text",
    "rich_text",
    "number",
    "currency",
    "rating",
    "select",
    "multi_select",
    "date",
    "user",
    "checkbox",
    "email",
    "phone",
    "channel",
    "attachment",
];

const UPDATE_TYPES: &[&str] = &[
    "text",
    "rich_text",
    "message",
    "link",
    "number",
    "date",
    "user",
    "attachment",
    "checkbox",
    "email",
    "phone",
    "channel",
    "rating",
    "created_by",
    "last_edited_by",
    "created_time",
    "last_edited_time",
    "vote",
    "assignee",
    "due_date",
    "completed",
    "select",
    "multi_select",
];

fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_owned()
}

fn hashed_id(prefix: &str, seed: &str, n: u64) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in seed.bytes().chain(n.to_be_bytes()) {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    const A: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut out = String::from(prefix);
    let mut x = h;
    for _ in 0..9 {
        out.push(A[(x % 36) as usize] as char);
        x /= 36;
    }
    out
}

fn parse_choices(options: Option<&Value>) -> Result<Vec<Value>, String> {
    let raw = match options {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Object(o)) => o
            .get("choices")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| err("invalid_columns: options must contain a choices array"))?,
        Some(_) => {
            return Err(err(
                "invalid_columns: options must be a list or an object with choices",
            ));
        }
    };
    let mut out = Vec::new();
    for (i, c) in raw.iter().enumerate() {
        let (label, value, color) = match c {
            Value::String(l) => (l.clone(), slug(l), COLORS[i % COLORS.len()].to_owned()),
            Value::Object(_) => {
                let label = s(c, "label").to_owned();
                if label.is_empty() {
                    return Err(err("invalid_columns: every option needs a label"));
                }
                let value = match s(c, "value") {
                    "" => slug(&label),
                    v => v.to_owned(),
                };
                let color = match s(c, "color") {
                    "" => COLORS[i % COLORS.len()].to_owned(),
                    col if COLORS.contains(&col) => col.to_owned(),
                    col => return Err(err(&format!("invalid_columns: unknown color {col}"))),
                };
                (label, value, color)
            }
            _ => return Err(err("invalid_columns: invalid option")),
        };
        out.push(json!({"value": value, "label": label, "color": color}));
    }
    if out.len() > 100 {
        return Err(err("invalid_columns: at most 100 options"));
    }
    Ok(out)
}

fn make_column(list_id: &str, n: &mut u64, def: &Value, allowed: &[&str]) -> Result<Value, String> {
    let name = s(def, "name");
    if name.is_empty() {
        return Err(err("invalid_columns: every column needs a name"));
    }
    let ty = s(def, "type");
    if !allowed.contains(&ty) {
        return Err(err(&format!("invalid_columns: unknown column type {ty:?}")));
    }
    let mut col = json!({"key": hashed_id("Col0", list_id, *n), "name": name, "type": ty});
    *n += 1;
    if ty == "select" || ty == "multi_select" {
        let choices = parse_choices(def.get("options"))?;
        if choices.is_empty() {
            return Err(err(&format!(
                "invalid_columns: column {name:?} needs options"
            )));
        }
        col["options"] = json!({"choices": choices});
    }
    Ok(col)
}

fn link(ctx: &Ctx, id: &str) -> String {
    format!("{}/lists/{}/{id}", ctx.base_url(), ctx.team_id())
}

fn columns_md(cols: &[Value]) -> String {
    let mut out = String::new();
    for (i, c) in cols.iter().enumerate() {
        out.push_str(&format!(
            "- {} (type: {}, key: {})",
            s(c, "name"),
            s(c, "type"),
            s(c, "key")
        ));
        if i == 0 {
            out.push_str(" [primary]");
        }
        if let Some(ch) = c["options"]["choices"].as_array() {
            let labels: Vec<&str> = ch.iter().map(|x| s(x, "label")).collect();
            out.push_str(&format!(" options: {}", labels.join(", ")));
        }
        out.push('\n');
    }
    out
}

pub fn create(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let name = arg_str(args, "name")
        .ok_or_else(|| err("invalid_args: `name` is required"))?
        .to_owned();
    let id = ctx.new_id("list", "F0");
    let defs = args
        .get("columns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if defs.len() > 30 {
        return Err(err("over_column_maximum"));
    }
    let mut n = 0;
    let mut cols = Vec::new();
    for d in &defs {
        cols.push(make_column(&id, &mut n, d, CREATE_TYPES)?);
    }
    if cols.is_empty() {
        cols.push(make_column(
            &id,
            &mut n,
            &json!({"name": "Name", "type": "text"}),
            CREATE_TYPES,
        )?);
    } else if let Some(i) = cols.iter().position(|c| s(c, "type") == "text") {
        let primary = cols.remove(i);
        cols.insert(0, primary);
    } else {
        let task = make_column(
            &id,
            &mut n,
            &json!({"name": "Task", "type": "text"}),
            CREATE_TYPES,
        )?;
        cols.insert(0, task);
    }
    let description = arg_str(args, "description").unwrap_or("").to_owned();
    let doc = json!({
        "id": id, "name": name, "description": description, "icon": "", "owner": CALLER,
        "co_owners": [], "columns": cols, "records": [], "created": ctx.now, "next_col": n, "next_rec": 0,
    });
    ctx.put("lists", &id, doc);
    let mut out = format!(
        "## List created: {name}\n- **List ID:** {id}\n- **Link:** {}\n",
        link(ctx, &id)
    );
    if !description.is_empty() {
        out.push_str(&format!("- **Description:** {description}\n"));
    }
    out.push_str("\n### Columns\n");
    out.push_str(&columns_md(&cols));
    Ok(json!({"result": out}))
}

fn find_list(ctx: &Ctx, args: &Value) -> Result<(Value, Option<String>), String> {
    let id = arg_str(args, "list_id");
    let title = arg_str(args, "list_title");
    if id.is_none() && title.is_none() {
        return Err(err("invalid_args: provide list_id or list_title"));
    }
    if let Some(id) = id
        && let Some(l) = ctx.get("lists", id)
    {
        return Ok((l, None));
    }
    if let Some(t) = title {
        let t = t.to_lowercase();
        let mut best: Option<(usize, Value)> = None;
        for l in ctx.values("lists") {
            let name = s(&l, "name").to_lowercase();
            let score = if name == t {
                100
            } else if name.contains(&t) || t.contains(&name) {
                50
            } else {
                t.split_whitespace().filter(|w| name.contains(*w)).count()
            };
            if score > 0 && best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, l));
            }
        }
        if let Some((_, l)) = best {
            let note = id.map(|i| format!("Note: list_id {i} was not found; using the best match for list_title instead.\n\n"));
            return Ok((l, note));
        }
    }
    Err(err("list_not_found"))
}

fn render_value(ctx: &Ctx, col: &Value, v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => {
            if *b {
                "✓".into()
            } else {
                String::new()
            }
        }
        Value::Array(items) => {
            let ty = s(col, "type");
            items
                .iter()
                .map(|x| {
                    let x = x
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| x.to_string());
                    match ty {
                        "select" | "multi_select" => col["options"]["choices"]
                            .as_array()
                            .and_then(|c| c.iter().find(|o| s(o, "value") == x))
                            .map(|o| s(o, "label").to_owned())
                            .unwrap_or(x),
                        "user" | "assignee" | "created_by" | "last_edited_by" => match ctx.user(&x)
                        {
                            Some(u) => format!("{} ({x})", display_name(&u)),
                            None => x,
                        },
                        "channel" => match ctx.channel(&x) {
                            Some(c) => format!("#{} ({x})", s(&c, "name")),
                            None => x,
                        },
                        _ => x,
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
        Value::String(x) => x.clone(),
        other => other.to_string(),
    }
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', "<br>")
}

fn csv_cell(text: &str) -> String {
    if text.contains([',', '"', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

/// Markdown table of records `[offset, offset+limit)`; returns (table, more).
pub fn render_table(ctx: &Ctx, l: &Value, offset: usize, limit: usize) -> (String, bool) {
    let cols: Vec<Value> = l["columns"].as_array().cloned().unwrap_or_default();
    let records: Vec<Value> = l["records"].as_array().cloned().unwrap_or_default();
    let mut out = String::from("| Record ID |");
    for c in &cols {
        out.push_str(&format!(" {} |", cell(s(c, "name"))));
    }
    out.push_str("\n|---|");
    for _ in &cols {
        out.push_str("---|");
    }
    out.push('\n');
    for r in records.iter().skip(offset).take(limit) {
        out.push_str(&format!("| {} |", s(r, "id")));
        for c in &cols {
            let v = r["fields"].get(s(c, "key")).cloned().unwrap_or(Value::Null);
            out.push_str(&format!(" {} |", cell(&render_value(ctx, c, &v))));
        }
        out.push('\n');
    }
    (out, records.len() > offset + limit)
}

pub fn read(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let (l, note) = find_list(ctx, args)?;
    let id = s(&l, "id").to_owned();
    let cols: Vec<Value> = l["columns"].as_array().cloned().unwrap_or_default();
    let mut out = note.unwrap_or_default();
    out.push_str(&format!(
        "# {}\nList ID: {id}\nLink: {}\n",
        s(&l, "name"),
        link(ctx, &id)
    ));
    if !s(&l, "description").is_empty() {
        out.push_str(&format!("Description: {}\n", s(&l, "description")));
    }
    if arg_bool(args, "schema_only").unwrap_or(false) {
        out.push_str("\n## Columns\n");
        out.push_str(&columns_md(&cols));
        return Ok(json!({"result": out}));
    }
    let limit = arg_i64(args, "limit").unwrap_or(100).clamp(1, 100) as usize;
    let offset: usize = match arg_str(args, "cursor") {
        Some(c) => parse_cursor(c, "rec_offset")
            .and_then(|o| o.parse().ok())
            .ok_or_else(|| err("invalid_cursor"))?,
        None => 0,
    };
    let records: Vec<Value> = l["records"].as_array().cloned().unwrap_or_default();
    let total = records.len();
    out.push_str(&format!("Records: {total}\n\n"));
    let more = if arg_str(args, "format") == Some("csv") {
        let mut header = vec!["Record ID".to_owned()];
        header.extend(cols.iter().map(|c| csv_cell(s(c, "name"))));
        out.push_str(&header.join(","));
        out.push('\n');
        for r in records.iter().skip(offset).take(limit) {
            let mut row = vec![s(r, "id").to_owned()];
            for c in &cols {
                let v = r["fields"].get(s(c, "key")).cloned().unwrap_or(Value::Null);
                row.push(csv_cell(&render_value(ctx, c, &v)));
            }
            out.push_str(&row.join(","));
            out.push('\n');
        }
        total > offset + limit
    } else {
        let (table, more) = render_table(ctx, &l, offset, limit);
        out.push_str(&table);
        more
    };
    if more {
        out.push_str(&format!(
            "\nnext_cursor: {}\n",
            cursor("rec_offset", offset + limit)
        ));
    }
    Ok(json!({"result": out}))
}

fn can_edit(l: &Value) -> bool {
    s(l, "owner") == CALLER
        || l["co_owners"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x == CALLER))
        || l.get("editable_by_all")
            .and_then(Value::as_bool)
            .unwrap_or(true)
}

fn can_admin(l: &Value) -> bool {
    s(l, "owner") == CALLER
        || l["co_owners"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x == CALLER))
}

/// Maps names/keys to columns. Errors name the offending inputs.
fn resolve_columns<'a>(
    cols: &'a [Value],
    input: &Map<String, Value>,
) -> Result<Vec<(&'a Value, Value)>, String> {
    let mut missing = Vec::new();
    let mut ambiguous = Vec::new();
    let mut out = Vec::new();
    for (name, v) in input {
        if let Some(c) = cols.iter().find(|c| s(c, "key") == name) {
            out.push((c, v.clone()));
            continue;
        }
        let found: Vec<&Value> = cols
            .iter()
            .filter(|c| s(c, "name").eq_ignore_ascii_case(name))
            .collect();
        match found.len() {
            0 => missing.push(name.clone()),
            1 => out.push((found[0], v.clone())),
            _ => ambiguous.push(name.clone()),
        }
    }
    if !missing.is_empty() {
        return Err(err(&format!(
            "column_names_not_found: {}",
            missing.join(", ")
        )));
    }
    if !ambiguous.is_empty() {
        return Err(err(&format!(
            "ambiguous_column_names: {}",
            ambiguous.join(", ")
        )));
    }
    Ok(out)
}

fn as_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| x.to_string())
            })
            .collect(),
        Value::String(s) => s
            .split(',')
            .map(|x| x.trim().to_owned())
            .filter(|x| !x.is_empty())
            .collect(),
        Value::Null => Vec::new(),
        other => vec![other.to_string()],
    }
}

fn find_user(ctx: &Ctx, r: &str) -> Option<String> {
    let r = r
        .trim()
        .trim_start_matches("<@")
        .trim_end_matches('>')
        .trim_start_matches('@');
    let r = r.split('|').next().unwrap_or(r);
    if ctx.user(r).is_some() {
        return Some(r.to_owned());
    }
    ctx.values("users").into_iter().find_map(|u| {
        let hit = ["email", "name", "display_name", "real_name"]
            .iter()
            .any(|k| !s(&u, k).is_empty() && s(&u, k).eq_ignore_ascii_case(r));
        hit.then(|| s(&u, "id").to_owned())
    })
}

/// Converts one input value to the column's stored form.
fn convert(ctx: &Ctx, col: &Value, v: &Value) -> Result<Value, String> {
    let ty = s(col, "type");
    let single = |items: Vec<String>| -> Result<Vec<String>, String> {
        if items.len() > 1 {
            Err("only one value is allowed".into())
        } else if items.is_empty() {
            Err("a value is required".into())
        } else {
            Ok(items)
        }
    };
    match ty {
        "text" | "rich_text" | "phone" | "link" | "message" => match v {
            Value::String(x) if !x.is_empty() => Ok(json!(x)),
            Value::Number(n) => Ok(json!(n.to_string())),
            _ => Err("expected text".into()),
        },
        "email" => match v.as_str() {
            Some(x) if x.contains('@') && !x.contains(' ') => Ok(json!(x)),
            _ => Err("expected an email address".into()),
        },
        "number" | "currency" => match v {
            Value::Number(_) => Ok(v.clone()),
            Value::String(x) => x
                .trim()
                .trim_start_matches('$')
                .replace(',', "")
                .parse::<f64>()
                .map(|f| {
                    if f.fract() == 0.0 {
                        json!(f as i64)
                    } else {
                        json!(f)
                    }
                })
                .map_err(|_| "expected a number".into()),
            _ => Err("expected a number".into()),
        },
        "rating" => {
            let n = match v {
                Value::Number(n) => n.as_i64(),
                Value::String(x) => x.trim().parse().ok(),
                _ => None,
            };
            match n {
                Some(n) if (1..=5).contains(&n) => Ok(json!(n)),
                _ => Err("rating must be between 1 and 5".into()),
            }
        }
        "checkbox" | "completed" => match v {
            Value::Bool(b) => Ok(json!(b)),
            Value::String(x)
                if ["true", "yes", "done", "checked"].contains(&x.to_lowercase().as_str()) =>
            {
                Ok(json!(true))
            }
            Value::String(x)
                if ["false", "no", "unchecked", ""].contains(&x.to_lowercase().as_str()) =>
            {
                Ok(json!(false))
            }
            _ => Err("expected true or false".into()),
        },
        "date" | "due_date" => {
            let x = v.as_str().unwrap_or("");
            match tz::parse_date(x) {
                Some((y, m, d)) => Ok(json!(format!("{y:04}-{m:02}-{d:02}"))),
                None => Err("expected a date (YYYY-MM-DD)".into()),
            }
        }
        "select" | "multi_select" => {
            let items = as_list(v);
            let items = if ty == "select" {
                single(items)?
            } else {
                items
            };
            let choices = col["options"]["choices"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let mut out = Vec::new();
            for it in items {
                let found = choices
                    .iter()
                    .find(|c| s(c, "value") == it || s(c, "label").eq_ignore_ascii_case(&it))
                    .ok_or_else(|| format!("{it:?} is not one of the column's options"))?;
                out.push(json!(s(found, "value")));
            }
            Ok(Value::Array(out))
        }
        "user" | "assignee" => {
            let items = single(as_list(v)).or_else(|e| {
                if ty == "assignee" {
                    Ok(as_list(v))
                } else {
                    Err(e)
                }
            })?;
            let mut out = Vec::new();
            for it in items {
                out.push(json!(
                    find_user(ctx, &it).ok_or_else(|| format!("unknown user {it:?}"))?
                ));
            }
            Ok(Value::Array(out))
        }
        "channel" => {
            let items = single(as_list(v))?;
            let it = items[0].trim_start_matches('#').to_owned();
            let c = ctx
                .channel(&it)
                .or_else(|| {
                    ctx.values("channels")
                        .into_iter()
                        .find(|c| s(c, "name") == it)
                })
                .ok_or_else(|| format!("unknown channel {it:?}"))?;
            Ok(json!([s(&c, "id")]))
        }
        "attachment" => Ok(Value::Array(
            as_list(v).into_iter().map(Value::String).collect(),
        )),
        "vote" => Err("cannot set another user's vote".into()),
        other => Err(format!("column type {other} is read-only")),
    }
}

fn apply_values(
    ctx: &Ctx,
    cols: &[Value],
    input: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let resolved = resolve_columns(cols, input)?;
    let mut failures = Vec::new();
    let mut out = Map::new();
    for (col, v) in resolved {
        match convert(ctx, col, &v) {
            Ok(x) => {
                out.insert(s(col, "key").to_owned(), x);
            }
            Err(reason) => failures.push(format!("{}: {reason}", s(col, "name"))),
        }
    }
    if !failures.is_empty() {
        return Err(err(&format!(
            "data_not_matching_schema: {}",
            failures.join("; ")
        )));
    }
    Ok(out)
}

pub fn add_record(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let id = arg_str(args, "list_id")
        .ok_or_else(|| err("list_id_invalid"))?
        .to_owned();
    if !id.starts_with('F') {
        return Err(err("list_id_invalid"));
    }
    let mut l = ctx.get("lists", &id).ok_or_else(|| err("list_not_found"))?;
    let input = args
        .get("columns")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| err("invalid_args: `columns` must be a non-empty object"))?;
    let cols: Vec<Value> = l["columns"].as_array().cloned().unwrap_or_default();
    if cols.is_empty() {
        return Err(err("list_has_no_columns"));
    }
    if !can_edit(&l) {
        return Err(err("no_permission_to_edit_list"));
    }
    let fields = apply_values(ctx, &cols, input)?;
    let n = l
        .get("next_rec")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| l["records"].as_array().map_or(0, |a| a.len() as u64));
    let rec_id = hashed_id("Rec0", &id, n + 1000);
    l["next_rec"] = json!(n + 1);
    let record = json!({"id": rec_id, "fields": fields, "created_by": CALLER, "created": ctx.now});
    l.as_object_mut()
        .expect("list")
        .entry("records")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .expect("records")
        .push(record);
    ctx.put("lists", &id, l.clone());
    Ok(json!({
        "result": format!(
            "Record added to list \"{}\".\nRecord ID: {rec_id}\nLink: {}?record_id={rec_id}\n",
            s(&l, "name"),
            link(ctx, &id)
        )
    }))
}

pub fn update_record(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let id = arg_str(args, "list_id")
        .ok_or_else(|| err("list_id_invalid"))?
        .to_owned();
    if !id.starts_with('F') {
        return Err(err("list_id_invalid"));
    }
    let mut l = ctx.get("lists", &id).ok_or_else(|| err("list_not_found"))?;
    let rec_id = arg_str(args, "record_id")
        .ok_or_else(|| err("invalid_record_id"))?
        .to_owned();
    if !rec_id.starts_with("Rec") {
        return Err(err("invalid_record_id"));
    }
    let input = args
        .get("updated_columns")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| err("invalid_args: `updated_columns` must be a non-empty object"))?;
    let cols: Vec<Value> = l["columns"].as_array().cloned().unwrap_or_default();
    if cols.is_empty() {
        return Err(err("list_has_no_columns"));
    }
    if !can_edit(&l) {
        return Err(err("no_permission_to_edit_list"));
    }
    let pos = l["records"]
        .as_array()
        .and_then(|a| a.iter().position(|r| s(r, "id") == rec_id))
        .ok_or_else(|| err("invalid_row_id"))?;
    let fields = apply_values(ctx, &cols, input)?;
    let rec = &mut l["records"][pos];
    if !rec.get("fields").is_some_and(Value::is_object) {
        rec["fields"] = json!({});
    }
    for (k, v) in fields {
        rec["fields"][k] = v;
    }
    rec["updated"] = json!(ctx.now);
    rec["last_edited_by"] = json!(CALLER);
    ctx.put("lists", &id, l.clone());
    Ok(json!({
        "result": format!(
            "Record {rec_id} updated in list \"{}\".\nLink: {}?record_id={rec_id}\n",
            s(&l, "name"),
            link(ctx, &id)
        )
    }))
}

pub fn update(ctx: &mut Ctx, args: &Value) -> ToolResult {
    let id = arg_str(args, "list_id")
        .ok_or_else(|| err("list_id_invalid"))?
        .to_owned();
    let mut l = ctx.get("lists", &id).ok_or_else(|| err("list_not_found"))?;
    let adds = args
        .get("columns_to_add")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let updates = args
        .get("columns_to_update")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let deletes = args
        .get("columns_to_delete")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let meta = ["name", "description", "icon"]
        .iter()
        .any(|k| arg_str(args, k).is_some());
    if !meta && adds.is_empty() && updates.is_empty() && deletes.is_empty() {
        return Err(err("invalid_args: provide at least one field to update"));
    }
    if !can_edit(&l) {
        return Err(err("no_permission_to_edit_list"));
    }
    let mut lines = Vec::new();
    for k in ["name", "description", "icon"] {
        if let Some(v) = arg_str(args, k) {
            l[k] = json!(v);
            lines.push(format!("- Updated {k} to \"{v}\""));
        }
    }
    if !(adds.is_empty() && updates.is_empty() && deletes.is_empty()) && !can_admin(&l) {
        let owner = ctx
            .user(s(&l, "owner"))
            .map(|u| display_name(&u))
            .unwrap_or_default();
        return Err(err(&format!(
            "permission_denied: only the list owner ({owner}) can change columns"
        )));
    }
    let mut n = l.get("next_col").and_then(Value::as_u64).unwrap_or(100);
    let mut cols: Vec<Value> = l["columns"].as_array().cloned().unwrap_or_default();
    for a in &adds {
        if cols.len() >= 30 {
            lines.push(format!(
                "- Failed to add column \"{}\": over_column_maximum",
                s(a, "name")
            ));
            continue;
        }
        match make_column(&id, &mut n, a, UPDATE_TYPES) {
            Ok(c) => {
                lines.push(format!(
                    "- Added column \"{}\" (key: {})",
                    s(&c, "name"),
                    s(&c, "key")
                ));
                cols.push(c);
            }
            Err(e) => lines.push(format!("- Failed to add column \"{}\": {e}", s(a, "name"))),
        }
    }
    for u in &updates {
        let key = s(u, "key");
        let Some(c) = cols.iter_mut().find(|c| s(c, "key") == key) else {
            lines.push(format!("- Failed to update column {key}: column_not_found"));
            continue;
        };
        if let Some(name) = arg_str(u, "name") {
            c["name"] = json!(name);
        }
        if let Some(ty) = arg_str(u, "type") {
            if !UPDATE_TYPES.contains(&ty) {
                lines.push(format!(
                    "- Failed to update column {key}: invalid type {ty}"
                ));
                continue;
            }
            c["type"] = json!(ty);
        }
        if u.get("options").is_some() {
            match parse_choices(u.get("options")) {
                Ok(ch) => c["options"] = json!({"choices": ch}),
                Err(e) => {
                    lines.push(format!("- Failed to update column {key}: {e}"));
                    continue;
                }
            }
        }
        lines.push(format!("- Updated column {key}"));
    }
    for d in &deletes {
        let key = s(d, "key");
        match cols.iter().position(|c| s(c, "key") == key) {
            Some(0) => lines.push(format!(
                "- Failed to delete column {key}: cannot delete the primary column"
            )),
            Some(i) => {
                cols.remove(i);
                lines.push(format!("- Deleted column {key}"));
            }
            None => lines.push(format!("- Failed to delete column {key}: column_not_found")),
        }
    }
    l["columns"] = Value::Array(cols);
    l["next_col"] = json!(n);
    ctx.put("lists", &id, l.clone());
    Ok(json!({
        "result": format!(
            "Updated list \"{}\" (ID: {id})\n{}\nPermalink: {}\n",
            s(&l, "name"),
            lines.join("\n"),
            link(ctx, &id)
        )
    }))
}
