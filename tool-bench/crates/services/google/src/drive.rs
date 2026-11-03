//! Drive, Docs and Sheets tools over the `drive_files` collection.
//!
//! A file: `{id, name, mimeType, parents, owners, createdTime, modifiedTime, size, trashed,
//! starred, description, permissions, content}`. Google Docs keep their body text in
//! `content`; Sheets keep `sheets: [{sheetId, title, rowCount, columnCount, values, tables}]`.

use serde_json::{Value, json};

use crate::util::{
    Ctx, R, USER, arg, arg_bool, arg_i64, arg_json, arg_list, http_error, py_repr, s,
};

pub const DOC: &str = "application/vnd.google-apps.document";
pub const SHEET: &str = "application/vnd.google-apps.spreadsheet";
pub const SLIDES: &str = "application/vnd.google-apps.presentation";
pub const FOLDER: &str = "application/vnd.google-apps.folder";
pub const FORM: &str = "application/vnd.google-apps.form";

pub fn root_id(ctx: &Ctx) -> String {
    ctx.get("drive_settings", "settings")
        .map(|v| s(&v, "rootFolderId").to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "0AOctoCorpRootUk9PVA".to_owned())
}

pub fn link(f: &Value) -> String {
    if let Some(l) = f.get("webViewLink").and_then(Value::as_str) {
        return l.to_owned();
    }
    let id = s(f, "id");
    match s(f, "mimeType") {
        DOC => format!("https://docs.google.com/document/d/{id}/edit?usp=drivesdk"),
        SHEET => format!("https://docs.google.com/spreadsheets/d/{id}/edit?usp=drivesdk"),
        SLIDES => format!("https://docs.google.com/presentation/d/{id}/edit?usp=drivesdk"),
        FORM => format!("https://docs.google.com/forms/d/{id}/edit?usp=drivesdk"),
        FOLDER => format!("https://drive.google.com/drive/folders/{id}"),
        _ => format!("https://drive.google.com/file/d/{id}/view?usp=drivesdk"),
    }
}

fn file_url(id: &str) -> String {
    format!(
        "https://www.googleapis.com/drive/v3/files/{id}?fields=id%2C+name%2C+mimeType%2C+webViewLink%2C+size&supportsAllDrives=true&alt=json"
    )
}

pub fn need_file(ctx: &Ctx, tool: &str, id: &str) -> Result<Value, String> {
    ctx.get("drive_files", id).ok_or_else(|| {
        format!(
            "API error in {tool}: {}",
            http_error(
                404,
                &file_url(id),
                &format!("File not found: {id}."),
                "notFound"
            )
        )
    })
}

fn touch(ctx: &Ctx, f: &mut Value) {
    f["modifiedTime"] = json!(ctx.rfc3339_ms());
    f["lastModifyingUser"] = json!({"displayName": "Ops Team", "emailAddress": USER});
}

pub fn new_file(ctx: &mut Ctx, name: &str, mime: &str, parent: Option<&str>) -> Value {
    let id = ctx.drive_id();
    let now = ctx.rfc3339_ms();
    let parent = match parent {
        Some(p) if p != "root" => p.to_owned(),
        _ => root_id(ctx),
    };
    let mut f = json!({
        "id": id, "name": name, "mimeType": mime, "parents": [parent], "createdTime": now, "modifiedTime": now,
        "owners": [{"displayName": "Ops Team", "emailAddress": USER}], "trashed": false, "starred": false,
        "permissions": [{"id": "16118283490155935341", "type": "user", "role": "owner", "emailAddress": USER}],
        "shared": false,
    });
    f["webViewLink"] = json!(link(&f));
    f
}

// ───────────────────────────── query language ─────────────────────────────

fn qtokens(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = q.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '\'' || c == '"' {
            let quote = c;
            chars.next();
            let mut t = String::from("'");
            while let Some(n) = chars.next() {
                if n == '\\' {
                    if let Some(x) = chars.next() {
                        t.push(x);
                    }
                } else if n == quote {
                    break;
                } else {
                    t.push(n);
                }
            }
            out.push(t);
        } else if c == '(' || c == ')' {
            out.push(c.to_string());
            chars.next();
        } else if "=!<>".contains(c) {
            let mut t = String::new();
            while let Some(&n) = chars.peek() {
                if "=!<>".contains(n) {
                    t.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            out.push(t);
        } else {
            let mut t = String::new();
            while let Some(&n) = chars.peek() {
                if n.is_whitespace() || "()=!<>'\"".contains(n) {
                    break;
                }
                t.push(n);
                chars.next();
            }
            out.push(t);
        }
    }
    out
}

enum DQ {
    And(Box<DQ>, Box<DQ>),
    Or(Box<DQ>, Box<DQ>),
    Not(Box<DQ>),
    Clause(String, String, String),
    True,
}

struct P {
    t: Vec<String>,
    i: usize,
}

impl P {
    fn peek(&self) -> Option<&str> {
        self.t.get(self.i).map(String::as_str)
    }
    fn next(&mut self) -> Option<String> {
        let v = self.t.get(self.i).cloned();
        self.i += 1;
        v
    }
    fn or(&mut self) -> DQ {
        let mut l = self.and();
        while self.peek().is_some_and(|x| x.eq_ignore_ascii_case("or")) {
            self.next();
            let r = self.and();
            l = DQ::Or(Box::new(l), Box::new(r));
        }
        l
    }
    fn and(&mut self) -> DQ {
        let mut l = self.not();
        while self.peek().is_some_and(|x| x.eq_ignore_ascii_case("and")) {
            self.next();
            let r = self.not();
            l = DQ::And(Box::new(l), Box::new(r));
        }
        l
    }
    fn not(&mut self) -> DQ {
        if self.peek().is_some_and(|x| x.eq_ignore_ascii_case("not")) {
            self.next();
            return DQ::Not(Box::new(self.not()));
        }
        if self.peek() == Some("(") {
            self.next();
            let e = self.or();
            if self.peek() == Some(")") {
                self.next();
            }
            return e;
        }
        let a = self.next().unwrap_or_default();
        let op = self.next().unwrap_or_default();
        if op.eq_ignore_ascii_case("in") {
            let field = self.next().unwrap_or_default();
            return DQ::Clause(
                field.to_lowercase(),
                "in".into(),
                a.trim_start_matches('\'').to_owned(),
            );
        }
        if a.eq_ignore_ascii_case("sharedwithme") {
            self.i -= 1;
            return DQ::Clause("sharedwithme".into(), "=".into(), "true".into());
        }
        let v = self.next().unwrap_or_default();
        if a.is_empty() {
            return DQ::True;
        }
        DQ::Clause(
            a.to_lowercase(),
            op.to_lowercase(),
            v.trim_start_matches('\'').to_owned(),
        )
    }
}

fn file_text(f: &Value) -> String {
    let mut t = format!(
        "{} {} {}",
        s(f, "name"),
        s(f, "description"),
        s(f, "content")
    );
    if let Some(sheets) = f["sheets"].as_array() {
        for sh in sheets {
            for row in sh["values"].as_array().into_iter().flatten() {
                for c in row.as_array().into_iter().flatten() {
                    t.push(' ');
                    t.push_str(c.as_str().unwrap_or(""));
                }
            }
        }
    }
    t
}

fn eval_q(ctx: &Ctx, q: &DQ, f: &Value) -> bool {
    match q {
        DQ::True => true,
        DQ::And(a, b) => eval_q(ctx, a, f) && eval_q(ctx, b, f),
        DQ::Or(a, b) => eval_q(ctx, a, f) || eval_q(ctx, b, f),
        DQ::Not(a) => !eval_q(ctx, a, f),
        DQ::Clause(field, op, v) => {
            let cmp = |actual: &str| -> bool {
                match op.as_str() {
                    "=" => actual == v,
                    "!=" => actual != v,
                    "contains" => actual.to_lowercase().contains(&v.to_lowercase()),
                    ">" => actual > v.as_str(),
                    "<" => actual < v.as_str(),
                    ">=" => actual >= v.as_str(),
                    "<=" => actual <= v.as_str(),
                    _ => false,
                }
            };
            match field.as_str() {
                "name" | "title" => {
                    if op == "contains" {
                        // Drive matches name prefixes of words; a substring match is close enough.
                        s(f, "name").to_lowercase().contains(&v.to_lowercase())
                    } else {
                        cmp(s(f, "name"))
                    }
                }
                // Full-text search matches every word, in any order.
                "fulltext" => {
                    let hay = file_text(f).to_lowercase();
                    v.to_lowercase().split_whitespace().all(|w| hay.contains(w))
                }
                "mimetype" => cmp(s(f, "mimeType")),
                "parents" => {
                    let target = if v == "root" { root_id(ctx) } else { v.clone() };
                    f["parents"]
                        .as_array()
                        .is_some_and(|p| p.iter().any(|x| x == target.as_str()))
                }
                "owners" => f["owners"]
                    .as_array()
                    .is_some_and(|o| o.iter().any(|x| s(x, "emailAddress") == v)),
                "writers" | "readers" => f["permissions"].as_array().is_some_and(|o| {
                    o.iter().any(|x| {
                        s(x, "emailAddress") == v
                            && (field == "readers"
                                || matches!(
                                    s(x, "role"),
                                    "writer" | "owner" | "organizer" | "fileOrganizer"
                                ))
                    })
                }),
                "trashed" => {
                    (f["trashed"].as_bool().unwrap_or(false) == (v == "true")) == (op == "=")
                }
                "starred" => {
                    (f["starred"].as_bool().unwrap_or(false) == (v == "true")) == (op == "=")
                }
                "modifiedtime" => cmp(s(f, "modifiedTime")),
                "createdtime" => cmp(s(f, "createdTime")),
                "viewedbymetime" => cmp(s(f, "viewedByMeTime")),
                "sharedwithme" => !f["owners"]
                    .as_array()
                    .is_some_and(|o| o.iter().any(|x| s(x, "emailAddress") == USER)),
                _ => false,
            }
        }
    }
}

fn is_structured(q: &str) -> bool {
    let l = q.to_lowercase();
    [
        " contains ",
        " in parents",
        " in owners",
        "mimetype",
        "trashed",
        "starred",
        "modifiedtime",
        "createdtime",
        "sharedwithme",
        "fulltext",
        "name =",
        "name=",
        "name !=",
    ]
    .iter()
    .any(|p| l.contains(p))
}

fn resolve_type(t: &str) -> String {
    match t.to_lowercase().as_str() {
        "folder" => FOLDER.into(),
        "document" | "doc" | "docs" => DOC.into(),
        "spreadsheet" | "sheet" | "sheets" => SHEET.into(),
        "presentation" | "slides" | "slide" => SLIDES.into(),
        "form" => FORM.into(),
        "pdf" => "application/pdf".into(),
        "image" => "image/".into(),
        other => other.to_owned(),
    }
}

pub fn query_files(
    ctx: &Ctx,
    query: &str,
    include_trashed: bool,
    file_type: Option<&str>,
) -> Vec<Value> {
    let q = if is_structured(query) {
        let mut p = P {
            t: qtokens(query),
            i: 0,
        };
        p.or()
    } else {
        DQ::Clause("fulltext".into(), "contains".into(), query.to_owned())
    };
    let explicit_trash = query.to_lowercase().contains("trashed");
    let mime = file_type.map(resolve_type);
    let mut out: Vec<Value> = ctx
        .all("drive_files")
        .into_iter()
        .filter(|f| include_trashed || explicit_trash || !f["trashed"].as_bool().unwrap_or(false))
        .filter(|f| {
            mime.as_ref().is_none_or(|m| {
                if m.ends_with('/') {
                    s(f, "mimeType").starts_with(m.as_str())
                } else {
                    s(f, "mimeType") == m
                }
            })
        })
        .filter(|f| eval_q(ctx, &q, f))
        .collect();
    out.sort_by(|a, b| s(b, "modifiedTime").cmp(s(a, "modifiedTime")));
    out
}

fn order(mut v: Vec<Value>, order_by: Option<&str>) -> Vec<Value> {
    match order_by.map(str::to_lowercase).as_deref() {
        Some(o) if o.starts_with("name") => {
            v.sort_by_key(|a| s(a, "name").to_lowercase());
            if o.contains("desc") {
                v.reverse();
            }
        }
        Some(o) if o.starts_with("createdtime") => {
            v.sort_by(|a, b| s(a, "createdTime").cmp(s(b, "createdTime")));
            if o.contains("desc") {
                v.reverse();
            }
        }
        Some(o) if o.starts_with("modifiedtime") && !o.contains("desc") => {
            v.sort_by(|a, b| s(a, "modifiedTime").cmp(s(b, "modifiedTime")))
        }
        _ => {}
    }
    v
}

fn item_line(f: &Value, detailed: bool, show_anyone: bool) -> String {
    if !detailed {
        return format!(
            "- Name: \"{}\" (ID: {}, Type: {})",
            s(f, "name"),
            s(f, "id"),
            s(f, "mimeType")
        );
    }
    let size = if f.get("size").is_some() {
        format!(
            ", Size: {}",
            f["size"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| f["size"].to_string())
        )
    } else {
        String::new()
    };
    let created = if s(f, "createdTime").is_empty() {
        String::new()
    } else {
        format!(", Created: {}", s(f, "createdTime"))
    };
    let lmu = &f["lastModifyingUser"];
    let edited = match (s(lmu, "displayName"), s(lmu, "emailAddress")) {
        ("", "") => String::new(),
        (n, "") => format!(", Last Edited By: {n}"),
        ("", e) => format!(", Last Edited By: {e}"),
        (n, e) => format!(", Last Edited By: {n} <{e}>"),
    };
    let anyone = if show_anyone {
        f["permissions"]
            .as_array()
            .and_then(|p| p.iter().find(|x| s(x, "type") == "anyone"))
            .map(|p| format!(", Anyone with link: {}", s(p, "role")))
            .unwrap_or_default()
    } else {
        String::new()
    };
    format!(
        "- Name: \"{}\" (ID: {}, Type: {}{size}{created}, Modified: {}{edited}{anyone}) Link: {}",
        s(f, "name"),
        s(f, "id"),
        s(f, "mimeType"),
        if s(f, "modifiedTime").is_empty() {
            "N/A"
        } else {
            s(f, "modifiedTime")
        },
        link(f)
    )
}

fn paginate(items: Vec<Value>, a: &Value, default: i64) -> (Vec<Value>, Option<String>) {
    let size = arg_i64(a, "page_size").unwrap_or(default).max(1) as usize;
    let offset: usize = arg(a, "page_token")
        .and_then(|t| t.trim_start_matches("~!!~").parse().ok())
        .unwrap_or(0);
    let next = (items.len() > offset + size).then(|| format!("~!!~{}", offset + size));
    (items.into_iter().skip(offset).take(size).collect(), next)
}

pub fn search_files(ctx: &mut Ctx, a: &Value) -> R {
    let query = s(a, "query").to_owned();
    let all = order(
        query_files(
            ctx,
            &query,
            arg_bool(a, "include_trashed").unwrap_or(false),
            arg(a, "file_type"),
        ),
        arg(a, "order_by"),
    );
    let (page, next) = paginate(all, a, 10);
    if page.is_empty() && next.is_none() {
        return Ok(format!("No files found for '{query}'."));
    }
    let detailed = arg_bool(a, "detailed").unwrap_or(true);
    let mut out = vec![format!(
        "Found {} files for {USER} matching '{query}':",
        page.len()
    )];
    out.extend(page.iter().map(|f| item_line(f, detailed, true)));
    if let Some(n) = next {
        out.push(format!("nextPageToken: {n}"));
    }
    Ok(out.join("\n"))
}

pub fn list_items(ctx: &mut Ctx, a: &Value) -> R {
    let folder = arg(a, "folder_id").unwrap_or("root").to_owned();
    if arg(a, "resource_type").is_some_and(|r| r.eq_ignore_ascii_case("shared_drives")) {
        let drives = ctx.all("shared_drives");
        if drives.is_empty() {
            return Ok(format!("No shared drives found for {USER}."));
        }
        let mut out = vec![format!("Found {} shared drives for {USER}:", drives.len())];
        out.extend(
            drives
                .iter()
                .map(|d| format!("- Name: \"{}\" (ID: {})", s(d, "name"), s(d, "id"))),
        );
        return Ok(out.join("\n"));
    }
    if arg(a, "resource_type").is_some_and(|r| !r.eq_ignore_ascii_case("items")) {
        return Err("resource_type must be either 'items' or 'shared_drives'".into());
    }
    let target = if folder == "root" {
        root_id(ctx)
    } else {
        folder.clone()
    };
    if target != root_id(ctx) && ctx.get("drive_files", &target).is_none() {
        return Err(format!(
            "API error in list_drive_items: {}",
            http_error(
                404,
                &file_url(&target),
                &format!("File not found: {target}."),
                "notFound"
            )
        ));
    }
    let mime = arg(a, "file_type").map(resolve_type);
    let mut items: Vec<Value> = ctx
        .all("drive_files")
        .into_iter()
        .filter(|f| !f["trashed"].as_bool().unwrap_or(false))
        .filter(|f| {
            f["parents"]
                .as_array()
                .is_some_and(|p| p.iter().any(|x| x == target.as_str()))
        })
        .filter(|f| mime.as_ref().is_none_or(|m| s(f, "mimeType") == m))
        .collect();
    if let Some(q) = arg(a, "query") {
        let ql = q.to_lowercase();
        items.retain(|f| s(f, "name").to_lowercase().contains(&ql));
    }
    items.sort_by(|x, y| {
        (s(x, "mimeType") != FOLDER)
            .cmp(&(s(y, "mimeType") != FOLDER))
            .then(
                s(x, "name")
                    .to_lowercase()
                    .cmp(&s(y, "name").to_lowercase()),
            )
    });
    let items = order(items, arg(a, "order_by"));
    let (page, next) = paginate(items, a, 100);
    if page.is_empty() && next.is_none() {
        return Ok(format!("No items found in folder '{folder}'."));
    }
    let detailed = arg_bool(a, "detailed").unwrap_or(true);
    let mut out = vec![format!(
        "Found {} items in folder '{folder}' for {USER}:",
        page.len()
    )];
    out.extend(page.iter().map(|f| item_line(f, detailed, false)));
    if let Some(n) = next {
        out.push(format!("nextPageToken: {n}"));
    }
    Ok(out.join("\n"))
}

/// Text export of a file (Docs → text, Sheets → CSV of the first sheet).
pub fn export_text(f: &Value) -> String {
    match s(f, "mimeType") {
        SHEET => f["sheets"][0]["values"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|r| {
                        r.as_array()
                            .map(|c| {
                                c.iter()
                                    .map(|x| csv(x.as_str().unwrap_or("")))
                                    .collect::<Vec<_>>()
                                    .join(",")
                            })
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join("\r\n")
            })
            .unwrap_or_default(),
        SLIDES => f["slides"]
            .as_array()
            .map(|sl| {
                sl.iter()
                    .map(|x| {
                        x["elements"]
                            .as_array()
                            .map(|e| {
                                e.iter()
                                    .map(|el| s(el, "text"))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_default(),
        _ => s(f, "content").to_owned(),
    }
}

fn csv(v: &str) -> String {
    if v.contains([',', '"', '\n']) {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_owned()
    }
}

pub fn get_content(ctx: &mut Ctx, a: &Value) -> R {
    let f = need_file(ctx, "get_drive_file_content", s(a, "file_id"))?;
    let body = match s(&f, "mimeType") {
        FOLDER => {
            return Err(format!(
                "API error in get_drive_file_content: {}",
                http_error(
                    403,
                    &format!(
                        "https://www.googleapis.com/drive/v3/files/{}?alt=media",
                        s(&f, "id")
                    ),
                    "Only files with binary content can be downloaded. Use Export with Docs Editors files.",
                    "fileNotDownloadable"
                )
            ));
        }
        m if m.starts_with("image/") || m == "application/pdf" && f.get("content").is_none() => {
            format!(
                "[Binary or unsupported text encoding for mimeType '{m}' - {} bytes]",
                f["size"]
                    .as_str()
                    .and_then(|x| x.parse::<i64>().ok())
                    .or(f["size"].as_i64())
                    .unwrap_or(0)
            )
        }
        _ => export_text(&f),
    };
    Ok(format!(
        "File: \"{}\" (ID: {}, Type: {})\nLink: {}\n\n--- CONTENT ---\n{body}",
        s(&f, "name"),
        s(&f, "id"),
        s(&f, "mimeType"),
        link(&f)
    ))
}

pub fn download_url(ctx: &mut Ctx, a: &Value) -> R {
    let f = need_file(ctx, "get_drive_file_download_url", s(a, "file_id"))?;
    let (fmt, ext) = match (s(&f, "mimeType"), arg(a, "export_format")) {
        (DOC, Some("docx")) => (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "docx",
        ),
        (DOC, Some("txt")) => ("text/plain", "txt"),
        (DOC, _) => ("application/pdf", "pdf"),
        (SHEET, Some("csv")) => ("text/csv", "csv"),
        (SHEET, Some("pdf")) => ("application/pdf", "pdf"),
        (SHEET, _) => (
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "xlsx",
        ),
        (SLIDES, Some("pptx")) => (
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "pptx",
        ),
        (SLIDES, _) => ("application/pdf", "pdf"),
        (m, _) => (m, ""),
    };
    let saved = format!(
        "{}_{}{}",
        ctx.hex_id("download", 12),
        s(&f, "name"),
        if ext.is_empty() {
            String::new()
        } else {
            format!(".{ext}")
        }
    );
    Ok(format!(
        "File downloaded successfully!\nFile: {}\nFile ID: {}\nOutput MIME type: {fmt}\nSize: {:.1} KB\n\n📎 Saved to: /home/app/.workspace-mcp/attachments/{saved}\n\nThe file has been saved to disk and can be accessed directly via the file path.",
        s(&f, "name"),
        s(&f, "id"),
        export_text(&f).len() as f64 / 1024.0
    ))
}

pub fn create_folder(ctx: &mut Ctx, a: &Value) -> R {
    let name = s(a, "folder_name").to_owned();
    let parent = arg(a, "parent_folder_id").unwrap_or("root").to_owned();
    if parent != "root" && ctx.get("drive_files", &parent).is_none() {
        return Err(format!(
            "API error in create_drive_folder: {}",
            http_error(
                404,
                &file_url(&parent),
                &format!("File not found: {parent}."),
                "notFound"
            )
        ));
    }
    let f = new_file(ctx, &name, FOLDER, Some(&parent));
    ctx.put("drive_files", f.clone());
    Ok(format!(
        "Successfully created folder '{name}' (ID: {}) in folder '{parent}' for {USER}. Link: {}",
        s(&f, "id"),
        link(&f)
    ))
}

pub fn create_file(ctx: &mut Ctx, a: &Value) -> R {
    let name = s(a, "file_name").to_owned();
    let folder = arg(a, "folder_id").unwrap_or("root").to_owned();
    let mime = arg(a, "mime_type").unwrap_or("text/plain").to_owned();
    if a.get("content").is_none()
        && arg(a, "fileUrl").is_none()
        && arg(a, "base64_content").is_none()
        && mime != FOLDER
    {
        return Err("You must provide either 'content', 'base64_content', or 'fileUrl'.".into());
    }
    if folder != "root" && ctx.get("drive_files", &folder).is_none() {
        return Err(format!(
            "API error in create_drive_file: {}",
            http_error(
                404,
                &file_url(&folder),
                &format!("File not found: {folder}."),
                "notFound"
            )
        ));
    }
    if mime == FOLDER {
        let f = new_file(ctx, &name, FOLDER, Some(&folder));
        ctx.put("drive_files", f.clone());
        return Ok(format!(
            "Successfully created folder '{name}' (ID: {}) in folder '{folder}' for {USER}. Link: {}",
            s(&f, "id"),
            link(&f)
        ));
    }
    let mut f = new_file(ctx, &name, &mime, Some(&folder));
    let content = a
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    f["size"] = json!(content.len().to_string());
    f["content"] = json!(content);
    ctx.put("drive_files", f.clone());
    Ok(format!(
        "Successfully created file '{name}' (ID: {}) in folder '{folder}' for {USER}. Link: {}",
        s(&f, "id"),
        link(&f)
    ))
}

pub fn import_as(ctx: &mut Ctx, a: &Value, mime: &str, label: &str) -> R {
    let name = s(a, "file_name").to_owned();
    let content = a.get("content").and_then(Value::as_str).map(str::to_owned);
    if content.is_none()
        && arg(a, "file_path").is_none()
        && arg(a, "file_url").is_none()
        && arg(a, "base64_content").is_none()
    {
        return Err(
            "Provide exactly one of 'content', 'file_path', 'file_url', or 'base64_content'."
                .into(),
        );
    }
    if arg(a, "file_path").is_some() || arg(a, "file_url").is_some() {
        return Err("The file could not be read: local file access and URL fetches are disabled on this server.".into());
    }
    let folder = arg(a, "folder_id").unwrap_or("root").to_owned();
    let mut f = new_file(ctx, &name, mime, Some(&folder));
    let text = content.unwrap_or_default();
    match mime {
        SHEET => {
            let rows: Vec<Value> = text
                .lines()
                .map(|l| {
                    json!(
                        l.split(',')
                            .map(|c| c.trim_matches('"').to_owned())
                            .collect::<Vec<_>>()
                    )
                })
                .collect();
            f["sheets"] = json!([{"sheetId": 0, "title": "Sheet1", "rowCount": 1000, "columnCount": 26, "values": rows}]);
        }
        SLIDES => {
            f["slides"] =
                json!([{"objectId": "p1", "elements": [{"objectId": "i0", "text": text}]}])
        }
        _ => f["content"] = json!(ensure_nl(&text)),
    }
    ctx.put("drive_files", f.clone());
    Ok(format!(
        "Successfully imported '{name}' as a Google {label} (ID: {}) in folder '{folder}' for {USER}. Link: {}",
        s(&f, "id"),
        link(&f)
    ))
}

fn perm_info(p: &Value) -> String {
    let role = s(p, "role");
    let id = s(p, "id");
    let base = match s(p, "type") {
        "anyone" => format!("Anyone with the link ({role}) [id: {id}]"),
        "user" => format!("User: {} ({role}) [id: {id}]", s(p, "emailAddress")),
        "group" => format!("Group: {} ({role}) [id: {id}]", s(p, "emailAddress")),
        "domain" => format!("Domain: {} ({role}) [id: {id}]", s(p, "domain")),
        other => format!("{other} ({role}) [id: {id}]"),
    };
    match p.get("expirationTime").and_then(Value::as_str) {
        Some(e) => format!("{base} | expires: {e}"),
        None => base,
    }
}

pub fn permissions(ctx: &mut Ctx, a: &Value) -> R {
    let f = match need_file(ctx, "get_drive_file_permissions", s(a, "file_id")) {
        Ok(f) => f,
        Err(e) => {
            return Ok(format!(
                "Error getting file permissions: {}",
                e.trim_start_matches("API error in get_drive_file_permissions: ")
            ));
        }
    };
    let perms: Vec<Value> = f["permissions"].as_array().cloned().unwrap_or_default();
    let parents = f["parents"]
        .as_array()
        .map(|p| {
            p.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|x| !x.is_empty())
        .unwrap_or_else(|| "None (root or orphaned)".into());
    let owners = f["owners"]
        .as_array()
        .map(|o| {
            o.iter()
                .map(|x| format!("{} ({})", s(x, "displayName"), s(x, "emailAddress")))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|x| !x.is_empty())
        .unwrap_or_else(|| "None available".into());
    let shared = perms.iter().any(|p| s(p, "role") != "owner");
    let mut out = vec![
        format!("File: {}", s(&f, "name")),
        format!("ID: {}", s(&f, "id")),
        format!("Type: {}", s(&f, "mimeType")),
        format!("Parents: {parents}"),
        format!("Owners: {owners}"),
        format!("Size: {} bytes", f["size"].as_str().unwrap_or("N/A")),
        format!("Created: {}", s(&f, "createdTime")),
        format!("Modified: {}", s(&f, "modifiedTime")),
        format!(
            "Trashed: {}",
            if f["trashed"].as_bool().unwrap_or(false) {
                "True"
            } else {
                "False"
            }
        ),
        String::new(),
        "Sharing Status:".into(),
        format!("  Shared: {}", if shared { "True" } else { "False" }),
    ];
    if perms.is_empty() {
        out.push("  No additional permissions (private file)".into());
    } else {
        out.push(format!("  Number of permissions: {}", perms.len()));
        out.push("  Permissions:".into());
        out.extend(perms.iter().map(|p| format!("    - {}", perm_info(p))));
    }
    out.extend([
        String::new(),
        "URLs:".into(),
        format!("  View Link: {}", link(&f)),
    ]);
    if !matches!(s(&f, "mimeType"), DOC | SHEET | SLIDES | FOLDER | FORM) {
        out.push(format!(
            "  Direct Download Link: https://drive.google.com/uc?id={}&export=download",
            s(&f, "id")
        ));
    }
    if perms.iter().any(|p| {
        s(p, "type") == "anyone" && matches!(s(p, "role"), "reader" | "writer" | "commenter")
    }) {
        out.extend([String::new(), "✅ This file is shared with 'Anyone with the link' - it can be inserted into Google Docs".into()]);
    } else {
        out.extend([
            String::new(),
            "❌ This file is NOT shared with 'Anyone with the link' - it cannot be inserted into Google Docs".into(),
            "   To fix: Right-click the file in Google Drive → Share → Anyone with the link → Viewer".into(),
        ]);
    }
    Ok(out.join("\n"))
}

pub fn public_access(ctx: &mut Ctx, a: &Value) -> R {
    let name = s(a, "file_name").to_owned();
    let matches: Vec<Value> = ctx
        .all("drive_files")
        .into_iter()
        .filter(|f| s(f, "name") == name && !f["trashed"].as_bool().unwrap_or(false))
        .collect();
    let Some(f) = matches.first() else {
        return Ok(format!("No file found with name '{name}'"));
    };
    let public = f["permissions"]
        .as_array()
        .is_some_and(|p| p.iter().any(|x| s(x, "type") == "anyone"));
    let mut out = vec![format!("Search results for '{name}':")];
    if matches.len() > 1 {
        out.push(format!(
            "Found {} files with this name. Checking the most recent one...",
            matches.len()
        ));
    }
    out.extend([
        format!("File: {}", s(f, "name")),
        format!("ID: {}", s(f, "id")),
        format!("Type: {}", s(f, "mimeType")),
        format!("Modified: {}", s(f, "modifiedTime")),
        format!("Public Access: {}", if public { "✅ Yes" } else { "❌ No" }),
    ]);
    if public {
        out.push(format!(
            "Direct URL: https://drive.google.com/uc?export=view&id={}",
            s(f, "id")
        ));
    } else {
        out.push("⚠️  This file is not publicly accessible. To make it public:".into());
        out.push(
            "   Right-click the file in Google Drive → Share → Anyone with the link → Viewer"
                .into(),
        );
    }
    Ok(out.join("\n"))
}

pub fn update_file(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "file_id").to_owned();
    let mut f = need_file(ctx, "update_drive_file", &id)?;
    let old = f.clone();
    let mut changes = Vec::new();
    if let Some(n) = arg(a, "name") {
        changes.push(format!("   • Name: '{}' → '{n}'", s(&old, "name")));
        f["name"] = json!(n);
    }
    if let Some(d) = a.get("description").and_then(Value::as_str) {
        let old_d = if s(&old, "description").is_empty() {
            "(empty)".to_owned()
        } else {
            format!("'{}'", s(&old, "description"))
        };
        changes.push(format!("   • Description: {old_d} → '{d}'"));
        f["description"] = json!(d);
    }
    if let Some(p) = arg(a, "add_parents") {
        let mut parents: Vec<String> = f["parents"]
            .as_array()
            .map(|x| {
                x.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        for x in p.split(',').map(str::trim) {
            if x != "root" && ctx.get("drive_files", x).is_none() {
                return Err(format!(
                    "API error in update_drive_file: {}",
                    http_error(
                        404,
                        &file_url(x),
                        &format!("File not found: {x}."),
                        "notFound"
                    )
                ));
            }
            let x = if x == "root" {
                root_id(ctx)
            } else {
                x.to_owned()
            };
            if !parents.contains(&x) {
                parents.push(x);
            }
        }
        f["parents"] = json!(parents);
        changes.push(format!("   • Added to folder(s): {p}"));
    }
    if let Some(p) = arg(a, "remove_parents") {
        let rm: Vec<String> = p
            .split(',')
            .map(|x| {
                if x.trim() == "root" {
                    root_id(ctx)
                } else {
                    x.trim().to_owned()
                }
            })
            .collect();
        let parents: Vec<Value> = f["parents"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|x| !rm.iter().any(|r| x == r.as_str()))
            .collect();
        f["parents"] = json!(parents);
        changes.push(format!("   • Removed from folder(s): {p}"));
    }
    if let Some(st) = arg_bool(a, "starred") {
        f["starred"] = json!(st);
        changes.push(format!(
            "   • File {}",
            if st { "starred" } else { "unstarred" }
        ));
    }
    if let Some(t) = arg_bool(a, "trashed") {
        f["trashed"] = json!(t);
        changes.push(format!(
            "   • File {}",
            if t {
                "moved to trash"
            } else {
                "restored from trash"
            }
        ));
    }
    if let Some(w) = arg_bool(a, "writers_can_share") {
        f["writersCanShare"] = json!(w);
        changes.push(format!(
            "   • Writers {} share the file",
            if w { "can" } else { "cannot" }
        ));
    }
    if let Some(c) = arg_bool(a, "copy_requires_writer_permission") {
        f["copyRequiresWriterPermission"] = json!(c);
        changes.push(format!(
            "   • Copying {} writer permission",
            if c { "requires" } else { "doesn't require" }
        ));
    }
    if let Some(p) = arg_json(a, "properties") {
        f["properties"] = p.clone();
        changes.push(format!("   • Updated custom properties: {}", py_repr(&p)));
    }
    if let Some(c) = a.get("content").and_then(Value::as_str) {
        let mode = arg(a, "mode").unwrap_or("replace");
        let mime = s(&f, "mimeType").to_owned();
        if matches!(mime.as_str(), DOC | SHEET | SLIDES) && mode != "replace" {
            return Err(format!(
                "mode='{mode}' is only supported for non-Google files, because a native {mime} would have to be exported and re-imported, losing formatting."
            ));
        }
        let existing = s(&f, "content").to_owned();
        let new = match mode {
            "append" => format!("{existing}{c}"),
            "prepend" => format!("{c}{existing}"),
            _ => c.to_owned(),
        };
        f["content"] = json!(if mime == DOC {
            ensure_nl(&new)
        } else {
            new.clone()
        });
        f["size"] = json!(new.len().to_string());
        let detail = if mode == "replace" {
            "Replaced content from inline content".to_owned()
        } else {
            format!(
                "{} text",
                if mode == "append" {
                    "Appended"
                } else {
                    "Prepended"
                }
            )
        };
        changes.push(format!("   • {detail} (written as {mime})"));
    }
    touch(ctx, &mut f);
    ctx.put("drive_files", f.clone());
    let mut out = vec![
        format!("✅ Successfully updated file: {}", s(&f, "name")),
        format!("   File ID: {id}"),
    ];
    if !changes.is_empty() {
        out.push(String::new());
        out.push("Changes applied:".into());
        out.extend(changes);
    }
    out.push(String::new());
    out.push(format!("View file: {}", link(&f)));
    Ok(out.join("\n"))
}

pub fn shareable_link(ctx: &mut Ctx, a: &Value) -> R {
    let f = need_file(ctx, "get_drive_shareable_link", s(a, "file_id"))?;
    let perms: Vec<Value> = f["permissions"].as_array().cloned().unwrap_or_default();
    let mut out = vec![
        format!("File: {}", s(&f, "name")),
        format!("ID: {}", s(&f, "id")),
        format!("Type: {}", s(&f, "mimeType")),
        format!(
            "Shared: {}",
            if perms.iter().any(|p| s(p, "role") != "owner") {
                "True"
            } else {
                "False"
            }
        ),
        String::new(),
        "Links:".into(),
        format!("  View: {}", link(&f)),
    ];
    if !matches!(s(&f, "mimeType"), DOC | SHEET | SLIDES | FOLDER | FORM) {
        out.push(format!(
            "  Download: https://drive.google.com/uc?id={}&export=download",
            s(&f, "id")
        ));
    }
    if !perms.is_empty() {
        out.push(String::new());
        out.push("Current permissions:".into());
        out.extend(perms.iter().map(|p| format!("  - {}", perm_info(p))));
    }
    Ok(out.join("\n"))
}

fn new_perm(ctx: &mut Ctx, ty: &str, role: &str, who: Option<&str>) -> Value {
    let n = ctx.st.ids.next_int("permission", 7_330_000_000_000_000_000);
    let id = if ty == "anyone" {
        "anyoneWithLink".to_owned()
    } else {
        n.to_string()
    };
    let mut p = json!({"id": id, "type": ty, "role": role});
    match (ty, who) {
        ("domain", Some(d)) => p["domain"] = json!(d),
        (_, Some(e)) => p["emailAddress"] = json!(e),
        _ => {}
    }
    p
}

const ROLES: [&str; 6] = [
    "reader",
    "commenter",
    "writer",
    "fileOrganizer",
    "organizer",
    "owner",
];

pub fn manage_access(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "file_id").to_owned();
    let action = s(a, "action").to_owned();
    if !["grant", "grant_batch", "update", "revoke", "transfer_owner"].contains(&action.as_str()) {
        return Err(format!(
            "Invalid action '{action}'. Must be one of: grant, grant_batch, update, revoke, transfer_owner"
        ));
    }
    let mut f = need_file(ctx, "manage_drive_access", &id)?;
    let mut perms: Vec<Value> = f["permissions"].as_array().cloned().unwrap_or_default();
    let name = s(&f, "name").to_owned();
    let out = match action.as_str() {
        "grant" => {
            let ty = arg(a, "share_type").unwrap_or("user").to_owned();
            let role = arg(a, "role").unwrap_or("reader").to_owned();
            if !ROLES.contains(&role.as_str()) {
                return Err(format!(
                    "Invalid role '{role}'. Must be one of: reader, commenter, writer"
                ));
            }
            let who = arg(a, "share_with").map(str::to_owned);
            if ty != "anyone" && who.is_none() {
                return Err(format!("share_with is required for share_type '{ty}'"));
            }
            let p = new_perm(ctx, &ty, &role, who.as_deref());
            perms.push(p.clone());
            vec![
                format!("Successfully shared '{name}'"),
                String::new(),
                "Permission created:".into(),
                format!("  - {}", perm_info(&p)),
                String::new(),
                format!("View link: {}", link(&f)),
            ]
        }
        "grant_batch" => {
            let recipients = a
                .get("recipients")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if recipients.is_empty() {
                return Err("recipients is required for grant_batch action".into());
            }
            let mut results = Vec::new();
            for r in &recipients {
                let ty = r
                    .get("share_type")
                    .and_then(Value::as_str)
                    .unwrap_or("user");
                let role = r.get("role").and_then(Value::as_str).unwrap_or("reader");
                let who = r
                    .get("email")
                    .and_then(Value::as_str)
                    .or(r.get("domain").and_then(Value::as_str));
                let p = new_perm(ctx, ty, role, who);
                results.push(format!("  - {}", perm_info(&p)));
                perms.push(p);
            }
            let mut v = vec![
                format!("Batch share results for '{name}'"),
                String::new(),
                format!("Summary: {} succeeded, 0 failed", recipients.len()),
                String::new(),
                "Results:".into(),
            ];
            v.extend(results);
            v.extend([String::new(), format!("View link: {}", link(&f))]);
            v
        }
        "update" => {
            let pid = arg(a, "permission_id")
                .ok_or_else(|| "permission_id is required for update action".to_owned())?
                .to_owned();
            let role = arg(a, "role")
                .ok_or_else(|| "role is required for update action".to_owned())?
                .to_owned();
            let idx = perms.iter().position(|p| s(p, "id") == pid).ok_or_else(|| format!("API error in manage_drive_access: {}", http_error(404, &format!("https://www.googleapis.com/drive/v3/files/{id}/permissions/{pid}?alt=json"), &format!("Permission not found: {pid}."), "notFound")))?;
            perms[idx]["role"] = json!(role);
            vec![
                format!("Successfully updated permission on '{name}'"),
                String::new(),
                "Updated permission:".into(),
                format!("  - {}", perm_info(&perms[idx])),
            ]
        }
        "revoke" => {
            let pid = arg(a, "permission_id")
                .ok_or_else(|| "permission_id is required for revoke action".to_owned())?
                .to_owned();
            if !perms.iter().any(|p| s(p, "id") == pid) {
                return Err(format!(
                    "API error in manage_drive_access: {}",
                    http_error(
                        404,
                        &format!(
                            "https://www.googleapis.com/drive/v3/files/{id}/permissions/{pid}?alt=json"
                        ),
                        &format!("Permission not found: {pid}."),
                        "notFound"
                    )
                ));
            }
            perms.retain(|p| s(p, "id") != pid);
            vec![
                format!("Successfully removed permission from '{name}'"),
                String::new(),
                format!("Permission ID '{pid}' has been revoked."),
            ]
        }
        _ => {
            let new_owner = arg(a, "new_owner_email")
                .ok_or_else(|| "new_owner_email is required for transfer_owner action".to_owned())?
                .to_owned();
            let old: Vec<String> = f["owners"]
                .as_array()
                .map(|o| o.iter().map(|x| s(x, "emailAddress").to_owned()).collect())
                .unwrap_or_default();
            for p in perms.iter_mut() {
                if s(p, "role") == "owner" {
                    p["role"] = json!("writer");
                }
            }
            let mut p = new_perm(ctx, "user", "owner", Some(&new_owner));
            if let Some(existing) = perms.iter_mut().find(|x| s(x, "emailAddress") == new_owner) {
                existing["role"] = json!("owner");
                p = existing.clone();
            } else {
                perms.push(p.clone());
            }
            let _ = p;
            f["owners"] = json!([{"emailAddress": new_owner, "displayName": new_owner}]);
            let mut v = vec![
                format!("Successfully transferred ownership of '{name}'"),
                String::new(),
                format!("New owner: {new_owner}"),
                format!("Previous owner(s): {}", old.join(", ")),
            ];
            if arg_bool(a, "move_to_new_owners_root").unwrap_or(false) {
                v.push(format!("File moved to {new_owner}'s My Drive root."));
            }
            v
        }
    };
    f["permissions"] = json!(perms);
    f["shared"] = json!(
        f["permissions"]
            .as_array()
            .is_some_and(|p| p.iter().any(|x| s(x, "role") != "owner"))
    );
    touch(ctx, &mut f);
    ctx.put("drive_files", f);
    Ok(out.join("\n"))
}

pub fn copy_file(ctx: &mut Ctx, a: &Value) -> R {
    let f = need_file(ctx, "copy_drive_file", s(a, "file_id"))?;
    let parent = arg(a, "parent_folder_id").unwrap_or("root").to_owned();
    let name = arg(a, "new_name")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Copy of {}", s(&f, "name")));
    let mut c = new_file(ctx, &name, s(&f, "mimeType"), Some(&parent));
    for k in ["content", "sheets", "slides", "size", "description"] {
        if let Some(v) = f.get(k) {
            c[k] = v.clone();
        }
    }
    ctx.put("drive_files", c.clone());
    Ok([
        format!("Successfully copied '{}'", s(&f, "name")),
        String::new(),
        format!("Original file ID: {}", s(&f, "id")),
        format!("New file ID: {}", s(&c, "id")),
        format!("New file name: {name}"),
        format!("File type: {}", s(&c, "mimeType")),
        format!("Location: {parent}"),
        String::new(),
        format!("View copied file: {}", link(&c)),
    ]
    .join("\n"))
}

pub fn set_permissions(ctx: &mut Ctx, a: &Value) -> R {
    let mut f = need_file(ctx, "set_drive_file_permissions", s(a, "file_id"))?;
    let mut changes = Vec::new();
    if let Some(w) = arg_bool(a, "writers_can_share") {
        f["writersCanShare"] = json!(w);
        changes.push(format!(
            "  - Editors sharing: {}",
            if w { "enabled" } else { "disabled" }
        ));
    }
    if let Some(c) = arg_bool(a, "copy_requires_writer_permission") {
        f["copyRequiresWriterPermission"] = json!(c);
        changes.push(format!(
            "  - Viewers copy/print/download: {}",
            if c { "disabled" } else { "enabled" }
        ));
    }
    if let Some(ls) = arg(a, "link_sharing") {
        if !["off", "reader", "commenter", "writer"].contains(&ls) {
            return Err(format!(
                "Invalid link_sharing '{ls}'. Must be one of: commenter, off, reader, writer"
            ));
        }
        let mut perms: Vec<Value> = f["permissions"].as_array().cloned().unwrap_or_default();
        let existing = perms.iter().position(|p| s(p, "type") == "anyone");
        match (ls, existing) {
            ("off", Some(i)) => {
                perms.remove(i);
                changes.push("  - Link sharing: disabled".into());
            }
            ("off", None) => changes.push("  - Link sharing: already off".into()),
            (role, Some(i)) => {
                perms[i]["role"] = json!(role);
                changes.push(format!("  - Link sharing: updated to '{role}'"));
            }
            (role, None) => {
                perms.push(new_perm(ctx, "anyone", role, None));
                changes.push(format!("  - Link sharing: enabled as '{role}'"));
            }
        }
        f["permissions"] = json!(perms);
    }
    if changes.is_empty() {
        return Err("No permission settings specified. Provide link_sharing, writers_can_share, or copy_requires_writer_permission.".into());
    }
    touch(ctx, &mut f);
    ctx.put("drive_files", f.clone());
    let mut out = vec![
        format!("Permission settings updated for '{}'", s(&f, "name")),
        String::new(),
        "Changes:".into(),
    ];
    out.extend(changes);
    out.extend([String::new(), format!("View link: {}", link(&f))]);
    Ok(out.join("\n"))
}

// ───────────────────────────── Docs ─────────────────────────────

pub fn ensure_nl(t: &str) -> String {
    if t.ends_with('\n') {
        t.to_owned()
    } else {
        format!("{t}\n")
    }
}

fn doc_link(id: &str) -> String {
    format!("https://docs.google.com/document/d/{id}/edit")
}

fn need_doc(ctx: &Ctx, tool: &str, id: &str) -> Result<Value, String> {
    let f = ctx
        .get("drive_files", id)
        .filter(|f| s(f, "mimeType") == DOC)
        .ok_or_else(|| {
            format!(
                "API error in {tool}: {}",
                http_error(
                    404,
                    &format!("https://docs.googleapis.com/v1/documents/{id}?alt=json"),
                    "Requested entity was not found.",
                    "notFound"
                )
            )
        })?;
    Ok(f)
}

/// Doc id from a raw ID or a docs.google.com URL.
fn doc_id(raw: &str) -> String {
    match raw.split("/d/").nth(1) {
        Some(rest) => rest.split(['/', '?', '#']).next().unwrap_or(raw).to_owned(),
        None => raw.to_owned(),
    }
}

pub fn search_docs(ctx: &mut Ctx, a: &Value) -> R {
    let q = s(a, "query").to_owned();
    let files: Vec<Value> = ctx
        .all("drive_files")
        .into_iter()
        .filter(|f| {
            s(f, "mimeType") == DOC
                && !f["trashed"].as_bool().unwrap_or(false)
                && s(f, "name").to_lowercase().contains(&q.to_lowercase())
        })
        .collect();
    let (page, _) = paginate(files, a, 10);
    if page.is_empty() {
        return Ok(format!("No Google Docs found matching '{q}'."));
    }
    let mut out = vec![format!("Found {} Google Docs matching '{q}':", page.len())];
    out.extend(page.iter().map(|f| {
        format!(
            "- {} (ID: {}) Modified: {} Link: {}",
            s(f, "name"),
            s(f, "id"),
            s(f, "modifiedTime"),
            link(f)
        )
    }));
    Ok(out.join("\n"))
}

pub fn get_doc(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let f = need_file(ctx, "get_doc_content", &id)?;
    if s(&f, "mimeType") != DOC {
        return Ok(format!(
            "File: \"{}\" (ID: {id}, Type: {})\nLink: {}\n\n--- CONTENT ---\n{}",
            s(&f, "name"),
            s(&f, "mimeType"),
            link(&f),
            export_text(&f)
        ));
    }
    let text = ensure_nl(s(&f, "content"));
    let body = match arg(a, "tab_id") {
        Some(t) if t != "t.0" => return Ok(format!("Error: Tab {t} not found in document.")),
        Some(_) => text,
        None => format!("\n--- TAB: Tab 1 (ID: t.0) ---\n{text}"),
    };
    let name = if arg(a, "tab_id").is_some() {
        format!("{} [tab: Tab 1]", s(&f, "name"))
    } else {
        s(&f, "name").to_owned()
    };
    Ok(format!(
        "File: \"{name}\" (ID: {id}, Type: {DOC})\nLink: {}\n\n--- CONTENT ---\n{body}",
        link(&f)
    ))
}

pub fn docs_in_folder(ctx: &mut Ctx, a: &Value) -> R {
    let folder = arg(a, "folder_id").unwrap_or("root").to_owned();
    let target = if folder == "root" {
        root_id(ctx)
    } else {
        folder.clone()
    };
    let items: Vec<Value> = ctx
        .all("drive_files")
        .into_iter()
        .filter(|f| {
            s(f, "mimeType") == DOC
                && !f["trashed"].as_bool().unwrap_or(false)
                && f["parents"]
                    .as_array()
                    .is_some_and(|p| p.iter().any(|x| x == target.as_str()))
        })
        .collect();
    let (page, _) = paginate(items, a, 100);
    if page.is_empty() {
        return Ok(format!("No Google Docs found in folder '{folder}'."));
    }
    let mut out = vec![format!("Found {} Docs in folder '{folder}':", page.len())];
    out.extend(page.iter().map(|f| {
        format!(
            "- {} (ID: {}) Modified: {} Link: {}",
            s(f, "name"),
            s(f, "id"),
            s(f, "modifiedTime"),
            link(f)
        )
    }));
    Ok(out.join("\n"))
}

pub fn create_doc(ctx: &mut Ctx, a: &Value) -> R {
    let title = s(a, "title").to_owned();
    let folder = arg(a, "folder_id");
    let mut f = new_file(ctx, &title, DOC, folder);
    let content = a
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    f["content"] = json!(ensure_nl(&content));
    f["webViewLink"] = json!(link(&f));
    let id = s(&f, "id").to_owned();
    ctx.put("drive_files", f);
    let placement = match folder {
        Some(p) => format!(" Placed in folder {p}."),
        None => String::new(),
    };
    let note = if content.is_empty() {
        "Document is empty (body starts at index 1, total length 2).".to_owned()
    } else {
        format!(
            "Initial content: {} characters inserted.",
            content.chars().count()
        )
    };
    Ok(format!(
        "Created Google Doc '{title}' (ID: {id}) for {USER}.{placement} {note} Use batch_update_doc with end_of_segment=true to append content. Link: {}",
        doc_link(&id)
    ))
}

/// Character offset (0-based) of a 1-based Docs index.
fn char_offset(text: &str, index: i64) -> usize {
    ((index.max(1) - 1) as usize).min(text.chars().count())
}

fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

fn doc_range_error(id: &str, end: usize, start: i64) -> String {
    format!(
        "Error: start_index {start} is beyond the end of the document (length {}) in document {id}.",
        end + 1
    )
}

pub fn modify_doc_text(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let mut f = need_doc(ctx, "modify_doc_text", &id)?;
    let mut text = ensure_nl(s(&f, "content"));
    let len = text.chars().count();
    let new_text = a.get("text").and_then(Value::as_str);
    let end_of_segment = arg_bool(a, "end_of_segment").unwrap_or(false);
    let fmt_keys = [
        "bold",
        "italic",
        "underline",
        "strikethrough",
        "font_size",
        "font_family",
        "font_weight",
        "text_color",
        "background_color",
        "link_url",
        "clear_link",
        "baseline_offset",
        "small_caps",
    ];
    let formatting: Vec<String> = fmt_keys
        .iter()
        .filter_map(|k| {
            a.get(*k)
                .filter(|v| !v.is_null())
                .map(|v| format!("{k}={}", py_repr(v)))
        })
        .collect();
    if new_text.is_none()
        && formatting.is_empty()
        && arg(a, "heading").is_none()
        && arg_i64(a, "heading_level").is_none()
    {
        return Ok("Error: Must provide either 'text' to insert/replace, or formatting parameters (bold, italic, underline, strikethrough, font_size, font_family, text_color, background_color, link_url, clear_link, baseline_offset, small_caps).".into());
    }
    let mut ops = Vec::new();
    let start = arg_i64(a, "start_index");
    let end = arg_i64(a, "end_index");
    if let Some(t) = new_text {
        if end_of_segment {
            let at = byte_at(&text, len - 1);
            text.insert_str(at, t);
            ops.push(format!(
                "Inserted text at end of segment '{}'",
                arg(a, "segment_id").unwrap_or("body")
            ));
        } else {
            let st = start
                .ok_or_else(|| "start_index is required unless end_of_segment=true".to_owned())?;
            if st < 1 {
                return Ok("Error: start_index must be >= 1 (index 0 is the start of the body before the first paragraph).".into());
            }
            if st as usize > len {
                return Ok(doc_range_error(&id, len, st));
            }
            match end.filter(|e| *e > st) {
                Some(en) => {
                    let (sb, eb) = (
                        byte_at(&text, char_offset(&text, st)),
                        byte_at(&text, char_offset(&text, en).min(len - 1)),
                    );
                    text.replace_range(sb..eb, t);
                    ops.push(format!("Replaced text from index {st} to {en}"));
                }
                None => {
                    let b = byte_at(&text, char_offset(&text, st));
                    text.insert_str(b, t);
                    ops.push(format!("Inserted text at index {st}"));
                }
            }
        }
    }
    if !formatting.is_empty() {
        let fs = start.unwrap_or(1);
        let fe = match (new_text, end) {
            (Some(t), _) if end.is_none_or(|e| e <= fs) => fs + t.chars().count() as i64,
            (_, Some(e)) => e,
            _ => fs,
        };
        ops.push(format!(
            "Applied formatting ({}) to range {fs}-{fe}",
            formatting.join(", ")
        ));
        let mut styles = f["textStyles"].as_array().cloned().unwrap_or_default();
        styles.push(json!({"start": fs, "end": fe, "style": formatting}));
        f["textStyles"] = json!(styles);
    }
    if let Some(h) = arg(a, "heading")
        .map(str::to_owned)
        .or_else(|| arg_i64(a, "heading_level").map(|l| format!("HEADING_{l}")))
    {
        ops.push(format!("Applied {h} paragraph style"));
    }
    f["content"] = json!(text);
    touch(ctx, &mut f);
    ctx.put("drive_files", f);
    let info = new_text
        .map(|t| format!(" Text length: {} characters.", t.chars().count()))
        .unwrap_or_default();
    Ok(format!(
        "{} in document {id}.{info} Link: {}",
        ops.join("; "),
        doc_link(&id)
    ))
}

pub fn find_replace(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let mut f = need_doc(ctx, "find_and_replace_doc", &id)?;
    let find = s(a, "find_text").to_owned();
    let replace = s(a, "replace_text").to_owned();
    let text = s(&f, "content").to_owned();
    let match_case = arg_bool(a, "match_case").unwrap_or(false);
    let (count, new) = if find.is_empty() {
        (0, text.clone())
    } else if match_case {
        (text.matches(&find).count(), text.replace(&find, &replace))
    } else {
        let lower = text.to_lowercase();
        let fl = find.to_lowercase();
        let mut out = String::new();
        let mut i = 0;
        let mut n = 0;
        while let Some(pos) = lower[i..].find(&fl) {
            out.push_str(&text[i..i + pos]);
            out.push_str(&replace);
            i += pos + fl.len();
            n += 1;
        }
        out.push_str(&text[i..]);
        (n, out)
    };
    if count > 0 {
        f["content"] = json!(new);
        touch(ctx, &mut f);
        ctx.put("drive_files", f);
    }
    Ok(format!(
        "Replaced {count} occurrence(s) of '{find}' with '{replace}' in document {id}. Link: {}",
        doc_link(&id)
    ))
}

pub fn insert_elements(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let mut f = need_doc(ctx, "insert_doc_elements", &id)?;
    let index = arg_i64(a, "index").unwrap_or(1);
    let et = s(a, "element_type");
    let description = match et {
        "table" => {
            let (r, c) = (
                arg_i64(a, "rows").unwrap_or(0),
                arg_i64(a, "columns").unwrap_or(0),
            );
            if r < 1 || c < 1 {
                return Ok(
                    "Error: 'rows' and 'columns' parameters are required for table insertion."
                        .into(),
                );
            }
            format!("table ({r}x{c})")
        }
        "list" => {
            let lt = arg(a, "list_type").unwrap_or("UNORDERED");
            let t = arg(a, "text").unwrap_or("List item");
            let mut text = ensure_nl(s(&f, "content"));
            let b = byte_at(&text, char_offset(&text, index));
            text.insert_str(b, &format!("{t}\n"));
            f["content"] = json!(text);
            format!("{} list", lt.to_lowercase())
        }
        "page_break" => "page break".to_owned(),
        other => {
            return Ok(format!(
                "Error: Unsupported element type '{other}'. Supported types: 'table', 'list', 'page_break'."
            ));
        }
    };
    let mut els = f["elements"].as_array().cloned().unwrap_or_default();
    els.push(json!({"type": et, "index": index, "rows": arg_i64(a, "rows"), "columns": arg_i64(a, "columns")}));
    f["elements"] = json!(els);
    touch(ctx, &mut f);
    ctx.put("drive_files", f);
    Ok(format!(
        "Inserted {description} at index {index} in document {id}. Link: {}",
        doc_link(&id)
    ))
}

pub fn insert_image(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let mut f = need_doc(ctx, "insert_doc_image", &id)?;
    let src = s(a, "image_source").to_owned();
    let index = arg_i64(a, "index").unwrap_or(1);
    let desc = if src.starts_with("http") {
        "image from URL".to_owned()
    } else {
        let img = ctx.get("drive_files", &src).ok_or_else(|| {
            format!(
                "Error: Could not access Drive file {src}: {}",
                http_error(
                    404,
                    &file_url(&src),
                    &format!("File not found: {src}."),
                    "notFound"
                )
            )
        })?;
        if !s(&img, "mimeType").starts_with("image/") {
            return Ok(format!(
                "Error: File {src} is not an image (MIME type: {}).",
                s(&img, "mimeType")
            ));
        }
        format!("Drive image '{}'", s(&img, "name"))
    };
    let size = match (arg_i64(a, "width"), arg_i64(a, "height")) {
        (Some(w), Some(h)) => format!(" (size: {w}x{h} points)"),
        (Some(w), None) => format!(" (width: {w} points)"),
        (None, Some(h)) => format!(" (height: {h} points)"),
        _ => String::new(),
    };
    let mut els = f["elements"].as_array().cloned().unwrap_or_default();
    els.push(json!({"type": "image", "index": index, "source": src}));
    f["elements"] = json!(els);
    touch(ctx, &mut f);
    ctx.put("drive_files", f);
    Ok(format!(
        "Inserted {desc}{size} at index {index} in document {id}. Link: {}",
        doc_link(&id)
    ))
}

pub fn headers_footers(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let mut f = need_doc(ctx, "update_doc_headers_footers", &id)?;
    let section = s(a, "section_type").to_owned();
    if !["header", "footer"].contains(&section.as_str()) {
        return Ok("Error: section_type must be 'header' or 'footer'".into());
    }
    let kind = arg(a, "header_footer_type").unwrap_or("DEFAULT").to_owned();
    f[format!("{section}_{kind}")] = json!(s(a, "content"));
    touch(ctx, &mut f);
    ctx.put("drive_files", f);
    Ok(format!(
        "Updated {section} ({kind}) content in document {id}. Runtime: header-footer-v2. Link: {}",
        doc_link(&id)
    ))
}

pub fn batch_update_doc(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let ops = match arg_json(a, "operations") {
        Some(Value::Array(o)) if !o.is_empty() => o,
        _ => return Ok("Error: operations must be a non-empty list".into()),
    };
    need_doc(ctx, "batch_update_doc", &id)?;
    let mut done = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let ty = s(op, "type");
        let mut args = op.clone();
        args["document_id"] = json!(id);
        let res = match ty {
            "insert_text" | "replace_text" | "format_text" => modify_doc_text(ctx, &args),
            "find_replace" => {
                args["find_text"] = op.get("find_text").cloned().unwrap_or(json!(""));
                args["replace_text"] = op.get("replace_text").cloned().unwrap_or(json!(""));
                find_replace(ctx, &args)
            }
            "insert_table" => {
                args["element_type"] = json!("table");
                insert_elements(ctx, &args)
            }
            "insert_page_break" => {
                args["element_type"] = json!("page_break");
                insert_elements(ctx, &args)
            }
            "delete_text" => {
                let mut f = need_doc(ctx, "batch_update_doc", &id)?;
                let mut text = ensure_nl(s(&f, "content"));
                let (st, en) = (
                    arg_i64(op, "start_index").unwrap_or(1),
                    arg_i64(op, "end_index").unwrap_or(1),
                );
                let (sb, eb) = (
                    byte_at(&text, char_offset(&text, st)),
                    byte_at(&text, char_offset(&text, en)),
                );
                if sb < eb {
                    text.replace_range(sb..eb, "");
                }
                f["content"] = json!(text);
                ctx.put("drive_files", f);
                Ok(format!("Deleted text from index {st} to {en}"))
            }
            other => {
                return Ok(format!(
                    "Error: Operation {i} has unsupported type '{other}'."
                ));
            }
        };
        match res {
            Ok(r) if r.starts_with("Error") => return Ok(r),
            Ok(_) => done.push(ty.to_owned()),
            Err(e) => return Err(e),
        }
    }
    Ok(format!(
        "Successfully executed {} operations ({}) on document {id}. Link: {}",
        done.len(),
        done.join(", "),
        doc_link(&id)
    ))
}

pub fn inspect_doc(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let f = need_doc(ctx, "inspect_doc_structure", &id)?;
    let text = ensure_nl(s(&f, "content"));
    let paragraphs: Vec<&str> = text.split_inclusive('\n').collect();
    let total = text.chars().count() as i64 + 1;
    let tables = f["elements"]
        .as_array()
        .map(|e| e.iter().filter(|x| s(x, "type") == "table").count())
        .unwrap_or(0);
    let mut result = json!({
        "title": s(&f, "name"),
        "total_length": total,
        "total_elements": paragraphs.len() + tables,
        "statistics": {"paragraphs": paragraphs.len(), "tables": tables, "section_breaks": 1},
        "tabs": [{"tab_id": "t.0", "title": "Tab 1", "index": 0}],
    });
    if arg_bool(a, "detailed").unwrap_or(false) {
        let preview = arg_i64(a, "preview_chars").unwrap_or(30).max(0) as usize;
        let mut idx = 1i64;
        let els: Vec<Value> = paragraphs
            .iter()
            .map(|p| {
                let n = p.chars().count() as i64;
                let e = json!({"type": "paragraph", "start_index": idx, "end_index": idx + n, "text_preview": p.chars().take(preview).collect::<String>()});
                idx += n;
                e
            })
            .collect();
        result["elements"] = json!(els);
    }
    Ok(format!(
        "Document structure analysis for {id}:\n\n{}\n\nLink: {}",
        serde_json::to_string_pretty(&result).expect("JSON"),
        doc_link(&id)
    ))
}

pub fn create_table(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let mut f = need_doc(ctx, "create_table_with_data", &id)?;
    let data = match arg_json(a, "table_data") {
        Some(Value::Array(rows)) if !rows.is_empty() && rows.iter().all(Value::is_array) => rows,
        _ => {
            return Ok(
                "ERROR: table_data must be a non-empty list of lists (rows of cell strings)."
                    .into(),
            );
        }
    };
    let rows = data.len();
    let cols = data
        .iter()
        .map(|r| r.as_array().map_or(0, Vec::len))
        .max()
        .unwrap_or(0);
    let index = arg_i64(a, "index").unwrap_or(1);
    let mut els = f["elements"].as_array().cloned().unwrap_or_default();
    els.push(json!({"type": "table", "index": index, "rows": rows, "columns": cols, "data": data}));
    f["elements"] = json!(els);
    touch(ctx, &mut f);
    ctx.put("drive_files", f);
    Ok(format!(
        "SUCCESS: Table created and populated. Table: {rows}x{cols}, Index: {index}. Link: {}",
        doc_link(&id)
    ))
}

pub fn debug_table(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let f = need_doc(ctx, "debug_table_structure", &id)?;
    let tables: Vec<Value> = f["elements"]
        .as_array()
        .map(|e| {
            e.iter()
                .filter(|x| s(x, "type") == "table")
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let ti = arg_i64(a, "table_index").unwrap_or(0).max(0) as usize;
    let Some(t) = tables.get(ti) else {
        return Ok(format!(
            "Error: Table index {ti} not found. Document has {} table(s).",
            tables.len()
        ));
    };
    let info = json!({"table_index": ti, "dimensions": format!("{}x{}", t["rows"], t["columns"]), "table_range": format!("[{}-{}]", t["index"], t["index"].as_i64().unwrap_or(1) + 3), "cells": t.get("data").cloned().unwrap_or(json!([]))});
    Ok(format!(
        "Table structure debug for table {ti}:\n\n{}\n\nLink: {}",
        serde_json::to_string_pretty(&info).expect("JSON"),
        doc_link(&id)
    ))
}

pub fn export_pdf(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let Some(f) = ctx.get("drive_files", &id) else {
        return Ok(format!(
            "Error: Could not access document {id}: {}",
            http_error(
                404,
                &file_url(&id),
                &format!("File not found: {id}."),
                "notFound"
            )
        ));
    };
    if s(&f, "mimeType") != DOC {
        return Ok(format!(
            "Error: File '{}' is not a Google Doc (MIME type: {}). Only native Google Docs can be exported to PDF.",
            s(&f, "name"),
            s(&f, "mimeType")
        ));
    }
    let name = arg(a, "pdf_filename")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}.pdf", s(&f, "name")));
    let folder = arg(a, "folder_id");
    let mut pdf = new_file(ctx, &name, "application/pdf", folder);
    let size = s(&f, "content").len() * 4 + 2048;
    pdf["size"] = json!(size.to_string());
    ctx.put("drive_files", pdf.clone());
    let folder_info = folder
        .map(|x| format!(" in folder {x}"))
        .unwrap_or_default();
    Ok(format!(
        "Successfully exported '{}' to PDF and saved to Drive as '{name}' (ID: {}, {} bytes){folder_info}. PDF: {} | Original: {}",
        s(&f, "name"),
        s(&pdf, "id"),
        thousands(size),
        link(&pdf),
        link(&f)
    ))
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn paragraph_style(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let mut f = need_doc(ctx, "update_paragraph_style", &id)?;
    let (st, en) = (
        arg_i64(a, "start_index").unwrap_or(1),
        arg_i64(a, "end_index").unwrap_or(1),
    );
    let keys = [
        "heading_level",
        "alignment",
        "line_spacing",
        "indent_first_line",
        "indent_start",
        "indent_end",
        "space_above",
        "space_below",
        "named_style_type",
        "direction",
        "keep_lines_together",
        "keep_with_next",
        "avoid_widow_and_orphan",
        "page_break_before",
        "spacing_mode",
        "shading_color",
        "list_type",
    ];
    let parts: Vec<String> = keys
        .iter()
        .filter_map(|k| {
            a.get(*k)
                .filter(|v| !v.is_null())
                .map(|v| format!("{k}={}", py_repr(v)))
        })
        .collect();
    if parts.is_empty() {
        return Ok(format!(
            "No paragraph style changes or list creation specified for document {id}"
        ));
    }
    let mut styles = f["paragraphStyles"].as_array().cloned().unwrap_or_default();
    styles.push(json!({"start": st, "end": en, "style": parts}));
    f["paragraphStyles"] = json!(styles);
    touch(ctx, &mut f);
    ctx.put("drive_files", f);
    Ok(format!(
        "Applied paragraph formatting ({}) to range {st}-{en} in document {id}. Link: {}",
        parts.join(", "),
        doc_link(&id)
    ))
}

pub fn doc_markdown(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    if let Some(m) = arg(a, "comment_mode")
        && !["inline", "appendix", "none"].contains(&m)
    {
        return Ok(format!(
            "Error: comment_mode must be one of ['inline', 'appendix', 'none'], got '{m}'"
        ));
    }
    let f = need_doc(ctx, "get_doc_as_markdown", &id)?;
    let mut md = s(&f, "content").trim_end().to_owned();
    md.push('\n');
    if arg_bool(a, "include_comments").unwrap_or(true) && arg(a, "comment_mode") != Some("none") {
        let comments: Vec<Value> = ctx
            .all("drive_comments")
            .into_iter()
            .filter(|c| {
                s(c, "fileId") == id
                    && (arg_bool(a, "include_resolved").unwrap_or(false)
                        || !c["resolved"].as_bool().unwrap_or(false))
            })
            .collect();
        if !comments.is_empty() {
            md.push_str("\n---\n\n## Comments\n\n");
            for c in comments {
                md.push_str(&format!(
                    "- **{}**: {}\n",
                    s(&c["author"], "displayName"),
                    s(&c, "content")
                ));
            }
        }
    }
    Ok(md)
}

pub fn manage_tab(ctx: &mut Ctx, a: &Value) -> R {
    let id = doc_id(s(a, "document_id"));
    let mut f = need_doc(ctx, "manage_doc_tab", &id)?;
    let action = s(a, "action");
    let msg = match action {
        "create" => {
            let title = arg(a, "title").unwrap_or("Untitled tab");
            let index = arg_i64(a, "index").unwrap_or(1);
            let mut tabs = f["extraTabs"].as_array().cloned().unwrap_or_default();
            let tid = format!("t.{}", ctx.hex_id("tab", 12));
            tabs.push(json!({"tabId": tid, "title": title}));
            f["extraTabs"] = json!(tabs);
            format!("Inserted tab '{title}' at index {index} in document {id}. Tab ID: {tid}")
        }
        "rename" => format!(
            "Renamed tab {} to '{}' in document {id}.",
            arg(a, "tab_id").unwrap_or("t.0"),
            arg(a, "title").unwrap_or("")
        ),
        "delete" => format!(
            "Deleted tab {} from document {id}.",
            arg(a, "tab_id").unwrap_or("")
        ),
        "populate_from_markdown" => {
            let md = arg(a, "markdown_text").unwrap_or("");
            if arg_bool(a, "replace_existing").unwrap_or(false) {
                f["content"] = json!(ensure_nl(md));
            } else {
                f["content"] = json!(format!("{}{}", ensure_nl(s(&f, "content")), ensure_nl(md)));
            }
            format!(
                "Populated tab {} in document {id} from markdown ({} characters).",
                arg(a, "tab_id").unwrap_or("t.0"),
                md.chars().count()
            )
        }
        other => {
            return Err(format!(
                "Input should be 'create', 'rename', 'delete' or 'populate_from_markdown' (got '{other}')"
            ));
        }
    };
    touch(ctx, &mut f);
    ctx.put("drive_files", f);
    Ok(format!("{msg} Link: {}", doc_link(&id)))
}

// ───────────────────────────── Sheets ─────────────────────────────

fn col_index(letters: &str) -> Option<usize> {
    if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(
        letters
            .to_ascii_uppercase()
            .bytes()
            .fold(0usize, |acc, b| acc * 26 + (b - b'A' + 1) as usize)
            - 1,
    )
}

#[cfg(test)]
fn col_letters(mut i: usize) -> String {
    let mut out = Vec::new();
    loop {
        out.push((b'A' + (i % 26) as u8) as char);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    out.iter().rev().collect()
}

fn parse_cell(c: &str) -> (Option<usize>, Option<usize>) {
    let letters: String = c.chars().take_while(|x| x.is_ascii_alphabetic()).collect();
    let digits: String = c.chars().skip(letters.len()).collect();
    (
        col_index(&letters),
        digits.parse::<usize>().ok().map(|r| r.saturating_sub(1)),
    )
}

/// (sheet title or None, start row, start col, end row (incl), end col (incl)).
pub struct A1 {
    pub sheet: Option<String>,
    pub r0: usize,
    pub c0: usize,
    pub r1: Option<usize>,
    pub c1: Option<usize>,
}

pub fn parse_a1(range: &str) -> A1 {
    let (sheet, cells) = match range.rsplit_once('!') {
        Some((sh, c)) => (Some(sh.trim_matches('\'').replace("''", "'")), c.to_owned()),
        None => {
            let looks_cell = range.split(':').all(|p| {
                let letters = p.chars().take_while(char::is_ascii_alphabetic).count();
                !p.is_empty() && letters <= 3 && p.chars().skip(letters).all(|c| c.is_ascii_digit())
            });
            if looks_cell {
                (None, range.to_owned())
            } else {
                (Some(range.trim_matches('\'').to_owned()), String::new())
            }
        }
    };
    if cells.is_empty() {
        return A1 {
            sheet,
            r0: 0,
            c0: 0,
            r1: None,
            c1: None,
        };
    }
    let (a, b) = cells.split_once(':').unwrap_or((&cells, &cells));
    let (ac, ar) = parse_cell(a);
    let (bc, br) = parse_cell(b);
    A1 {
        sheet,
        r0: ar.unwrap_or(0),
        c0: ac.unwrap_or(0),
        r1: if b.is_empty() { None } else { br },
        c1: bc,
    }
}

fn need_sheet_file(ctx: &Ctx, tool: &str, id: &str) -> Result<Value, String> {
    ctx.get("drive_files", id)
        .filter(|f| s(f, "mimeType") == SHEET)
        .ok_or_else(|| {
            format!(
                "API error in {tool}: {}",
                http_error(
                    404,
                    &format!("https://sheets.googleapis.com/v4/spreadsheets/{id}?alt=json"),
                    "Requested entity was not found.",
                    "notFound"
                )
            )
        })
}

fn sheet_index(f: &Value, title: Option<&str>) -> Option<usize> {
    let sheets = f["sheets"].as_array()?;
    match title {
        None => (!sheets.is_empty()).then_some(0),
        Some(t) => sheets.iter().position(|sh| s(sh, "title") == t),
    }
}

fn range_err(tool: &str, id: &str, range: &str) -> String {
    crate::util::bad_request(
        tool,
        &format!("https://sheets.googleapis.com/v4/spreadsheets/{id}/values/{range}?alt=json"),
        &format!("Unable to parse range: {range}"),
    )
}

fn cell_str(v: &Value) -> String {
    match v {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        other => other.to_string(),
    }
}

pub fn list_spreadsheets(ctx: &mut Ctx, a: &Value) -> R {
    let mut files: Vec<Value> = ctx
        .all("drive_files")
        .into_iter()
        .filter(|f| s(f, "mimeType") == SHEET && !f["trashed"].as_bool().unwrap_or(false))
        .collect();
    files.sort_by(|x, y| s(y, "modifiedTime").cmp(s(x, "modifiedTime")));
    let mut a2 = a.clone();
    if let Some(m) = a.get("max_results") {
        a2["page_size"] = m.clone();
    }
    let (page, next) = paginate(files, &a2, 25);
    if page.is_empty() {
        return Ok(format!("No spreadsheets found for {USER}."));
    }
    let lines: Vec<String> = page
        .iter()
        .map(|f| {
            format!(
                "- \"{}\" (ID: {}) | Modified: {} | Link: {}",
                s(f, "name"),
                s(f, "id"),
                s(f, "modifiedTime"),
                link(f)
            )
        })
        .collect();
    let mut out = format!(
        "Successfully listed {} spreadsheets for {USER}:\n{}",
        page.len(),
        lines.join("\n")
    );
    if let Some(n) = next {
        out.push_str(&format!("\nnextPageToken: {n}"));
    }
    Ok(out)
}

pub fn spreadsheet_info(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "spreadsheet_id").to_owned();
    let f = need_sheet_file(ctx, "get_spreadsheet_info", &id)?;
    let sheets = f["sheets"].as_array().cloned().unwrap_or_default();
    let lines: Vec<String> = sheets
        .iter()
        .map(|sh| {
            let rules = sh["conditionalFormats"].as_array().map_or(0, Vec::len);
            format!(
                "  - \"{}\" (ID: {}) | Size: {}x{} | Conditional formats: {rules}",
                s(sh, "title"),
                sh["sheetId"],
                sh["rowCount"].as_i64().unwrap_or(1000),
                sh["columnCount"].as_i64().unwrap_or(26)
            )
        })
        .collect();
    Ok(format!(
        "Spreadsheet: \"{}\" (ID: {id}) | Locale: {}\nSheets ({}):\n{}",
        s(&f, "name"),
        f.get("locale").and_then(Value::as_str).unwrap_or("en_US"),
        sheets.len(),
        if lines.is_empty() {
            "  No sheets found".to_owned()
        } else {
            lines.join("\n")
        }
    ))
}

pub fn read_values(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "spreadsheet_id").to_owned();
    let range = arg(a, "range_name").unwrap_or("A1:Z1000").to_owned();
    let f = need_sheet_file(ctx, "read_sheet_values", &id)?;
    let r = parse_a1(&range);
    let si = sheet_index(&f, r.sheet.as_deref())
        .ok_or_else(|| range_err("read_sheet_values", &id, &range))?;
    let sheet = &f["sheets"][si];
    let rows: Vec<Vec<String>> = sheet["values"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row.as_array()
                        .map(|c| c.iter().map(cell_str).collect())
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    let r1 = r.r1.unwrap_or(r.r0.max(999)).min(r.r0 + 999);
    let mut out: Vec<Vec<String>> = Vec::new();
    for ri in r.r0..=r1 {
        let row = rows.get(ri).cloned().unwrap_or_default();
        let c1 = r.c1.unwrap_or(row.len().max(1) - 1);
        let cells: Vec<String> = (r.c0..=c1.max(r.c0))
            .map(|ci| row.get(ci).cloned().unwrap_or_default())
            .collect();
        let trimmed_len = cells
            .iter()
            .rposition(|c| !c.is_empty())
            .map_or(0, |p| p + 1);
        out.push(cells[..trimmed_len].to_vec());
    }
    while out.last().is_some_and(Vec::is_empty) {
        out.pop();
    }
    if out.is_empty() {
        return Ok(format!("No data found in range '{range}' for {USER}."));
    }
    let width = out[0].len();
    let lines: Vec<String> = out
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let mut padded = row.clone();
            while padded.len() < width {
                padded.push(String::new());
            }
            format!("Row {:2}: {}", i + 1, py_repr(&json!(padded)))
        })
        .collect();
    Ok(format!(
        "Successfully read {} rows from range '{range}' in spreadsheet {id} for {USER}:\n{}",
        out.len(),
        lines.join("\n")
    ))
}

pub fn modify_values(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "spreadsheet_id").to_owned();
    let range = s(a, "range_name").to_owned();
    let mut f = need_sheet_file(ctx, "modify_sheet_values", &id)?;
    let r = parse_a1(&range);
    let si = sheet_index(&f, r.sheet.as_deref())
        .ok_or_else(|| range_err("modify_sheet_values", &id, &range))?;
    let clear = arg_bool(a, "clear_values").unwrap_or(false);
    let values: Option<Vec<Vec<Value>>> = match arg_json(a, "values") {
        Some(Value::Array(rows)) => {
            let mut out = Vec::new();
            for (i, row) in rows.iter().enumerate() {
                match row {
                    Value::Array(c) => out.push(c.clone()),
                    other => {
                        return Err(format!(
                            "Invalid values structure: Row {i} must be a list, got {}",
                            match other {
                                Value::String(_) => "str",
                                Value::Number(_) => "int",
                                Value::Object(_) => "dict",
                                _ => "NoneType",
                            }
                        ));
                    }
                }
            }
            Some(out)
        }
        Some(Value::String(x)) => {
            return Err(format!(
                "Invalid JSON format for values: Expecting value: line 1 column 1 (char 0) ({x})"
            ));
        }
        _ => None,
    };
    if !clear && values.as_ref().is_none_or(Vec::is_empty) {
        return Err(
            "Either 'values' or 'chips' must be provided, or 'clear_values' must be True.".into(),
        );
    }
    let mut rows: Vec<Vec<Value>> = f["sheets"][si]["values"]
        .as_array()
        .map(|x| {
            x.iter()
                .map(|r| r.as_array().cloned().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    let title = s(&f["sheets"][si], "title").to_owned();
    let msg = if clear {
        let r1 = r.r1.unwrap_or(rows.len().saturating_sub(1));
        for row in rows.iter_mut().take(r1 + 1).skip(r.r0) {
            let c1 = r.c1.unwrap_or(row.len().saturating_sub(1));
            for ci in r.c0..=c1 {
                if let Some(c) = row.get_mut(ci) {
                    *c = json!("");
                }
            }
        }
        format!(
            "Successfully cleared range '{}' in spreadsheet {id} for {USER}.",
            qualified(&title, &range)
        )
    } else {
        let vals = values.unwrap_or_default();
        let raw = arg(a, "value_input_option") == Some("RAW");
        let mut cells = 0;
        let mut cols = 0;
        for (i, vr) in vals.iter().enumerate() {
            let ri = r.r0 + i;
            while rows.len() <= ri {
                rows.push(Vec::new());
            }
            for (j, v) in vr.iter().enumerate() {
                let ci = r.c0 + j;
                while rows[ri].len() <= ci {
                    rows[ri].push(json!(""));
                }
                let text = cell_str(v);
                rows[ri][ci] = json!(if raw {
                    text
                } else {
                    text.trim_start_matches('\'').to_owned()
                });
                cells += 1;
            }
            cols = cols.max(vr.len());
        }
        format!(
            "Successfully updated range '{range}' in spreadsheet {id} for {USER}. Updated: {cells} cells, {} rows, {cols} columns.",
            vals.len()
        )
    };
    f["sheets"][si]["values"] = json!(rows);
    touch(ctx, &mut f);
    ctx.put("drive_files", f);
    Ok(msg)
}

fn qualified(title: &str, range: &str) -> String {
    if range.contains('!') {
        range.to_owned()
    } else {
        let t = if title.contains(' ') {
            format!("'{title}'")
        } else {
            title.to_owned()
        };
        format!("{t}!{range}")
    }
}

pub fn format_range(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "spreadsheet_id").to_owned();
    let range = s(a, "range_name").to_owned();
    let mut f = need_sheet_file(ctx, "format_sheet_range", &id)?;
    let r = parse_a1(&range);
    let si = sheet_index(&f, r.sheet.as_deref())
        .ok_or_else(|| range_err("format_sheet_range", &id, &range))?;
    let keys = [
        "background_color",
        "text_color",
        "number_format_type",
        "number_format_pattern",
        "wrap_strategy",
        "horizontal_alignment",
        "vertical_alignment",
        "bold",
        "italic",
        "font_size",
        "merge_cells",
        "merge_type",
        "borders",
        "border_style",
        "border_color",
        "dropdown_values",
        "dropdown_strict",
        "clear_dropdown",
    ];
    let applied: Vec<String> = keys
        .iter()
        .filter(|k| a.get(**k).is_some_and(|v| !v.is_null()))
        .map(|k| k.replace('_', " "))
        .collect();
    if applied.is_empty() {
        return Err("Provide at least one formatting option (colors, number format, alignment, text style, borders, merge, or dropdown).".into());
    }
    let mut fm = f["sheets"][si]["formats"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    fm.push(json!({"range": range, "options": applied}));
    f["sheets"][si]["formats"] = json!(fm);
    ctx.put("drive_files", f);
    Ok(format!(
        "Applied formatting to range '{range}' in spreadsheet {id} for {USER}: {}.",
        applied.join(", ")
    ))
}

pub fn conditional_formatting(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "spreadsheet_id").to_owned();
    let mut f = need_sheet_file(ctx, "manage_conditional_formatting", &id)?;
    let action = s(a, "action").to_lowercase();
    let sheet_title = arg(a, "sheet_name")
        .map(str::to_owned)
        .or_else(|| arg(a, "range_name").and_then(|r| parse_a1(r).sheet));
    let si = sheet_index(&f, sheet_title.as_deref()).ok_or_else(|| {
        range_err(
            "manage_conditional_formatting",
            &id,
            arg(a, "range_name").unwrap_or(""),
        )
    })?;
    let mut rules = f["sheets"][si]["conditionalFormats"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let title = s(&f["sheets"][si], "title").to_owned();
    let msg = match action.as_str() {
        "add" => {
            let range =
                arg(a, "range_name").ok_or_else(|| "range_name is required for add.".to_owned())?;
            let ct = arg(a, "condition_type").ok_or_else(|| {
                "condition_type is required unless gradient_points is provided.".to_owned()
            })?;
            rules.push(json!({"range": range, "condition": ct, "values": arg_json(a, "condition_values"), "background": arg(a, "background_color"), "text": arg(a, "text_color")}));
            format!(
                "Added conditional format on '{range}' in sheet '{title}' (rule index {}).",
                rules.len() - 1
            )
        }
        "update" | "delete" => {
            let idx = arg_i64(a, "rule_index")
                .ok_or_else(|| format!("rule_index is required for {action}."))?
                as usize;
            if idx >= rules.len() {
                return Err(format!(
                    "rule_index {idx} is out of range; sheet '{title}' has {} conditional format rule(s).",
                    rules.len()
                ));
            }
            if action == "delete" {
                rules.remove(idx);
                format!("Deleted conditional format at index {idx} on sheet '{title}'.")
            } else {
                if let Some(ct) = arg(a, "condition_type") {
                    rules[idx]["condition"] = json!(ct);
                }
                format!("Updated conditional format at index {idx} on sheet '{title}'.")
            }
        }
        other => {
            return Err(format!(
                "Invalid action '{other}'. Must be 'add', 'update', or 'delete'."
            ));
        }
    };
    f["sheets"][si]["conditionalFormats"] = json!(rules);
    ctx.put("drive_files", f);
    Ok(msg)
}

pub fn create_spreadsheet(ctx: &mut Ctx, a: &Value) -> R {
    let title = s(a, "title").to_owned();
    let names = arg_list(a, "sheet_names")
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| vec!["Sheet1".into()]);
    let mut f = new_file(ctx, &title, SHEET, arg(a, "folder_id"));
    let sheets: Vec<Value> = names
        .iter()
        .enumerate()
        .map(|(i, n)| json!({"sheetId": if i == 0 { 0 } else { 1_000_000 + i as i64 * 7919 }, "title": n, "rowCount": 1000, "columnCount": 26, "values": []}))
        .collect();
    f["sheets"] = json!(sheets);
    f["locale"] = json!("en_US");
    let id = s(&f, "id").to_owned();
    ctx.put("drive_files", f);
    let placement = arg(a, "folder_id")
        .map(|p| format!(" Placed in folder {p}."))
        .unwrap_or_default();
    Ok(format!(
        "Successfully created spreadsheet '{title}' for {USER}.{placement} ID: {id} | URL: https://docs.google.com/spreadsheets/d/{id}/edit | Locale: en_US"
    ))
}

pub fn create_sheet(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "spreadsheet_id").to_owned();
    let mut f = need_sheet_file(ctx, "create_sheet", &id)?;
    let mut sheets = f["sheets"].as_array().cloned().unwrap_or_default();
    let sheet_id = ctx.st.ids.next_int("sheet_id", 1_276_430_001) as i64;
    if let Some(src) = arg(a, "source_sheet_name") {
        let source = sheets
            .iter()
            .find(|x| s(x, "title") == src)
            .cloned()
            .ok_or_else(|| format!("Source sheet '{src}' not found in spreadsheet {id}."))?;
        let new_title = arg(a, "sheet_name")
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Copy of {src}"));
        let mut copy = source;
        copy["sheetId"] = json!(sheet_id);
        copy["title"] = json!(new_title);
        sheets.push(copy);
        f["sheets"] = json!(sheets);
        ctx.put("drive_files", f);
        return Ok(format!(
            "Successfully duplicated '{src}' to '{new_title}' (ID: {sheet_id}) in spreadsheet {id} for {USER}."
        ));
    }
    let name = arg(a, "sheet_name")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Sheet{}", sheets.len() + 1));
    if sheets.iter().any(|x| s(x, "title") == name) {
        return Err(crate::util::bad_request(
            "create_sheet",
            &format!("https://sheets.googleapis.com/v4/spreadsheets/{id}:batchUpdate?alt=json"),
            &format!(
                "Invalid requests[0].addSheet: A sheet with the name \"{name}\" already exists. Please enter another name."
            ),
        ));
    }
    let sheet = json!({"sheetId": sheet_id, "title": name, "rowCount": 1000, "columnCount": 26, "values": []});
    match arg_i64(a, "insert_sheet_index") {
        Some(i) => sheets.insert((i.max(0) as usize).min(sheets.len()), sheet),
        None => sheets.push(sheet),
    }
    f["sheets"] = json!(sheets);
    ctx.put("drive_files", f);
    Ok(format!(
        "Successfully created sheet '{name}' (ID: {sheet_id}) in spreadsheet {id} for {USER}."
    ))
}

pub fn list_tables(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "spreadsheet_id").to_owned();
    let f = need_sheet_file(ctx, "list_sheet_tables", &id)?;
    let mut parts = Vec::new();
    for sh in f["sheets"].as_array().cloned().unwrap_or_default() {
        for t in sh["tables"].as_array().cloned().unwrap_or_default() {
            let cols: Vec<String> = t["columns"]
                .as_array()
                .map(|c| {
                    c.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            parts.push(format!(
                "  Table ID: {}\n  Name: {}\n  Sheet: {}\n  Range: rows {}-{}, cols {}-{}\n  Columns: {}",
                s(&t, "tableId"),
                s(&t, "name"),
                s(&sh, "title"),
                t["startRow"],
                t["endRow"],
                t["startColumn"],
                t["endColumn"],
                if cols.is_empty() { "N/A".into() } else { cols.join(", ") }
            ));
        }
    }
    if parts.is_empty() {
        return Ok(format!("No tables found in spreadsheet {id}."));
    }
    Ok(format!(
        "Found {} table(s) in spreadsheet {id} for {USER}:\n\n{}",
        parts.len(),
        parts.join("\n\n")
    ))
}

pub fn append_rows(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "spreadsheet_id").to_owned();
    let tid = s(a, "table_id").to_owned();
    let mut f = need_sheet_file(ctx, "append_table_rows", &id)?;
    let values: Vec<Vec<Value>> = match arg_json(a, "values") {
        Some(Value::Array(rows)) if !rows.is_empty() => rows
            .iter()
            .map(|r| r.as_array().cloned().unwrap_or_else(|| vec![r.clone()]))
            .collect(),
        _ => return Err("values must be a non-empty list of rows (list of lists).".into()),
    };
    let sheets = f["sheets"].as_array().cloned().unwrap_or_default();
    let (si, ti) = sheets
        .iter()
        .enumerate()
        .find_map(|(i, sh)| {
            sh["tables"]
                .as_array()
                .and_then(|t| t.iter().position(|x| s(x, "tableId") == tid))
                .map(|ti| (i, ti))
        })
        .ok_or_else(|| format!("Table '{tid}' not found in spreadsheet {id}."))?;
    let table = f["sheets"][si]["tables"][ti].clone();
    let end_row = table["endRow"].as_i64().unwrap_or(0) as usize;
    let start_col = table["startColumn"].as_i64().unwrap_or(0) as usize;
    let mut rows: Vec<Vec<Value>> = f["sheets"][si]["values"]
        .as_array()
        .map(|x| {
            x.iter()
                .map(|r| r.as_array().cloned().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    for (i, vr) in values.iter().enumerate() {
        let ri = end_row + i;
        while rows.len() <= ri {
            rows.push(Vec::new());
        }
        for (j, v) in vr.iter().enumerate() {
            let ci = start_col + j;
            while rows[ri].len() <= ci {
                rows[ri].push(json!(""));
            }
            rows[ri][ci] = json!(cell_str(v));
        }
    }
    f["sheets"][si]["values"] = json!(rows);
    f["sheets"][si]["tables"][ti]["endRow"] = json!(end_row + values.len());
    touch(ctx, &mut f);
    ctx.put("drive_files", f);
    Ok(format!(
        "Successfully appended {} row(s) to table '{tid}' in spreadsheet {id} for {USER}.",
        values.len()
    ))
}

pub fn manage_tab_sheet(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "spreadsheet_id").to_owned();
    let name = s(a, "sheet_name").to_owned();
    let mut f = need_sheet_file(ctx, "manage_sheet_tab", &id)?;
    let mut sheets = f["sheets"].as_array().cloned().unwrap_or_default();
    let idx = sheets
        .iter()
        .position(|x| s(x, "title") == name)
        .ok_or_else(|| format!("Sheet '{name}' not found in spreadsheet {id}."))?;
    let sheet_id = sheets[idx]["sheetId"].clone();
    let action = s(a, "action").to_lowercase();
    let summary = match action.as_str() {
        "rename" => {
            let n =
                arg(a, "new_name").ok_or_else(|| "new_name is required for rename.".to_owned())?;
            sheets[idx]["title"] = json!(n);
            format!("renamed sheet '{name}' to '{n}'")
        }
        "delete" => {
            if sheets.len() == 1 {
                return Err(crate::util::bad_request(
                    "manage_sheet_tab",
                    &format!(
                        "https://sheets.googleapis.com/v4/spreadsheets/{id}:batchUpdate?alt=json"
                    ),
                    "Invalid requests[0].deleteSheet: You can't remove all the sheets in a document.",
                ));
            }
            sheets.remove(idx);
            format!("deleted sheet '{name}'")
        }
        "move" => {
            let to = arg_i64(a, "new_index")
                .ok_or_else(|| "new_index is required for move.".to_owned())?
                .max(0) as usize;
            let sh = sheets.remove(idx);
            sheets.insert(to.min(sheets.len()), sh);
            format!("moved sheet '{name}' to index {to}")
        }
        "color" | "set_color" | "tab_color" => {
            sheets[idx]["tabColor"] = json!(arg(a, "tab_color"));
            format!("set tab color of '{name}'")
        }
        "hide" | "unhide" => {
            sheets[idx]["hidden"] = json!(action == "hide");
            format!("{action} sheet '{name}'")
        }
        "gridlines" | "set_gridlines" => {
            sheets[idx]["hideGridlines"] = json!(!arg_bool(a, "show_gridlines").unwrap_or(true));
            format!("updated gridlines on '{name}'")
        }
        other => {
            return Err(format!(
                "Invalid action '{other}'. Must be one of: rename, delete, move, hide, unhide, color, gridlines."
            ));
        }
    };
    f["sheets"] = json!(sheets);
    ctx.put("drive_files", f);
    Ok(format!(
        "Successfully {summary} (ID: {sheet_id}) in spreadsheet {id} for {USER}."
    ))
}

pub fn resize_dimensions(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "spreadsheet_id").to_owned();
    let mut f = need_sheet_file(ctx, "resize_sheet_dimensions", &id)?;
    let si = sheet_index(&f, arg(a, "sheet_name")).ok_or_else(|| {
        format!(
            "Sheet '{}' not found in spreadsheet {id}.",
            arg(a, "sheet_name").unwrap_or("")
        )
    })?;
    let keys = [
        "column_sizes",
        "row_sizes",
        "auto_resize_columns",
        "auto_resize_rows",
        "frozen_row_count",
        "frozen_column_count",
        "hide_columns",
        "unhide_columns",
        "hide_rows",
        "unhide_rows",
        "insert_rows",
        "insert_columns",
        "delete_rows",
        "delete_row_range",
        "delete_columns",
    ];
    let ops: Vec<&str> = keys
        .iter()
        .copied()
        .filter(|k| a.get(*k).is_some_and(|v| !v.is_null()))
        .collect();
    if ops.is_empty() {
        return Err("Provide at least one dimension operation.".into());
    }
    if let Some(n) = arg_i64(a, "frozen_row_count") {
        f["sheets"][si]["frozenRowCount"] = json!(n);
    }
    if let Some(n) = arg_i64(a, "insert_rows") {
        let at = arg_i64(a, "insert_rows_at").unwrap_or(0).max(0) as usize;
        let mut rows: Vec<Value> = f["sheets"][si]["values"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for _ in 0..n {
            rows.insert(at.min(rows.len()), json!([]));
        }
        f["sheets"][si]["values"] = json!(rows);
    }
    let title = s(&f["sheets"][si], "title").to_owned();
    ctx.put("drive_files", f);
    Ok(format!(
        "Successfully applied {} dimension change(s) to sheet '{title}' in spreadsheet {id} for {USER}: {}.",
        ops.len(),
        ops.join(", ")
    ))
}

pub fn move_rows(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "spreadsheet_id").to_owned();
    let mut f = need_sheet_file(ctx, "move_sheet_rows", &id)?;
    let src = s(a, "source_sheet").to_owned();
    let dst = s(a, "destination_sheet").to_owned();
    let (start, end) = (
        arg_i64(a, "start_row").unwrap_or(1).max(1) as usize,
        arg_i64(a, "end_row").unwrap_or(1).max(1) as usize,
    );
    let si = sheet_index(&f, Some(&src))
        .ok_or_else(|| format!("Sheet '{src}' not found in spreadsheet {id}."))?;
    let di = sheet_index(&f, Some(&dst))
        .ok_or_else(|| format!("Sheet '{dst}' not found in spreadsheet {id}."))?;
    if end < start {
        return Err("end_row must be >= start_row.".into());
    }
    let mut srows: Vec<Value> = f["sheets"][si]["values"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let moved: Vec<Value> = (start - 1..end)
        .filter_map(|i| srows.get(i).cloned())
        .collect();
    for _ in start - 1..end.min(srows.len()) {
        srows.remove(start - 1);
    }
    f["sheets"][si]["values"] = json!(srows);
    let mut drows: Vec<Value> = f["sheets"][di]["values"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    drows.extend(moved.iter().cloned());
    f["sheets"][di]["values"] = json!(drows);
    touch(ctx, &mut f);
    ctx.put("drive_files", f);
    Ok(format!(
        "Successfully moved {} row(s) from '{src}' (rows {start}-{end}) to the end of '{dst}' in spreadsheet {id} for {USER}.",
        moved.len()
    ))
}

pub fn named_range(ctx: &mut Ctx, a: &Value) -> R {
    let id = s(a, "spreadsheet_id").to_owned();
    let mut f = need_sheet_file(ctx, "manage_named_range", &id)?;
    let mut ranges = f["namedRanges"].as_array().cloned().unwrap_or_default();
    let action = s(a, "action").to_lowercase();
    let msg = match action.as_str() {
        "list" => {
            if ranges.is_empty() {
                return Ok(format!("No named ranges found in spreadsheet {id}."));
            }
            let lines: Vec<String> = ranges
                .iter()
                .map(|r| {
                    format!(
                        "- {} (ID: {}): {}",
                        s(r, "name"),
                        s(r, "namedRangeId"),
                        s(r, "range")
                    )
                })
                .collect();
            return Ok(format!(
                "Named ranges in spreadsheet {id}:\n{}",
                lines.join("\n")
            ));
        }
        "create" | "add" => {
            let name = arg(a, "name").ok_or_else(|| "name is required for create.".to_owned())?;
            let range = arg(a, "range_name")
                .ok_or_else(|| "range_name is required for create.".to_owned())?;
            let rid = ctx.hex_id("named_range", 12);
            ranges.push(json!({"namedRangeId": rid, "name": name, "range": range}));
            format!("Created named range '{name}' (ID: {rid}) for {range} in spreadsheet {id}.")
        }
        "update" => {
            let rid = arg(a, "named_range_id")
                .ok_or_else(|| "named_range_id is required for update.".to_owned())?;
            let r = ranges
                .iter_mut()
                .find(|r| s(r, "namedRangeId") == rid)
                .ok_or_else(|| format!("Named range '{rid}' not found."))?;
            if let Some(n) = arg(a, "new_name") {
                r["name"] = json!(n);
            }
            if let Some(n) = arg(a, "new_range") {
                r["range"] = json!(n);
            }
            format!("Updated named range {rid} in spreadsheet {id}.")
        }
        "delete" => {
            let rid = arg(a, "named_range_id")
                .ok_or_else(|| "named_range_id is required for delete.".to_owned())?
                .to_owned();
            if !ranges.iter().any(|r| s(r, "namedRangeId") == rid) {
                return Err(format!("Named range '{rid}' not found."));
            }
            ranges.retain(|r| s(r, "namedRangeId") != rid);
            format!("Deleted named range {rid} from spreadsheet {id}.")
        }
        other => {
            return Err(format!(
                "Invalid action '{other}'. Must be 'list', 'create', 'update', or 'delete'."
            ));
        }
    };
    f["namedRanges"] = json!(ranges);
    ctx.put("drive_files", f);
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a1_ranges() {
        let r = parse_a1("'Q4 Budget'!B2:D10");
        assert_eq!(
            (r.sheet.as_deref(), r.r0, r.c0, r.r1, r.c1),
            (Some("Q4 Budget"), 1, 1, Some(9), Some(3))
        );
        let r = parse_a1("A:C");
        assert_eq!(
            (r.sheet, r.r0, r.c0, r.r1, r.c1),
            (None, 0, 0, None, Some(2))
        );
        let r = parse_a1("Leads");
        assert_eq!(r.sheet.as_deref(), Some("Leads"));
        assert_eq!(col_letters(27), "AB");
    }
}
