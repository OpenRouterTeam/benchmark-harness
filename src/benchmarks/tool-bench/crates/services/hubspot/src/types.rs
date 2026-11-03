//! HubSpot's object model: object types (standard and custom), association type
//! definitions, default property sets and the built-in property definitions.

use mock_core::store::Store;
use serde_json::{Value, json};

/// Standard object types: (collection / plural name, objectTypeId, singular name).
pub const STANDARD_TYPES: &[(&str, &str, &str)] = &[
    ("contacts", "0-1", "contact"),
    ("companies", "0-2", "company"),
    ("deals", "0-3", "deal"),
    ("tickets", "0-5", "ticket"),
    ("products", "0-7", "product"),
    ("line_items", "0-8", "line_item"),
    ("quotes", "0-14", "quote"),
    ("communications", "0-18", "communication"),
    ("feedback_submissions", "0-19", "feedback_submission"),
    ("tasks", "0-27", "task"),
    ("notes", "0-46", "note"),
    ("meetings", "0-47", "meeting"),
    ("calls", "0-48", "call"),
    ("emails", "0-49", "email"),
    ("marketing_events", "0-54", "marketing_event"),
    ("subscriptions", "0-69", "subscription"),
    ("users", "0-115", "user"),
    ("postal_mail", "0-116", "postal_mail"),
    ("orders", "0-123", "order"),
    ("leads", "0-136", "lead"),
    ("services", "0-162", "service"),
    ("courses", "0-410", "course"),
    ("listings", "0-420", "listing"),
    ("appointments", "0-421", "appointment"),
];

/// Activity types that share HubSpot's engagement id space.
pub const ENGAGEMENT_TYPES: &[&str] = &["notes", "tasks", "calls", "emails", "meetings"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectType {
    /// Store collection, e.g. `deals` or a custom object's name.
    pub name: String,
    pub type_id: String,
    pub singular: String,
    pub custom: bool,
}

impl ObjectType {
    pub fn is_engagement(&self) -> bool {
        ENGAGEMENT_TYPES.contains(&self.name.as_str())
    }

    /// The property holding the creation time.
    pub fn created_key(&self) -> &'static str {
        if matches!(
            self.name.as_str(),
            "contacts" | "companies" | "deals" | "tickets"
        ) {
            "createdate"
        } else {
            "hs_createdate"
        }
    }

    /// The property holding the last-modified time.
    pub fn modified_key(&self) -> &'static str {
        if self.name == "contacts" {
            "lastmodifieddate"
        } else {
            "hs_lastmodifieddate"
        }
    }

    /// Upper-case label used in some error messages (`CONTACT`, `DEAL`, ...).
    pub fn upper(&self) -> String {
        self.singular.to_uppercase()
    }
}

/// Resolves a path object type: plural, singular, `0-x` id, or a custom object's
/// `objectTypeId`, `fullyQualifiedName` or name.
pub fn resolve(store: &Store, raw: &str) -> Option<ObjectType> {
    let lower = raw.to_lowercase();
    for (name, id, singular) in STANDARD_TYPES {
        if lower == *name || lower == *singular || lower == *id {
            return Some(ObjectType {
                name: (*name).to_owned(),
                type_id: (*id).to_owned(),
                singular: (*singular).to_owned(),
                custom: false,
            });
        }
    }
    let schemas = store.collection("schemas")?;
    schemas.values().find_map(|s| {
        let name = s["name"].as_str()?;
        let fqn = s["fullyQualifiedName"].as_str().unwrap_or_default();
        let id = s["objectTypeId"].as_str().unwrap_or_default();
        let short = fqn.split_once('_').map_or("", |(_, n)| n);
        let plural = s["labels"]["plural"].as_str().unwrap_or_default();
        let hit = [name, fqn, id, short, plural]
            .iter()
            .any(|c| !c.is_empty() && c.to_lowercase() == lower);
        hit.then(|| ObjectType {
            name: name.to_owned(),
            type_id: id.to_owned(),
            singular: name.to_owned(),
            custom: true,
        })
    })
}

/// Properties returned when a request names none.
pub fn default_properties(store: &Store, ty: &ObjectType) -> Vec<String> {
    let names: &[&str] = match ty.name.as_str() {
        "contacts" => &[
            "createdate",
            "email",
            "firstname",
            "hs_object_id",
            "lastmodifieddate",
            "lastname",
        ],
        "companies" => &[
            "createdate",
            "domain",
            "hs_lastmodifieddate",
            "hs_object_id",
            "name",
        ],
        "deals" => &[
            "amount",
            "closedate",
            "createdate",
            "dealname",
            "dealstage",
            "hs_lastmodifieddate",
            "hs_object_id",
            "pipeline",
        ],
        "tickets" => &[
            "content",
            "createdate",
            "hs_lastmodifieddate",
            "hs_object_id",
            "hs_pipeline",
            "hs_pipeline_stage",
            "hs_ticket_category",
            "hs_ticket_priority",
            "subject",
        ],
        "products" => &[
            "createdate",
            "description",
            "hs_lastmodifieddate",
            "hs_object_id",
            "name",
            "price",
        ],
        "line_items" => &[
            "createdate",
            "hs_lastmodifieddate",
            "hs_object_id",
            "hs_product_id",
            "name",
            "price",
            "quantity",
        ],
        _ => &["hs_createdate", "hs_lastmodifieddate", "hs_object_id"],
    };
    let mut out: Vec<String> = names.iter().map(|s| (*s).to_owned()).collect();
    if ty.custom
        && let Some(primary) = schema(store, ty).and_then(|s| s["primaryDisplayProperty"].as_str())
    {
        out.push(primary.to_owned());
    }
    out
}

/// Properties free-text `query` searches.
pub fn searchable_properties(store: &Store, ty: &ObjectType) -> Vec<String> {
    let names: &[&str] = match ty.name.as_str() {
        "contacts" => &[
            "firstname",
            "lastname",
            "email",
            "hs_additional_emails",
            "phone",
            "mobilephone",
            "company",
        ],
        "companies" => &["name", "domain", "website", "phone"],
        "deals" => &[
            "dealname",
            "pipeline",
            "dealstage",
            "description",
            "dealtype",
        ],
        "tickets" => &[
            "subject",
            "content",
            "hs_pipeline_stage",
            "hs_ticket_category",
        ],
        "notes" => &["hs_note_body"],
        "tasks" => &["hs_task_subject", "hs_task_body"],
        "calls" => &["hs_call_title", "hs_call_body"],
        "emails" => &["hs_email_subject", "hs_email_text"],
        "meetings" => &["hs_meeting_title", "hs_meeting_body"],
        "products" => &["name", "description", "hs_sku"],
        "line_items" => &["name", "description"],
        _ => &["name"],
    };
    let mut out: Vec<String> = names.iter().map(|s| (*s).to_owned()).collect();
    if ty.custom
        && let Some(list) = schema(store, ty).and_then(|s| s["searchableProperties"].as_array())
    {
        out = list
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
    }
    out
}

pub fn schema<'a>(store: &'a Store, ty: &ObjectType) -> Option<&'a Value> {
    store
        .collection("schemas")?
        .values()
        .find(|s| s["objectTypeId"].as_str() == Some(ty.type_id.as_str()))
}

// ---------------------------------------------------------------------------------------
// Association definitions
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct AssocDef {
    pub from: String,
    pub to: String,
    pub category: String,
    pub type_id: u64,
    pub label: Option<String>,
    pub inverse: u64,
    /// The v3 `associations` type name, e.g. `contact_to_company`.
    pub v3_name: String,
}

/// `(from, to, typeId, label, inverse typeId, v3 name)`
type BuiltinAssoc = (
    &'static str,
    &'static str,
    u64,
    Option<&'static str>,
    u64,
    &'static str,
);

/// HubSpot-defined association types.
const HUBSPOT_ASSOCIATIONS: &[BuiltinAssoc] = &[
    (
        "contacts",
        "companies",
        279,
        None,
        280,
        "contact_to_company_unlabeled",
    ),
    (
        "contacts",
        "companies",
        1,
        Some("Primary"),
        2,
        "contact_to_company",
    ),
    (
        "companies",
        "contacts",
        280,
        None,
        279,
        "company_to_contact_unlabeled",
    ),
    (
        "companies",
        "contacts",
        2,
        Some("Primary"),
        1,
        "company_to_contact",
    ),
    ("deals", "contacts", 3, None, 4, "deal_to_contact"),
    ("contacts", "deals", 4, None, 3, "contact_to_deal"),
    (
        "deals",
        "companies",
        341,
        None,
        342,
        "deal_to_company_unlabeled",
    ),
    (
        "deals",
        "companies",
        5,
        Some("Primary"),
        6,
        "deal_to_company",
    ),
    (
        "companies",
        "deals",
        342,
        None,
        341,
        "company_to_deal_unlabeled",
    ),
    (
        "companies",
        "deals",
        6,
        Some("Primary"),
        5,
        "company_to_deal",
    ),
    ("tickets", "contacts", 16, None, 15, "ticket_to_contact"),
    ("contacts", "tickets", 15, None, 16, "contact_to_ticket"),
    (
        "tickets",
        "companies",
        339,
        None,
        340,
        "ticket_to_company_unlabeled",
    ),
    (
        "tickets",
        "companies",
        26,
        Some("Primary"),
        25,
        "ticket_to_company",
    ),
    (
        "companies",
        "tickets",
        340,
        None,
        339,
        "company_to_ticket_unlabeled",
    ),
    (
        "companies",
        "tickets",
        25,
        Some("Primary"),
        26,
        "company_to_ticket",
    ),
    ("deals", "tickets", 27, None, 28, "deal_to_ticket"),
    ("tickets", "deals", 28, None, 27, "ticket_to_deal"),
    ("deals", "line_items", 19, None, 20, "deal_to_line_item"),
    ("line_items", "deals", 20, None, 19, "line_item_to_deal"),
    ("deals", "quotes", 63, None, 64, "deal_to_quote"),
    ("quotes", "deals", 64, None, 63, "quote_to_deal"),
    ("contacts", "contacts", 449, None, 449, "contact_to_contact"),
    (
        "companies",
        "companies",
        450,
        None,
        450,
        "company_to_company",
    ),
    (
        "companies",
        "companies",
        13,
        Some("Parent Company"),
        14,
        "parent_to_child_company",
    ),
    (
        "companies",
        "companies",
        14,
        Some("Child Company"),
        13,
        "child_to_parent_company",
    ),
    ("contacts", "notes", 201, None, 202, "contact_to_note"),
    ("notes", "contacts", 202, None, 201, "note_to_contact"),
    ("companies", "notes", 189, None, 190, "company_to_note"),
    ("notes", "companies", 190, None, 189, "note_to_company"),
    ("deals", "notes", 213, None, 214, "deal_to_note"),
    ("notes", "deals", 214, None, 213, "note_to_deal"),
    ("tickets", "notes", 227, None, 228, "ticket_to_note"),
    ("notes", "tickets", 228, None, 227, "note_to_ticket"),
    ("contacts", "tasks", 203, None, 204, "contact_to_task"),
    ("tasks", "contacts", 204, None, 203, "task_to_contact"),
    ("companies", "tasks", 191, None, 192, "company_to_task"),
    ("tasks", "companies", 192, None, 191, "task_to_company"),
    ("deals", "tasks", 215, None, 216, "deal_to_task"),
    ("tasks", "deals", 216, None, 215, "task_to_deal"),
    ("tickets", "tasks", 229, None, 230, "ticket_to_task"),
    ("tasks", "tickets", 230, None, 229, "task_to_ticket"),
    ("contacts", "calls", 193, None, 194, "contact_to_call"),
    ("calls", "contacts", 194, None, 193, "call_to_contact"),
    ("companies", "calls", 181, None, 182, "company_to_call"),
    ("calls", "companies", 182, None, 181, "call_to_company"),
    ("deals", "calls", 205, None, 206, "deal_to_call"),
    ("calls", "deals", 206, None, 205, "call_to_deal"),
    ("tickets", "calls", 219, None, 220, "ticket_to_call"),
    ("calls", "tickets", 220, None, 219, "call_to_ticket"),
    ("contacts", "emails", 197, None, 198, "contact_to_email"),
    ("emails", "contacts", 198, None, 197, "email_to_contact"),
    ("companies", "emails", 185, None, 186, "company_to_email"),
    ("emails", "companies", 186, None, 185, "email_to_company"),
    ("deals", "emails", 209, None, 210, "deal_to_email"),
    ("emails", "deals", 210, None, 209, "email_to_deal"),
    ("tickets", "emails", 223, None, 224, "ticket_to_email"),
    ("emails", "tickets", 224, None, 223, "email_to_ticket"),
    ("contacts", "meetings", 199, None, 200, "contact_to_meeting"),
    ("meetings", "contacts", 200, None, 199, "meeting_to_contact"),
    (
        "companies",
        "meetings",
        187,
        None,
        188,
        "company_to_meeting",
    ),
    (
        "meetings",
        "companies",
        188,
        None,
        187,
        "meeting_to_company",
    ),
    ("deals", "meetings", 211, None, 212, "deal_to_meeting"),
    ("meetings", "deals", 212, None, 211, "meeting_to_deal"),
    ("tickets", "meetings", 225, None, 226, "ticket_to_meeting"),
    ("meetings", "tickets", 226, None, 225, "meeting_to_ticket"),
];

/// Every association type from `from` to `to`: HubSpot-defined, plus user-defined labels
/// (`association_labels` collection) and custom-object schema associations.
pub fn association_defs(store: &Store, from: &ObjectType, to: &ObjectType) -> Vec<AssocDef> {
    let mut defs: Vec<AssocDef> = HUBSPOT_ASSOCIATIONS
        .iter()
        .filter(|(f, t, ..)| *f == from.name && *t == to.name)
        .map(|(f, t, id, label, inverse, v3)| AssocDef {
            from: (*f).to_owned(),
            to: (*t).to_owned(),
            category: "HUBSPOT_DEFINED".to_owned(),
            type_id: *id,
            label: label.map(str::to_owned),
            inverse: *inverse,
            v3_name: (*v3).to_owned(),
        })
        .collect();
    if let Some(labels) = store.collection("association_labels") {
        for l in labels.values() {
            let (Some(f), Some(t), Some(id)) = (
                l["fromObjectType"].as_str(),
                l["toObjectType"].as_str(),
                l["typeId"].as_u64(),
            ) else {
                continue;
            };
            let label = l["label"].as_str().map(str::to_owned);
            let inverse = l["inverseTypeId"].as_u64().unwrap_or(id);
            let category = l["category"].as_str().unwrap_or("USER_DEFINED").to_owned();
            if f == from.name && t == to.name {
                defs.push(AssocDef {
                    from: from.name.clone(),
                    to: to.name.clone(),
                    category: category.clone(),
                    type_id: id,
                    label: label.clone(),
                    inverse,
                    v3_name: assoc_name(from, to, label.as_deref()),
                });
            }
            if t == from.name && f == to.name && (f != t || inverse != id) {
                defs.push(AssocDef {
                    from: from.name.clone(),
                    to: to.name.clone(),
                    category,
                    type_id: inverse,
                    label: l["inverseLabel"].as_str().map(str::to_owned).or(label),
                    inverse: id,
                    v3_name: assoc_name(from, to, None),
                });
            }
        }
    }
    if let Some(schemas) = store.collection("schemas") {
        for s in schemas.values() {
            for a in s["associations"].as_array().into_iter().flatten() {
                let (Some(f), Some(t)) =
                    (a["fromObjectTypeId"].as_str(), a["toObjectTypeId"].as_str())
                else {
                    continue;
                };
                let Some(id) = a["id"]
                    .as_str()
                    .and_then(|v| v.parse::<u64>().ok())
                    .or_else(|| a["id"].as_u64())
                else {
                    continue;
                };
                let inverse = a["inverseId"]
                    .as_str()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(id);
                let push = |defs: &mut Vec<AssocDef>, type_id, inverse| {
                    if !defs.iter().any(|d| d.type_id == type_id) {
                        defs.push(AssocDef {
                            from: from.name.clone(),
                            to: to.name.clone(),
                            category: "USER_DEFINED".to_owned(),
                            type_id,
                            label: None,
                            inverse,
                            v3_name: assoc_name(from, to, None),
                        });
                    }
                };
                if f == from.type_id && t == to.type_id {
                    push(&mut defs, id, inverse);
                } else if t == from.type_id && f == to.type_id {
                    push(&mut defs, inverse, id);
                }
            }
        }
    }
    defs
}

fn assoc_name(from: &ObjectType, to: &ObjectType, label: Option<&str>) -> String {
    let base = format!("{}_to_{}", from.singular, to.singular);
    match label {
        Some(l) => format!(
            "{base}_{}",
            l.to_lowercase()
                .replace(|c: char| !c.is_alphanumeric(), "_")
        ),
        None => base,
    }
}

// ---------------------------------------------------------------------------------------
// Built-in property definitions
// ---------------------------------------------------------------------------------------

type Prop = (&'static str, &'static str, &'static str, &'static str);

const CONTACT_PROPS: &[Prop] = &[
    ("email", "Email", "string", "text"),
    ("firstname", "First Name", "string", "text"),
    ("lastname", "Last Name", "string", "text"),
    ("phone", "Phone Number", "string", "phonenumber"),
    (
        "mobilephone",
        "Mobile Phone Number",
        "string",
        "phonenumber",
    ),
    ("company", "Company Name", "string", "text"),
    ("jobtitle", "Job Title", "string", "text"),
    ("website", "Website URL", "string", "text"),
    ("address", "Street Address", "string", "text"),
    ("city", "City", "string", "text"),
    ("state", "State/Region", "string", "text"),
    ("zip", "Postal Code", "string", "text"),
    ("country", "Country/Region", "string", "text"),
    ("industry", "Industry", "string", "text"),
    (
        "hs_additional_emails",
        "Additional email addresses",
        "enumeration",
        "checkbox",
    ),
    ("lifecyclestage", "Lifecycle Stage", "enumeration", "radio"),
    ("hs_lead_status", "Lead Status", "enumeration", "radio"),
    ("hubspot_owner_id", "Contact owner", "enumeration", "select"),
    (
        "associatedcompanyid",
        "Primary Associated Company ID",
        "number",
        "number",
    ),
    (
        "notes_last_updated",
        "Last Activity Date",
        "datetime",
        "date",
    ),
    ("createdate", "Create Date", "datetime", "date"),
    ("lastmodifieddate", "Last Modified Date", "datetime", "date"),
    ("hs_object_id", "Record ID", "number", "number"),
];

const COMPANY_PROPS: &[Prop] = &[
    ("name", "Company name", "string", "text"),
    ("domain", "Company Domain Name", "string", "text"),
    ("website", "Website URL", "string", "text"),
    ("phone", "Phone Number", "string", "phonenumber"),
    ("industry", "Industry", "string", "text"),
    ("description", "Description", "string", "textarea"),
    ("address", "Street Address", "string", "text"),
    ("city", "City", "string", "text"),
    ("state", "State/Region", "string", "text"),
    ("zip", "Postal Code", "string", "text"),
    ("country", "Country/Region", "string", "text"),
    (
        "numberofemployees",
        "Number of Employees",
        "number",
        "number",
    ),
    ("annualrevenue", "Annual Revenue", "number", "number"),
    ("type", "Type", "enumeration", "select"),
    ("lifecyclestage", "Lifecycle Stage", "enumeration", "radio"),
    ("hubspot_owner_id", "Company owner", "enumeration", "select"),
    (
        "notes_last_updated",
        "Last Activity Date",
        "datetime",
        "date",
    ),
    ("createdate", "Create Date", "datetime", "date"),
    (
        "hs_lastmodifieddate",
        "Last Modified Date",
        "datetime",
        "date",
    ),
    ("hs_object_id", "Record ID", "number", "number"),
];

const DEAL_PROPS: &[Prop] = &[
    ("dealname", "Deal Name", "string", "text"),
    ("amount", "Amount", "number", "number"),
    ("dealstage", "Deal Stage", "enumeration", "radio"),
    ("pipeline", "Pipeline", "enumeration", "select"),
    ("closedate", "Close Date", "datetime", "date"),
    ("dealtype", "Deal Type", "enumeration", "radio"),
    ("description", "Deal Description", "string", "textarea"),
    ("hubspot_owner_id", "Deal owner", "enumeration", "select"),
    ("hs_priority", "Priority", "enumeration", "select"),
    ("hs_next_step", "Next step", "string", "textarea"),
    (
        "closed_lost_reason",
        "Closed Lost Reason",
        "string",
        "textarea",
    ),
    (
        "closed_won_reason",
        "Closed Won Reason",
        "string",
        "textarea",
    ),
    (
        "hs_forecast_probability",
        "Forecast probability",
        "number",
        "number",
    ),
    (
        "notes_last_updated",
        "Last Activity Date",
        "datetime",
        "date",
    ),
    ("createdate", "Create Date", "datetime", "date"),
    (
        "hs_lastmodifieddate",
        "Last Modified Date",
        "datetime",
        "date",
    ),
    ("hs_object_id", "Record ID", "number", "number"),
];

const TICKET_PROPS: &[Prop] = &[
    ("subject", "Ticket name", "string", "text"),
    ("content", "Ticket description", "string", "textarea"),
    ("hs_pipeline", "Pipeline", "enumeration", "select"),
    (
        "hs_pipeline_stage",
        "Ticket status",
        "enumeration",
        "select",
    ),
    ("hs_ticket_priority", "Priority", "enumeration", "select"),
    ("hs_ticket_category", "Category", "enumeration", "checkbox"),
    ("source_type", "Source", "enumeration", "select"),
    ("hubspot_owner_id", "Ticket owner", "enumeration", "select"),
    ("closed_date", "Close date", "datetime", "date"),
    ("createdate", "Create date", "datetime", "date"),
    (
        "hs_lastmodifieddate",
        "Last modified date",
        "datetime",
        "date",
    ),
    ("hs_object_id", "Ticket ID", "number", "number"),
];

const NOTE_PROPS: &[Prop] = &[
    ("hs_note_body", "Note body", "string", "html"),
    ("hs_timestamp", "Activity date", "datetime", "date"),
    (
        "hubspot_owner_id",
        "Activity assigned to",
        "enumeration",
        "select",
    ),
    (
        "hs_attachment_ids",
        "Attached file IDs",
        "enumeration",
        "checkbox",
    ),
];

const TASK_PROPS: &[Prop] = &[
    ("hs_task_subject", "Task Title", "string", "text"),
    ("hs_task_body", "Notes", "string", "html"),
    ("hs_task_status", "Task Status", "enumeration", "select"),
    ("hs_task_priority", "Priority", "enumeration", "select"),
    ("hs_task_type", "Task Type", "enumeration", "select"),
    (
        "hs_task_for_object_type",
        "Task for object type",
        "string",
        "text",
    ),
    ("hs_timestamp", "Due date", "datetime", "date"),
    ("hubspot_owner_id", "Assigned to", "enumeration", "select"),
];

const CALL_PROPS: &[Prop] = &[
    ("hs_call_title", "Call Title", "string", "text"),
    ("hs_call_body", "Call notes", "string", "html"),
    ("hs_call_status", "Call status", "enumeration", "select"),
    (
        "hs_call_direction",
        "Call direction",
        "enumeration",
        "select",
    ),
    ("hs_call_duration", "Call duration", "number", "number"),
    ("hs_call_to_number", "To Number", "string", "text"),
    ("hs_call_from_number", "From Number", "string", "text"),
    ("hs_call_disposition", "Call outcome", "string", "text"),
    ("hs_timestamp", "Activity date", "datetime", "date"),
    (
        "hubspot_owner_id",
        "Activity assigned to",
        "enumeration",
        "select",
    ),
];

const EMAIL_PROPS: &[Prop] = &[
    ("hs_email_subject", "Email subject", "string", "text"),
    ("hs_email_text", "Email body", "string", "textarea"),
    ("hs_email_html", "Email HTML", "string", "html"),
    (
        "hs_email_direction",
        "Email direction",
        "enumeration",
        "select",
    ),
    (
        "hs_email_status",
        "Email send status",
        "enumeration",
        "select",
    ),
    ("hs_timestamp", "Activity date", "datetime", "date"),
    (
        "hubspot_owner_id",
        "Activity assigned to",
        "enumeration",
        "select",
    ),
];

const MEETING_PROPS: &[Prop] = &[
    ("hs_meeting_title", "Meeting name", "string", "text"),
    ("hs_meeting_body", "Meeting description", "string", "html"),
    ("hs_meeting_start_time", "Start Time", "datetime", "date"),
    ("hs_meeting_end_time", "End Time", "datetime", "date"),
    ("hs_meeting_location", "Location", "string", "text"),
    (
        "hs_meeting_outcome",
        "Meeting outcome",
        "enumeration",
        "select",
    ),
    ("hs_timestamp", "Activity date", "datetime", "date"),
    (
        "hubspot_owner_id",
        "Activity assigned to",
        "enumeration",
        "select",
    ),
];

const PRODUCT_PROPS: &[Prop] = &[
    ("name", "Name", "string", "text"),
    ("description", "Description", "string", "textarea"),
    ("price", "Unit price", "number", "number"),
    ("hs_sku", "SKU", "string", "text"),
    ("hs_cost_of_goods_sold", "Unit cost", "number", "number"),
    ("hs_recurring_billing_period", "Term", "string", "text"),
    ("createdate", "Create Date", "datetime", "date"),
    (
        "hs_lastmodifieddate",
        "Last Modified Date",
        "datetime",
        "date",
    ),
    ("hs_object_id", "Record ID", "number", "number"),
];

const LINE_ITEM_PROPS: &[Prop] = &[
    ("name", "Name", "string", "text"),
    ("description", "Description", "string", "textarea"),
    ("price", "Unit price", "number", "number"),
    ("quantity", "Quantity", "number", "number"),
    ("amount", "Net price", "number", "number"),
    ("hs_product_id", "Product ID", "number", "number"),
    ("hs_sku", "SKU", "string", "text"),
    ("createdate", "Create Date", "datetime", "date"),
    (
        "hs_lastmodifieddate",
        "Last Modified Date",
        "datetime",
        "date",
    ),
    ("hs_object_id", "Record ID", "number", "number"),
];

const COMMON_PROPS: &[Prop] = &[
    (
        "hs_createdate",
        "Object create date/time",
        "datetime",
        "date",
    ),
    (
        "hs_lastmodifieddate",
        "Object last modified date/time",
        "datetime",
        "date",
    ),
    ("hs_object_id", "Record ID", "number", "number"),
];

const DEFINED_AT: &str = "2019-08-06T02:41:52.513Z";

fn builtin_table(ty: &str) -> (&'static [Prop], &'static str) {
    match ty {
        "contacts" => (CONTACT_PROPS, "contactinformation"),
        "companies" => (COMPANY_PROPS, "companyinformation"),
        "deals" => (DEAL_PROPS, "dealinformation"),
        "tickets" => (TICKET_PROPS, "ticketinformation"),
        "notes" => (NOTE_PROPS, "engagement"),
        "tasks" => (TASK_PROPS, "task"),
        "calls" => (CALL_PROPS, "call"),
        "emails" => (EMAIL_PROPS, "email"),
        "meetings" => (MEETING_PROPS, "meeting"),
        "products" => (PRODUCT_PROPS, "productinformation"),
        "line_items" => (LINE_ITEM_PROPS, "lineiteminformation"),
        _ => (&[], "information"),
    }
}

fn builtin_options(ty: &str, name: &str) -> Vec<(&'static str, &'static str)> {
    match (ty, name) {
        ("deals", "dealstage") => vec![
            ("appointmentscheduled", "Appointment Scheduled"),
            ("qualifiedtobuy", "Qualified To Buy"),
            ("presentationscheduled", "Presentation Scheduled"),
            ("decisionmakerboughtin", "Decision Maker Bought-In"),
            ("contractsent", "Contract Sent"),
            ("closedwon", "Closed Won"),
            ("closedlost", "Closed Lost"),
        ],
        ("deals", "pipeline") => vec![("default", "Sales Pipeline")],
        ("deals", "dealtype") => vec![
            ("newbusiness", "New Business"),
            ("existingbusiness", "Existing Business"),
        ],
        ("deals", "hs_priority") => vec![("low", "Low"), ("medium", "Medium"), ("high", "High")],
        ("contacts" | "companies", "lifecyclestage") => vec![
            ("subscriber", "Subscriber"),
            ("lead", "Lead"),
            ("marketingqualifiedlead", "Marketing Qualified Lead"),
            ("salesqualifiedlead", "Sales Qualified Lead"),
            ("opportunity", "Opportunity"),
            ("customer", "Customer"),
            ("evangelist", "Evangelist"),
            ("other", "Other"),
        ],
        ("contacts", "hs_lead_status") => vec![
            ("NEW", "New"),
            ("OPEN", "Open"),
            ("IN_PROGRESS", "In Progress"),
            ("OPEN_DEAL", "Open Deal"),
            ("UNQUALIFIED", "Unqualified"),
            ("ATTEMPTED_TO_CONTACT", "Attempted to Contact"),
            ("CONNECTED", "Connected"),
            ("BAD_TIMING", "Bad Timing"),
        ],
        ("companies", "type") => vec![
            ("PROSPECT", "Prospect"),
            ("PARTNER", "Partner"),
            ("RESELLER", "Reseller"),
            ("VENDOR", "Vendor"),
            ("OTHER", "Other"),
        ],
        ("tickets", "hs_pipeline") => vec![("0", "Support Pipeline")],
        ("tickets", "hs_pipeline_stage") => vec![
            ("1", "New"),
            ("2", "Waiting on contact"),
            ("3", "Waiting on us"),
            ("4", "Closed"),
        ],
        ("tickets", "hs_ticket_priority") => vec![
            ("LOW", "Low"),
            ("MEDIUM", "Medium"),
            ("HIGH", "High"),
            ("URGENT", "Urgent"),
        ],
        ("tickets", "hs_ticket_category") => vec![
            ("PRODUCT_ISSUE", "Product Issue"),
            ("BILLING_ISSUE", "Billing Issue"),
            ("FEATURE_REQUEST", "Feature Request"),
            ("GENERAL_INQUIRY", "General Inquiry"),
        ],
        ("tickets", "source_type") => vec![
            ("CHAT", "Chat"),
            ("EMAIL", "Email"),
            ("FORM", "Form"),
            ("PHONE", "Phone"),
        ],
        ("tasks", "hs_task_status") => vec![
            ("NOT_STARTED", "Not Started"),
            ("IN_PROGRESS", "In Progress"),
            ("WAITING", "Waiting"),
            ("COMPLETED", "Completed"),
            ("DEFERRED", "Deferred"),
        ],
        ("tasks", "hs_task_priority") => {
            vec![
                ("NONE", "None"),
                ("LOW", "Low"),
                ("MEDIUM", "Medium"),
                ("HIGH", "High"),
            ]
        }
        ("tasks", "hs_task_type") => vec![("CALL", "Call"), ("EMAIL", "Email"), ("TODO", "To-do")],
        ("calls", "hs_call_status") => vec![
            ("BUSY", "Busy"),
            ("CALLING_CRM_USER", "Calling CRM user"),
            ("CANCELED", "Canceled"),
            ("COMPLETED", "Completed"),
            ("CONNECTING", "Connecting"),
            ("FAILED", "Failed"),
            ("IN_PROGRESS", "In progress"),
            ("NO_ANSWER", "No answer"),
            ("QUEUED", "Queued"),
            ("RINGING", "Ringing"),
        ],
        ("calls" | "emails", "hs_call_direction" | "hs_email_direction") => vec![
            ("INBOUND", "Inbound"),
            ("OUTBOUND", "Outbound"),
            ("EMAIL", "Email"),
            ("INCOMING_EMAIL", "Incoming email"),
            ("FORWARDED_EMAIL", "Forwarded email"),
        ],
        ("emails", "hs_email_status") => vec![
            ("BOUNCED", "Bounced"),
            ("FAILED", "Failed"),
            ("SCHEDULED", "Scheduled"),
            ("SENDING", "Sending"),
            ("SENT", "Sent"),
        ],
        ("meetings", "hs_meeting_outcome") => vec![
            ("SCHEDULED", "Scheduled"),
            ("COMPLETED", "Completed"),
            ("RESCHEDULED", "Rescheduled"),
            ("NO_SHOW", "No show"),
            ("CANCELED", "Canceled"),
        ],
        _ => Vec::new(),
    }
}

/// Built-in definitions for an object type, in HubSpot's full property shape.
pub fn builtin_properties(ty: &ObjectType) -> Vec<Value> {
    let (table, group) = builtin_table(&ty.name);
    let common: &[Prop] = if table.iter().any(|p| p.0 == "hs_object_id") {
        &[]
    } else {
        COMMON_PROPS
    };
    table
        .iter()
        .chain(common)
        .enumerate()
        .map(|(i, (name, label, kind, field))| {
            let options: Vec<Value> = builtin_options(&ty.name, name)
                .into_iter()
                .enumerate()
                .map(|(order, (value, label))| {
                    json!({"label": label, "value": value, "description": "", "displayOrder": order, "hidden": false})
                })
                .collect();
            let read_only = matches!(
                *name,
                "createdate" | "lastmodifieddate" | "hs_lastmodifieddate" | "hs_createdate" | "hs_object_id" | "notes_last_updated"
            );
            json!({
                "updatedAt": DEFINED_AT,
                "createdAt": DEFINED_AT,
                "name": name,
                "label": label,
                "type": kind,
                "fieldType": field,
                "description": "",
                "groupName": group,
                "options": options,
                "displayOrder": i,
                "calculated": false,
                "externalOptions": *name == "hubspot_owner_id",
                "hasUniqueValue": *name == "hs_object_id",
                "hidden": false,
                "hubspotDefined": true,
                "modificationMetadata": {
                    "archivable": false,
                    "readOnlyDefinition": true,
                    "readOnlyValue": read_only
                },
                "formField": !read_only && ty.name == "contacts",
                "dataSensitivity": "non_sensitive"
            })
        })
        .collect()
}

/// Every property definition for a type: built-ins, then custom-object schema properties,
/// then dataset/created definitions (`properties` collection, key `<type>/<name>`), later
/// sources replacing earlier ones with the same name.
pub fn property_definitions(store: &Store, ty: &ObjectType) -> Vec<Value> {
    let mut defs = builtin_properties(ty);
    let upsert = |defs: &mut Vec<Value>, p: Value| {
        let Some(name) = p["name"].as_str().map(str::to_owned) else {
            return;
        };
        match defs.iter_mut().find(|d| d["name"] == name.as_str()) {
            Some(existing) => *existing = p,
            None => defs.push(p),
        }
    };
    if ty.custom
        && let Some(props) = schema(store, ty).and_then(|s| s["properties"].as_array())
    {
        for p in props {
            upsert(&mut defs, complete_property(p.clone()));
        }
    }
    if let Some(stored) = store.collection("properties") {
        let prefix = format!("{}/", ty.name);
        for (key, p) in stored.iter() {
            if key.starts_with(&prefix) {
                upsert(&mut defs, complete_property(p.clone()));
            }
        }
    }
    defs
}

pub fn property_definition(store: &Store, ty: &ObjectType, name: &str) -> Option<Value> {
    property_definitions(store, ty)
        .into_iter()
        .find(|p| p["name"] == name)
}

/// Fills the fields HubSpot always returns on a property definition.
pub fn complete_property(mut p: Value) -> Value {
    let Some(obj) = p.as_object_mut() else {
        return p;
    };
    let defaults = [
        ("description", json!("")),
        ("options", json!([])),
        ("displayOrder", json!(-1)),
        ("calculated", json!(false)),
        ("externalOptions", json!(false)),
        ("hasUniqueValue", json!(false)),
        ("hidden", json!(false)),
        (
            "modificationMetadata",
            json!({"archivable": true, "readOnlyDefinition": false, "readOnlyValue": false}),
        ),
        ("formField", json!(false)),
    ];
    for (k, v) in defaults {
        obj.entry(k).or_insert(v);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_standard_types_by_any_name() {
        let store = Store::default();
        for raw in ["deals", "deal", "0-3", "DEALS"] {
            assert_eq!(resolve(&store, raw).unwrap().name, "deals");
        }
        assert!(resolve(&store, "dealz").is_none());
    }

    #[test]
    fn resolves_custom_types_from_schemas() {
        let mut store = Store::default();
        store.collection_mut("schemas").insert(
            "2-901",
            json!({"objectTypeId": "2-901", "name": "pets", "fullyQualifiedName": "p24681357_pets", "labels": {"singular": "Pet", "plural": "Pets"}}),
        );
        for raw in ["pets", "2-901", "p24681357_pets"] {
            let ty = resolve(&store, raw).unwrap();
            assert_eq!((ty.name.as_str(), ty.custom), ("pets", true));
        }
    }

    #[test]
    fn association_defs_include_primary_and_inverse() {
        let store = Store::default();
        let c = resolve(&store, "contacts").unwrap();
        let co = resolve(&store, "companies").unwrap();
        let defs = association_defs(&store, &c, &co);
        assert_eq!(defs.iter().map(|d| d.type_id).collect::<Vec<_>>(), [279, 1]);
        assert_eq!(defs[1].inverse, 2);
    }
}
