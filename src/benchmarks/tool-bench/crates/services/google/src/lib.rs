//! Mock Google Workspace (replica mode): the 119 tools of `workspace-mcp` 2.2.1
//! (`--tool-tier complete`, single user `ops@octo-corp.example`) over one store.
//!
//! Collections: `gmail_messages`, `gmail_labels`, `gmail_drafts`, `gmail_filters`,
//! `gmail_settings`, `calendars`, `calendar_events`, `calendar_freebusy`, `drive_files`
//! (Docs, Sheets, Slides, folders and plain files), `drive_comments`, `drive_settings`,
//! `shared_drives`, `task_lists`, `tasks`, `forms`, `form_responses`, `chat_spaces`,
//! `chat_messages`, `contacts`, `contact_groups`, `search_results`, `scripts`.
//!
//! Results follow FastMCP: the text in `content` plus `structuredContent: {"result": text}`;
//! failures are `Error calling tool '<name>': <message>` with `isError: true`.

mod calendar;
mod drive;
mod gmail;
mod other;
mod tz;
mod util;

use axum::{Json, Router, extract::State, http::StatusCode, response::Response, routing::post};
use mock_core::{
    http::json,
    mcp::{TOOLS_CALL_PATH, ToolCall},
    service::MockService,
    world::SharedWorld,
};
use serde_json::{Value, json as json_value};

use util::{Ctx, R};

pub const NAME: &str = "google";
pub const PORT: u16 = 8130;

pub struct Google;

impl MockService for Google {
    fn name(&self) -> &'static str {
        NAME
    }

    fn port(&self) -> u16 {
        PORT
    }

    fn hosts(&self) -> &'static [&'static str] {
        &[]
    }

    fn router(&self) -> Router<SharedWorld> {
        Router::new()
            .route(TOOLS_CALL_PATH, post(tools_call))
            .fallback(not_found)
    }
}

async fn not_found() -> Response {
    json(
        StatusCode::NOT_FOUND,
        json_value!({ "message": "Not Found" }),
    )
}

async fn tools_call(State(world): State<SharedWorld>, Json(call): Json<ToolCall>) -> Json<Value> {
    let mut guard = world.lock().expect("world lock");
    let now = guard.clock.now_unix();
    let st = guard.service_mut(NAME);
    let mut ctx = Ctx { st, now };
    Json(call_tool(&mut ctx, &call.name, &call.arguments))
}

/// Required arguments of every tool (from `servers/snapshots/google-workspace.json`).
const REQUIRED: &[(&str, &[&str])] = &[
    ("start_google_auth", &["service_name"]),
    ("get_script_project", &["action"]),
    ("manage_script_content", &["action", "script_id"]),
    ("run_script_function", &["script_id", "function_name"]),
    ("manage_deployment", &["action", "script_id"]),
    ("list_script_deployments", &["script_id"]),
    ("manage_script_project", &["action"]),
    ("manage_script_version", &["action", "script_id"]),
    ("get_script_version", &["action", "script_id"]),
    ("get_script_activity", &["action"]),
    ("generate_trigger_code", &["trigger_type", "function_name"]),
    ("manage_script_trigger", &["action", "script_id"]),
    ("list_calendars", &[]),
    ("get_events", &[]),
    ("manage_event", &["action"]),
    ("manage_out_of_office", &["action"]),
    ("manage_focus_time", &["action"]),
    ("query_freebusy", &["time_min", "time_max"]),
    ("create_calendar", &["summary"]),
    ("list_spaces", &[]),
    ("get_messages", &["space_id"]),
    ("send_message", &["space_id", "message_text"]),
    ("search_messages", &[]),
    ("create_reaction", &["message_id", "emoji_unicode"]),
    ("download_chat_attachment", &["message_id"]),
    ("list_contacts", &[]),
    ("get_contact", &["contact_id"]),
    ("search_contacts", &["query"]),
    ("manage_contact", &["action"]),
    ("list_contact_groups", &[]),
    ("get_contact_group", &["group_id"]),
    ("manage_contacts_batch", &["action"]),
    ("manage_contact_group", &["action"]),
    ("search_docs", &["query"]),
    ("get_doc_content", &["document_id"]),
    ("list_docs_in_folder", &[]),
    ("create_doc", &["title"]),
    ("modify_doc_text", &["document_id"]),
    (
        "find_and_replace_doc",
        &["document_id", "find_text", "replace_text"],
    ),
    (
        "insert_doc_elements",
        &["document_id", "element_type", "index"],
    ),
    (
        "insert_doc_image",
        &["document_id", "image_source", "index"],
    ),
    (
        "update_doc_headers_footers",
        &["document_id", "section_type", "content"],
    ),
    ("batch_update_doc", &["document_id", "operations"]),
    ("inspect_doc_structure", &["document_id"]),
    (
        "create_table_with_data",
        &["document_id", "table_data", "index"],
    ),
    ("debug_table_structure", &["document_id"]),
    ("export_doc_to_pdf", &["document_id"]),
    (
        "update_paragraph_style",
        &["document_id", "start_index", "end_index"],
    ),
    ("get_doc_as_markdown", &["document_id"]),
    ("manage_doc_tab", &["document_id", "action"]),
    ("list_document_comments", &["document_id"]),
    ("manage_document_comment", &["document_id", "action"]),
    ("search_drive_files", &["query"]),
    ("get_drive_file_content", &["file_id"]),
    ("get_drive_file_download_url", &["file_id"]),
    ("list_drive_items", &[]),
    ("create_drive_folder", &["folder_name"]),
    ("create_drive_file", &["file_name"]),
    ("import_to_google_doc", &["file_name"]),
    ("import_to_google_slides", &["file_name"]),
    ("import_to_google_sheets", &["file_name"]),
    ("get_drive_file_permissions", &["file_id"]),
    ("check_drive_file_public_access", &["file_name"]),
    ("update_drive_file", &["file_id"]),
    ("get_drive_shareable_link", &["file_id"]),
    ("manage_drive_access", &["file_id", "action"]),
    ("copy_drive_file", &["file_id"]),
    ("set_drive_file_permissions", &["file_id"]),
    ("create_form", &["title"]),
    ("get_form", &["form_id"]),
    ("set_publish_settings", &["form_id"]),
    ("get_form_response", &["form_id", "response_id"]),
    ("list_form_responses", &["form_id"]),
    ("batch_update_form", &["form_id", "requests"]),
    ("search_gmail_messages", &["query"]),
    ("get_gmail_message_content", &["message_id"]),
    ("get_gmail_messages_content_batch", &["message_ids"]),
    (
        "get_gmail_attachment_content",
        &["message_id", "attachment_id"],
    ),
    ("send_gmail_message", &[]),
    ("draft_gmail_message", &[]),
    ("get_gmail_thread_content", &["thread_id"]),
    ("get_gmail_threads_content_batch", &["thread_ids"]),
    ("list_gmail_labels", &[]),
    ("manage_gmail_label", &["action"]),
    ("list_gmail_filters", &[]),
    ("manage_gmail_filter", &["action"]),
    ("modify_gmail_message_labels", &["message_id"]),
    ("batch_modify_gmail_message_labels", &[]),
    ("search_custom", &["q"]),
    ("get_search_engine_info", &[]),
    ("list_spreadsheets", &[]),
    ("get_spreadsheet_info", &["spreadsheet_id"]),
    ("read_sheet_values", &["spreadsheet_id"]),
    ("modify_sheet_values", &["spreadsheet_id", "range_name"]),
    ("format_sheet_range", &["spreadsheet_id", "range_name"]),
    (
        "manage_conditional_formatting",
        &["spreadsheet_id", "action"],
    ),
    ("create_spreadsheet", &["title"]),
    ("create_sheet", &["spreadsheet_id"]),
    ("list_sheet_tables", &["spreadsheet_id"]),
    (
        "append_table_rows",
        &["spreadsheet_id", "table_id", "values"],
    ),
    (
        "manage_sheet_tab",
        &["spreadsheet_id", "sheet_name", "action"],
    ),
    ("resize_sheet_dimensions", &["spreadsheet_id"]),
    (
        "move_sheet_rows",
        &[
            "spreadsheet_id",
            "source_sheet",
            "start_row",
            "end_row",
            "destination_sheet",
        ],
    ),
    ("manage_named_range", &["spreadsheet_id", "action"]),
    ("list_spreadsheet_comments", &["spreadsheet_id"]),
    ("manage_spreadsheet_comment", &["spreadsheet_id", "action"]),
    ("create_presentation", &[]),
    ("get_presentation", &["presentation_id"]),
    (
        "batch_update_presentation",
        &["presentation_id", "requests"],
    ),
    ("get_page", &["presentation_id", "page_object_id"]),
    (
        "render_page_to_image",
        &["presentation_id", "page_object_id"],
    ),
    ("list_presentation_comments", &["presentation_id"]),
    (
        "manage_presentation_comment",
        &["presentation_id", "action"],
    ),
    ("list_task_lists", &[]),
    ("get_task_list", &["task_list_id"]),
    ("manage_task_list", &["action"]),
    ("list_tasks", &["task_list_id"]),
    ("get_task", &["task_list_id", "task_id"]),
    ("manage_task", &["action", "task_list_id"]),
];

pub fn tool_names() -> Vec<&'static str> {
    REQUIRED.iter().map(|(n, _)| *n).collect()
}

fn ok(text: String) -> Value {
    json_value!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": {"result": text},
        "isError": false,
    })
}

fn fail(text: String) -> Value {
    json_value!({"content": [{"type": "text", "text": text}], "isError": true})
}

pub fn call_tool(ctx: &mut Ctx, name: &str, args: &Value) -> Value {
    let Some((_, required)) = REQUIRED.iter().find(|(n, _)| *n == name) else {
        return fail(format!("Unknown tool: {name}"));
    };
    let a = if args.is_object() {
        args.clone()
    } else {
        json_value!({})
    };
    let missing: Vec<&&str> = required
        .iter()
        .filter(|r| a.get(**r).is_none_or(Value::is_null))
        .collect();
    if !missing.is_empty() {
        let mut msg = format!(
            "{} validation error{} for call[{name}]",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" }
        );
        for m in missing {
            msg.push_str(&format!(
                "\n{m}\n  Missing required argument [type=missing_argument, input_value={}, input_type=dict]\n    For further information visit https://errors.pydantic.dev/2.12/v/missing_argument",
                util::py_repr(&a)
            ));
        }
        return fail(format!("Error calling tool '{name}': {msg}"));
    }
    match dispatch(ctx, name, &a) {
        Ok(text) => ok(text),
        Err(e) => fail(format!("Error calling tool '{name}': {e}")),
    }
}

fn dispatch(ctx: &mut Ctx, name: &str, a: &Value) -> R {
    use other as o;
    match name {
        // Gmail
        "search_gmail_messages" => gmail::search_messages(ctx, a),
        "get_gmail_message_content" => gmail::get_message(ctx, a),
        "get_gmail_messages_content_batch" => gmail::get_messages_batch(ctx, a),
        "get_gmail_attachment_content" => gmail::get_attachment(ctx, a),
        "send_gmail_message" => gmail::send(ctx, a),
        "draft_gmail_message" => gmail::draft(ctx, a),
        "get_gmail_thread_content" => gmail::get_thread(ctx, a),
        "get_gmail_threads_content_batch" => gmail::get_threads_batch(ctx, a),
        "list_gmail_labels" => gmail::list_labels(ctx, a),
        "manage_gmail_label" => gmail::manage_label(ctx, a),
        "list_gmail_filters" => gmail::list_filters(ctx, a),
        "manage_gmail_filter" => gmail::manage_filter(ctx, a),
        "modify_gmail_message_labels" => gmail::modify_labels(ctx, a),
        "batch_modify_gmail_message_labels" => gmail::batch_modify(ctx, a),
        // Calendar
        "list_calendars" => calendar::list_calendars(ctx, a),
        "get_events" => calendar::get_events(ctx, a),
        "manage_event" => calendar::manage_event(ctx, a),
        "manage_out_of_office" => calendar::manage_status_event(ctx, a, "ooo"),
        "manage_focus_time" => calendar::manage_status_event(ctx, a, "focus"),
        "query_freebusy" => calendar::freebusy(ctx, a),
        "create_calendar" => calendar::create_calendar(ctx, a),
        // Drive
        "search_drive_files" => drive::search_files(ctx, a),
        "get_drive_file_content" => drive::get_content(ctx, a),
        "get_drive_file_download_url" => drive::download_url(ctx, a),
        "list_drive_items" => drive::list_items(ctx, a),
        "create_drive_folder" => drive::create_folder(ctx, a),
        "create_drive_file" => drive::create_file(ctx, a),
        "import_to_google_doc" => drive::import_as(ctx, a, drive::DOC, "Doc"),
        "import_to_google_slides" => drive::import_as(ctx, a, drive::SLIDES, "Slides"),
        "import_to_google_sheets" => drive::import_as(ctx, a, drive::SHEET, "Sheet"),
        "get_drive_file_permissions" => drive::permissions(ctx, a),
        "check_drive_file_public_access" => drive::public_access(ctx, a),
        "update_drive_file" => drive::update_file(ctx, a),
        "get_drive_shareable_link" => drive::shareable_link(ctx, a),
        "manage_drive_access" => drive::manage_access(ctx, a),
        "copy_drive_file" => drive::copy_file(ctx, a),
        "set_drive_file_permissions" => drive::set_permissions(ctx, a),
        // Docs
        "search_docs" => drive::search_docs(ctx, a),
        "get_doc_content" => drive::get_doc(ctx, a),
        "list_docs_in_folder" => drive::docs_in_folder(ctx, a),
        "create_doc" => drive::create_doc(ctx, a),
        "modify_doc_text" => drive::modify_doc_text(ctx, a),
        "find_and_replace_doc" => drive::find_replace(ctx, a),
        "insert_doc_elements" => drive::insert_elements(ctx, a),
        "insert_doc_image" => drive::insert_image(ctx, a),
        "update_doc_headers_footers" => drive::headers_footers(ctx, a),
        "batch_update_doc" => drive::batch_update_doc(ctx, a),
        "inspect_doc_structure" => drive::inspect_doc(ctx, a),
        "create_table_with_data" => drive::create_table(ctx, a),
        "debug_table_structure" => drive::debug_table(ctx, a),
        "export_doc_to_pdf" => drive::export_pdf(ctx, a),
        "update_paragraph_style" => drive::paragraph_style(ctx, a),
        "get_doc_as_markdown" => drive::doc_markdown(ctx, a),
        "manage_doc_tab" => drive::manage_tab(ctx, a),
        "list_document_comments" => o::list_comments(ctx, a, "document", "document_id"),
        "manage_document_comment" => o::manage_comment(ctx, a, "document", "document_id"),
        // Sheets
        "list_spreadsheets" => drive::list_spreadsheets(ctx, a),
        "get_spreadsheet_info" => drive::spreadsheet_info(ctx, a),
        "read_sheet_values" => drive::read_values(ctx, a),
        "modify_sheet_values" => drive::modify_values(ctx, a),
        "format_sheet_range" => drive::format_range(ctx, a),
        "manage_conditional_formatting" => drive::conditional_formatting(ctx, a),
        "create_spreadsheet" => drive::create_spreadsheet(ctx, a),
        "create_sheet" => drive::create_sheet(ctx, a),
        "list_sheet_tables" => drive::list_tables(ctx, a),
        "append_table_rows" => drive::append_rows(ctx, a),
        "manage_sheet_tab" => drive::manage_tab_sheet(ctx, a),
        "resize_sheet_dimensions" => drive::resize_dimensions(ctx, a),
        "move_sheet_rows" => drive::move_rows(ctx, a),
        "manage_named_range" => drive::named_range(ctx, a),
        "list_spreadsheet_comments" => o::list_comments(ctx, a, "spreadsheet", "spreadsheet_id"),
        "manage_spreadsheet_comment" => o::manage_comment(ctx, a, "spreadsheet", "spreadsheet_id"),
        // Slides
        "create_presentation" => o::create_presentation(ctx, a),
        "get_presentation" => o::get_presentation(ctx, a),
        "batch_update_presentation" => o::batch_update_presentation(ctx, a),
        "get_page" => o::get_page(ctx, a),
        "render_page_to_image" => o::render_page(ctx, a),
        "list_presentation_comments" => o::list_comments(ctx, a, "presentation", "presentation_id"),
        "manage_presentation_comment" => {
            o::manage_comment(ctx, a, "presentation", "presentation_id")
        }
        // Forms
        "create_form" => o::create_form(ctx, a),
        "get_form" => o::get_form(ctx, a),
        "set_publish_settings" => o::set_publish(ctx, a),
        "get_form_response" => o::get_form_response(ctx, a),
        "list_form_responses" => o::list_form_responses(ctx, a),
        "batch_update_form" => o::batch_update_form(ctx, a),
        // Tasks
        "list_task_lists" => o::list_task_lists(ctx, a),
        "get_task_list" => o::get_task_list(ctx, a),
        "manage_task_list" => o::manage_task_list(ctx, a),
        "list_tasks" => o::list_tasks(ctx, a),
        "get_task" => o::get_task(ctx, a),
        "manage_task" => o::manage_task(ctx, a),
        // Chat
        "list_spaces" => o::list_spaces(ctx, a),
        "get_messages" => o::get_chat_messages(ctx, a),
        "send_message" => o::send_chat_message(ctx, a),
        "search_messages" => o::search_chat(ctx, a),
        "create_reaction" => o::create_reaction(ctx, a),
        "download_chat_attachment" => o::chat_attachment(ctx, a),
        // Contacts
        "list_contacts" => o::list_contacts(ctx, a),
        "get_contact" => o::get_contact(ctx, a),
        "search_contacts" => o::search_contacts(ctx, a),
        "manage_contact" => o::manage_contact(ctx, a),
        "list_contact_groups" => o::list_contact_groups(ctx, a),
        "get_contact_group" => o::get_contact_group(ctx, a),
        "manage_contacts_batch" => o::manage_contacts_batch(ctx, a),
        "manage_contact_group" => o::manage_contact_group(ctx, a),
        // Custom Search
        "search_custom" => o::search_custom(ctx, a),
        "get_search_engine_info" => o::search_engine_info(ctx, a),
        // Apps Script
        "get_script_project" => o::get_script_project(ctx, a),
        "manage_script_content" => o::manage_script_content(ctx, a),
        "run_script_function" => o::run_script(ctx, a),
        "manage_deployment" => o::manage_deployment(ctx, a),
        "list_script_deployments" => o::list_deployments(ctx, a),
        "manage_script_project" => o::manage_script_project(ctx, a),
        "manage_script_version" => o::manage_script_version(ctx, a),
        "get_script_version" => o::get_script_version(ctx, a),
        "get_script_activity" => o::get_script_activity(ctx, a),
        "generate_trigger_code" => o::trigger_code(ctx, a),
        "manage_script_trigger" => o::manage_trigger(ctx, a),
        // Auth
        "start_google_auth" => o::start_auth(ctx, a),
        other => Err(format!("Unknown tool: {other}")),
    }
}
