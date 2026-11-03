//! sObject metadata: the standard objects the mock models, custom objects and fields created
//! through the Metadata API, and the describe payloads built from them.

use std::sync::OnceLock;

use mock_core::store::Store;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Id,
    Reference,
    String,
    TextArea,
    Picklist,
    MultiPicklist,
    Combobox,
    Boolean,
    Currency,
    Double,
    Percent,
    Int,
    Date,
    DateTime,
    Email,
    Phone,
    Url,
}

impl Ty {
    pub fn api_name(self) -> &'static str {
        match self {
            Ty::Id => "id",
            Ty::Reference => "reference",
            Ty::String => "string",
            Ty::TextArea => "textarea",
            Ty::Picklist => "picklist",
            Ty::MultiPicklist => "multipicklist",
            Ty::Combobox => "combobox",
            Ty::Boolean => "boolean",
            Ty::Currency => "currency",
            Ty::Double => "double",
            Ty::Percent => "percent",
            Ty::Int => "int",
            Ty::Date => "date",
            Ty::DateTime => "datetime",
            Ty::Email => "email",
            Ty::Phone => "phone",
            Ty::Url => "url",
        }
    }

    /// The type name used in SOQL filter type errors.
    pub fn soql_name(self) -> &'static str {
        match self {
            Ty::Id | Ty::Reference => "id",
            Ty::Boolean => "boolean",
            Ty::Currency | Ty::Double | Ty::Percent => "double",
            Ty::Int => "int",
            Ty::Date => "date",
            Ty::DateTime => "dateTime",
            _ => "string",
        }
    }

    fn soap_type(self) -> &'static str {
        match self {
            Ty::Id | Ty::Reference => "tns:ID",
            Ty::Boolean => "xsd:boolean",
            Ty::Currency | Ty::Double | Ty::Percent => "xsd:double",
            Ty::Int => "xsd:int",
            Ty::Date => "xsd:date",
            Ty::DateTime => "xsd:dateTime",
            _ => "xsd:string",
        }
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, Ty::Currency | Ty::Double | Ty::Percent | Ty::Int)
    }

    pub fn is_id(self) -> bool {
        matches!(self, Ty::Id | Ty::Reference)
    }

    /// Searched by SOSL `ALL FIELDS`.
    pub fn is_searchable_text(self) -> bool {
        matches!(
            self,
            Ty::String | Ty::TextArea | Ty::Email | Ty::Phone | Ty::Url | Ty::Combobox
        )
    }

    pub fn filterable(self) -> bool {
        self != Ty::TextArea
    }

    pub fn sortable(self) -> bool {
        !matches!(self, Ty::TextArea | Ty::MultiPicklist)
    }

    pub fn groupable(self) -> bool {
        !matches!(
            self,
            Ty::TextArea
                | Ty::MultiPicklist
                | Ty::Currency
                | Ty::Double
                | Ty::Percent
                | Ty::DateTime
        )
    }

    fn default_length(self) -> u32 {
        match self {
            Ty::Id | Ty::Reference => 18,
            Ty::String | Ty::Url | Ty::Combobox | Ty::Picklist => 255,
            Ty::TextArea => 32000,
            Ty::MultiPicklist => 4099,
            Ty::Email => 80,
            Ty::Phone => 40,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub label: String,
    pub ty: Ty,
    pub length: u32,
    pub precision: u32,
    pub scale: u32,
    pub digits: u32,
    pub nillable: bool,
    pub createable: bool,
    pub updateable: bool,
    pub defaulted: bool,
    pub calculated: bool,
    pub name_field: bool,
    pub unique: bool,
    pub external_id: bool,
    pub custom: bool,
    pub auto_number: bool,
    pub refs: Vec<String>,
    pub rel: Option<String>,
    pub child_rel: Option<String>,
    pub cascade: bool,
    pub picklist: Vec<String>,
    pub default_pick: Option<String>,
    pub help: Option<String>,
}

pub fn fd(name: &str, label: &str, ty: Ty) -> Field {
    let (precision, scale, digits) = match ty {
        Ty::Currency => (18, 2, 0),
        Ty::Double => (18, 0, 0),
        Ty::Percent => (3, 0, 0),
        Ty::Int => (0, 0, 9),
        _ => (0, 0, 0),
    };
    Field {
        name: name.to_owned(),
        label: label.to_owned(),
        ty,
        length: ty.default_length(),
        precision,
        scale,
        digits,
        nillable: ty != Ty::Boolean,
        createable: true,
        updateable: true,
        defaulted: ty == Ty::Boolean,
        calculated: false,
        name_field: false,
        unique: false,
        external_id: false,
        custom: name.ends_with("__c"),
        auto_number: false,
        refs: Vec::new(),
        rel: None,
        child_rel: None,
        cascade: false,
        picklist: Vec::new(),
        default_pick: None,
        help: None,
    }
}

impl Field {
    pub fn req(mut self) -> Self {
        self.nillable = false;
        self
    }
    pub fn ro(mut self) -> Self {
        self.createable = false;
        self.updateable = false;
        self
    }
    pub fn create_only(mut self) -> Self {
        self.updateable = false;
        self
    }
    pub fn len(mut self, n: u32) -> Self {
        self.length = n;
        self
    }
    pub fn refs(mut self, to: &[&str], rel: &str) -> Self {
        self.refs = to.iter().map(|s| (*s).to_owned()).collect();
        self.rel = Some(rel.to_owned());
        self
    }
    pub fn child(mut self, name: &str) -> Self {
        self.child_rel = Some(name.to_owned());
        self
    }
    pub fn cascade(mut self) -> Self {
        self.cascade = true;
        self
    }
    pub fn pick(mut self, values: &[&str]) -> Self {
        self.picklist = values.iter().map(|s| (*s).to_owned()).collect();
        self
    }
    /// Defaulted on create to `value` (and therefore not required from the caller).
    pub fn default_to(mut self, value: &str) -> Self {
        self.default_pick = Some(value.to_owned());
        self.defaulted = true;
        self.nillable = false;
        self
    }
    pub fn defaulted(mut self) -> Self {
        self.defaulted = true;
        self
    }
    pub fn calc(mut self) -> Self {
        self.calculated = true;
        self.ro()
    }
    pub fn name_field(mut self) -> Self {
        self.name_field = true;
        self
    }
    pub fn auto_number(mut self) -> Self {
        self.auto_number = true;
        self.defaulted = true;
        self.nillable = false;
        self.ro()
    }
    pub fn precision(mut self, p: u32, s: u32) -> Self {
        self.precision = p;
        self.scale = s;
        self
    }

    /// The caller must supply a value on create.
    pub fn required_on_create(&self) -> bool {
        !self.nillable && self.createable && !self.defaulted
    }

    pub fn id_lookup(&self) -> bool {
        self.ty == Ty::Id || self.external_id || (self.name_field && self.ty == Ty::String)
    }
}

#[derive(Debug, Clone)]
pub struct ChildRel {
    pub child: String,
    pub field: String,
    pub name: Option<String>,
    pub cascade: bool,
}

#[derive(Debug, Clone)]
pub struct Object {
    pub name: String,
    pub label: String,
    pub plural: String,
    pub prefix: String,
    pub custom: bool,
    pub fields: Vec<Field>,
    pub data_api: bool,
    pub tooling_api: bool,
    pub createable: bool,
    pub updateable: bool,
    pub deletable: bool,
    pub searchable: bool,
    /// Modelled in depth (records, CRUD); others only appear in the global describe.
    pub modelled: bool,
    pub children: Vec<ChildRel>,
}

impl Object {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// A parent relationship by name (`Account`, `Owner`, `Parent`).
    pub fn relationship(&self, rel: &str) -> Option<&Field> {
        self.fields.iter().find(|f| {
            f.rel
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case(rel))
        })
    }

    pub fn child(&self, rel: &str) -> Option<&ChildRel> {
        self.children.iter().find(|c| {
            c.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(rel))
        })
    }

    pub fn has_field(&self, name: &str) -> bool {
        self.field(name).is_some()
    }

    pub fn name_field(&self) -> Option<&Field> {
        self.fields.iter().find(|f| f.name_field)
    }
}

fn obj(name: &str, label: &str, plural: &str, prefix: &str, fields: Vec<Field>) -> Object {
    Object {
        name: name.to_owned(),
        label: label.to_owned(),
        plural: plural.to_owned(),
        prefix: prefix.to_owned(),
        custom: name.ends_with("__c"),
        fields,
        data_api: true,
        tooling_api: false,
        createable: true,
        updateable: true,
        deletable: true,
        searchable: true,
        modelled: true,
        children: Vec::new(),
    }
}

/// `Id`, `IsDeleted`, then `body`, then the audit fields.
fn with_system(label: &str, body: Vec<Field>, owner: bool, tail: Vec<Field>) -> Vec<Field> {
    let mut fields = vec![
        fd("Id", &format!("{label} ID"), Ty::Id)
            .ro()
            .req()
            .defaulted(),
        fd("IsDeleted", "Deleted", Ty::Boolean).ro(),
    ];
    fields.extend(body);
    if owner {
        fields.push(
            fd("OwnerId", "Owner ID", Ty::Reference)
                .refs(&["User"], "Owner")
                .req()
                .defaulted(),
        );
    }
    fields.extend(audit());
    fields.extend(tail);
    fields
}

fn audit() -> Vec<Field> {
    vec![
        fd("CreatedDate", "Created Date", Ty::DateTime)
            .ro()
            .req()
            .defaulted(),
        fd("CreatedById", "Created By ID", Ty::Reference)
            .refs(&["User"], "CreatedBy")
            .ro()
            .req()
            .defaulted(),
        fd("LastModifiedDate", "Last Modified Date", Ty::DateTime)
            .ro()
            .req()
            .defaulted(),
        fd("LastModifiedById", "Last Modified By ID", Ty::Reference)
            .refs(&["User"], "LastModifiedBy")
            .ro()
            .req()
            .defaulted(),
        fd("SystemModstamp", "System Modstamp", Ty::DateTime)
            .ro()
            .req()
            .defaulted(),
    ]
}

fn address(prefix: &str, label: &str) -> Vec<Field> {
    vec![
        fd(
            &format!("{prefix}Street"),
            &format!("{label} Street"),
            Ty::TextArea,
        )
        .len(255),
        fd(
            &format!("{prefix}City"),
            &format!("{label} City"),
            Ty::String,
        )
        .len(40),
        fd(
            &format!("{prefix}State"),
            &format!("{label} State/Province"),
            Ty::String,
        )
        .len(80),
        fd(
            &format!("{prefix}PostalCode"),
            &format!("{label} Zip/Postal Code"),
            Ty::String,
        )
        .len(20),
        fd(
            &format!("{prefix}Country"),
            &format!("{label} Country"),
            Ty::String,
        )
        .len(80),
    ]
}

const INDUSTRIES: &[&str] = &[
    "Agriculture",
    "Apparel",
    "Banking",
    "Biotechnology",
    "Chemicals",
    "Communications",
    "Construction",
    "Consulting",
    "Education",
    "Electronics",
    "Energy",
    "Engineering",
    "Entertainment",
    "Environmental",
    "Finance",
    "Food & Beverage",
    "Government",
    "Healthcare",
    "Hospitality",
    "Insurance",
    "Machinery",
    "Manufacturing",
    "Media",
    "Not For Profit",
    "Recreation",
    "Retail",
    "Shipping",
    "Technology",
    "Telecommunications",
    "Transportation",
    "Utilities",
    "Other",
];
const SOURCES: &[&str] = &[
    "Web",
    "Phone Inquiry",
    "Partner Referral",
    "Purchased List",
    "Other",
];
const SALUTATIONS: &[&str] = &["Mr.", "Ms.", "Mrs.", "Dr.", "Prof.", "Mx."];
const LOG_LEVELS: &[&str] = &[
    "NONE", "ERROR", "WARN", "INFO", "DEBUG", "FINE", "FINER", "FINEST",
];

/// Opportunity stages with their default probability and forecast category.
pub const STAGES: &[(&str, f64, &str, &str)] = &[
    ("Prospecting", 10.0, "Pipeline", "Pipeline"),
    ("Qualification", 10.0, "Pipeline", "Pipeline"),
    ("Needs Analysis", 20.0, "Pipeline", "Pipeline"),
    ("Value Proposition", 50.0, "Pipeline", "Pipeline"),
    ("Id. Decision Makers", 60.0, "Pipeline", "Pipeline"),
    ("Perception Analysis", 70.0, "Pipeline", "Pipeline"),
    ("Proposal/Price Quote", 75.0, "Pipeline", "Pipeline"),
    ("Negotiation/Review", 90.0, "Pipeline", "Pipeline"),
    ("Closed Won", 100.0, "Closed", "Closed"),
    ("Closed Lost", 0.0, "Omitted", "Omitted"),
];

fn account() -> Object {
    let mut body = vec![
        fd("MasterRecordId", "Master Record ID", Ty::Reference)
            .refs(&["Account"], "MasterRecord")
            .ro(),
        fd("Name", "Account Name", Ty::String).req().name_field(),
        fd("Type", "Account Type", Ty::Picklist).pick(&[
            "Prospect",
            "Customer - Direct",
            "Customer - Channel",
            "Channel Partner / Reseller",
            "Installation Partner",
            "Technology Partner",
            "Other",
        ]),
        fd("ParentId", "Parent Account ID", Ty::Reference)
            .refs(&["Account"], "Parent")
            .child("ChildAccounts"),
    ];
    body.extend(address("Billing", "Billing"));
    body.extend(address("Shipping", "Shipping"));
    body.extend([
        fd("Phone", "Account Phone", Ty::Phone),
        fd("Fax", "Account Fax", Ty::Phone),
        fd("AccountNumber", "Account Number", Ty::String).len(40),
        fd("Website", "Website", Ty::Url),
        fd("Sic", "SIC Code", Ty::String).len(20),
        fd("Industry", "Industry", Ty::Picklist).pick(INDUSTRIES),
        fd("AnnualRevenue", "Annual Revenue", Ty::Currency),
        fd("NumberOfEmployees", "Employees", Ty::Int).precision(0, 0),
        fd("Ownership", "Ownership", Ty::Picklist).pick(&[
            "Public",
            "Private",
            "Subsidiary",
            "Other",
        ]),
        fd("TickerSymbol", "Ticker Symbol", Ty::String).len(20),
        fd("Description", "Account Description", Ty::TextArea),
        fd("Rating", "Account Rating", Ty::Picklist).pick(&["Hot", "Warm", "Cold"]),
        fd("Site", "Account Site", Ty::String).len(80),
    ]);
    let tail = vec![
        fd("LastActivityDate", "Last Activity", Ty::Date).ro(),
        fd("AccountSource", "Account Source", Ty::Picklist).pick(SOURCES),
    ];
    obj(
        "Account",
        "Account",
        "Accounts",
        "001",
        with_system("Account", body, true, tail),
    )
}

fn contact() -> Object {
    let mut body = vec![
        fd("MasterRecordId", "Master Record ID", Ty::Reference)
            .refs(&["Contact"], "MasterRecord")
            .ro(),
        fd("AccountId", "Account ID", Ty::Reference)
            .refs(&["Account"], "Account")
            .child("Contacts")
            .cascade(),
        fd("LastName", "Last Name", Ty::String).len(80).req(),
        fd("FirstName", "First Name", Ty::String).len(40),
        fd("Salutation", "Salutation", Ty::Picklist).pick(SALUTATIONS),
        fd("Name", "Full Name", Ty::String)
            .len(121)
            .ro()
            .req()
            .defaulted()
            .name_field(),
    ];
    body.extend(address("Mailing", "Mailing"));
    body.extend([
        fd("Phone", "Business Phone", Ty::Phone),
        fd("MobilePhone", "Mobile Phone", Ty::Phone),
        fd("Email", "Email", Ty::Email),
        fd("Title", "Title", Ty::String).len(128),
        fd("Department", "Department", Ty::String).len(80),
        fd("ReportsToId", "Reports To ID", Ty::Reference).refs(&["Contact"], "ReportsTo"),
        fd("LeadSource", "Lead Source", Ty::Picklist).pick(SOURCES),
        fd("Birthdate", "Birthdate", Ty::Date),
        fd("Description", "Contact Description", Ty::TextArea),
        fd("HasOptedOutOfEmail", "Email Opt Out", Ty::Boolean),
        fd("DoNotCall", "Do Not Call", Ty::Boolean),
    ]);
    let tail = vec![fd("LastActivityDate", "Last Activity", Ty::Date).ro()];
    obj(
        "Contact",
        "Contact",
        "Contacts",
        "003",
        with_system("Contact", body, true, tail),
    )
}

fn lead() -> Object {
    let mut body = vec![
        fd("MasterRecordId", "Master Record ID", Ty::Reference)
            .refs(&["Lead"], "MasterRecord")
            .ro(),
        fd("LastName", "Last Name", Ty::String).len(80).req(),
        fd("FirstName", "First Name", Ty::String).len(40),
        fd("Salutation", "Salutation", Ty::Picklist).pick(SALUTATIONS),
        fd("Name", "Full Name", Ty::String)
            .len(121)
            .ro()
            .req()
            .defaulted()
            .name_field(),
        fd("Title", "Title", Ty::String).len(128),
        fd("Company", "Company", Ty::String).len(255).req(),
    ];
    body.extend([
        fd("Street", "Street", Ty::TextArea).len(255),
        fd("City", "City", Ty::String).len(40),
        fd("State", "State/Province", Ty::String).len(80),
        fd("PostalCode", "Zip/Postal Code", Ty::String).len(20),
        fd("Country", "Country", Ty::String).len(80),
        fd("Phone", "Phone", Ty::Phone),
        fd("MobilePhone", "Mobile Phone", Ty::Phone),
        fd("Email", "Email", Ty::Email),
        fd("Website", "Website", Ty::Url),
        fd("Description", "Description", Ty::TextArea),
        fd("LeadSource", "Lead Source", Ty::Picklist).pick(SOURCES),
        fd("Status", "Status", Ty::Picklist)
            .pick(&[
                "Open - Not Contacted",
                "Working - Contacted",
                "Closed - Converted",
                "Closed - Not Converted",
            ])
            .default_to("Open - Not Contacted"),
        fd("Industry", "Industry", Ty::Picklist).pick(INDUSTRIES),
        fd("Rating", "Rating", Ty::Picklist).pick(&["Hot", "Warm", "Cold"]),
        fd("AnnualRevenue", "Annual Revenue", Ty::Currency),
        fd("NumberOfEmployees", "Employees", Ty::Int),
    ]);
    let tail = vec![
        fd("IsConverted", "Converted", Ty::Boolean).create_only(),
        fd("ConvertedDate", "Converted Date", Ty::Date).ro(),
        fd("ConvertedAccountId", "Converted Account ID", Ty::Reference)
            .refs(&["Account"], "ConvertedAccount")
            .ro(),
        fd("ConvertedContactId", "Converted Contact ID", Ty::Reference)
            .refs(&["Contact"], "ConvertedContact")
            .ro(),
        fd(
            "ConvertedOpportunityId",
            "Converted Opportunity ID",
            Ty::Reference,
        )
        .refs(&["Opportunity"], "ConvertedOpportunity")
        .ro(),
        fd("IsUnreadByOwner", "Unread By Owner", Ty::Boolean),
        fd("LastActivityDate", "Last Activity", Ty::Date).ro(),
    ];
    obj(
        "Lead",
        "Lead",
        "Leads",
        "00Q",
        with_system("Lead", body, true, tail),
    )
}

fn opportunity() -> Object {
    let body = vec![
        fd("AccountId", "Account ID", Ty::Reference)
            .refs(&["Account"], "Account")
            .child("Opportunities")
            .cascade(),
        fd("IsPrivate", "Private", Ty::Boolean),
        fd("Name", "Name", Ty::String).len(120).req().name_field(),
        fd("Description", "Description", Ty::TextArea),
        fd("StageName", "Stage", Ty::Picklist)
            .pick(&STAGES.iter().map(|s| s.0).collect::<Vec<_>>())
            .req(),
        fd("Amount", "Amount", Ty::Currency),
        fd("Probability", "Probability (%)", Ty::Percent).defaulted(),
        fd("ExpectedRevenue", "Expected Amount", Ty::Currency).calc(),
        fd("TotalOpportunityQuantity", "Quantity", Ty::Double).precision(18, 2),
        fd("CloseDate", "Close Date", Ty::Date).req(),
        fd("Type", "Opportunity Type", Ty::Picklist).pick(&[
            "New Customer",
            "Existing Customer - Upgrade",
            "Existing Customer - Replacement",
            "Existing Customer - Downgrade",
        ]),
        fd("NextStep", "Next Step", Ty::String).len(255),
        fd("LeadSource", "Lead Source", Ty::Picklist).pick(SOURCES),
        fd("IsClosed", "Closed", Ty::Boolean).ro(),
        fd("IsWon", "Won", Ty::Boolean).ro(),
        fd("ForecastCategory", "Forecast Category", Ty::Picklist)
            .pick(&["Omitted", "Pipeline", "BestCase", "Forecast", "Closed"])
            .ro()
            .req()
            .defaulted(),
        fd("ForecastCategoryName", "Forecast Category", Ty::Picklist)
            .pick(&["Omitted", "Pipeline", "Best Case", "Commit", "Closed"])
            .defaulted(),
    ];
    let tail = vec![
        fd("LastActivityDate", "Last Activity", Ty::Date).ro(),
        fd("FiscalQuarter", "Fiscal Quarter", Ty::Int).ro(),
        fd("FiscalYear", "Fiscal Year", Ty::Int).ro(),
        fd("Fiscal", "Fiscal Period", Ty::String).len(6).ro(),
    ];
    obj(
        "Opportunity",
        "Opportunity",
        "Opportunities",
        "006",
        with_system("Opportunity", body, true, tail),
    )
}

fn case() -> Object {
    let body = vec![
        fd("MasterRecordId", "Master Record ID", Ty::Reference)
            .refs(&["Case"], "MasterRecord")
            .ro(),
        fd("CaseNumber", "Case Number", Ty::String)
            .len(30)
            .auto_number()
            .name_field(),
        fd("ContactId", "Contact ID", Ty::Reference)
            .refs(&["Contact"], "Contact")
            .child("Cases"),
        fd("AccountId", "Account ID", Ty::Reference)
            .refs(&["Account"], "Account")
            .child("Cases"),
        fd("ParentId", "Parent Case ID", Ty::Reference)
            .refs(&["Case"], "Parent")
            .child("Cases"),
        fd("SuppliedName", "Name", Ty::String).len(80),
        fd("SuppliedEmail", "Email Address", Ty::Email),
        fd("SuppliedPhone", "Phone", Ty::String).len(40),
        fd("SuppliedCompany", "Company", Ty::String).len(80),
        fd("Type", "Case Type", Ty::Picklist).pick(&[
            "Mechanical",
            "Electrical",
            "Electronic",
            "Structural",
            "Other",
        ]),
        fd("Status", "Status", Ty::Picklist)
            .pick(&["New", "Working", "Escalated", "Closed"])
            .default_to("New"),
        fd("Reason", "Case Reason", Ty::Picklist).pick(&[
            "Installation",
            "Equipment Complexity",
            "Performance",
            "Breakdown",
            "Equipment Design",
            "Feedback",
            "Other",
        ]),
        fd("Origin", "Case Origin", Ty::Picklist).pick(&["Phone", "Email", "Web"]),
        fd("Subject", "Subject", Ty::String),
        fd("Priority", "Priority", Ty::Picklist)
            .pick(&["High", "Medium", "Low"])
            .default_to("Medium"),
        fd("Description", "Description", Ty::TextArea),
        fd("IsClosed", "Closed", Ty::Boolean).ro(),
        fd("ClosedDate", "Closed Date", Ty::DateTime).ro(),
        fd("IsEscalated", "Escalated", Ty::Boolean),
    ];
    obj(
        "Case",
        "Case",
        "Cases",
        "500",
        with_system("Case", body, true, Vec::new()),
    )
}

fn task() -> Object {
    let body = vec![
        fd("WhoId", "Name ID", Ty::Reference)
            .refs(&["Contact", "Lead"], "Who")
            .child("Tasks"),
        fd("WhatId", "Related To ID", Ty::Reference)
            .refs(&["Account", "Opportunity", "Case"], "What")
            .child("Tasks"),
        fd("Subject", "Subject", Ty::Combobox).pick(&[
            "Call",
            "Send Letter",
            "Send Quote",
            "Other",
        ]),
        fd("ActivityDate", "Due Date Only", Ty::Date),
        fd("Status", "Status", Ty::Picklist)
            .pick(&[
                "Not Started",
                "In Progress",
                "Completed",
                "Waiting on someone else",
                "Deferred",
            ])
            .default_to("Not Started"),
        fd("Priority", "Priority", Ty::Picklist)
            .pick(&["High", "Normal", "Low"])
            .default_to("Normal"),
        fd("IsHighPriority", "High Priority", Ty::Boolean).ro(),
        fd("Description", "Description", Ty::TextArea),
        fd("Type", "Type", Ty::Picklist).pick(&["Call", "Email", "Meeting", "Other"]),
        fd("AccountId", "Account ID", Ty::Reference)
            .refs(&["Account"], "Account")
            .ro(),
        fd("IsClosed", "Closed", Ty::Boolean).ro(),
        fd("CallDurationInSeconds", "Call Duration", Ty::Int),
        fd("CallType", "Call Type", Ty::Picklist).pick(&["Internal", "Inbound", "Outbound"]),
        fd("CallDisposition", "Call Result", Ty::String),
        fd("TaskSubtype", "Task Subtype", Ty::Picklist)
            .pick(&["Task", "Email", "ListEmail", "Cadence", "Call", "LinkedIn"])
            .create_only()
            .default_to("Task"),
        fd("CompletedDateTime", "Completed Date/Time", Ty::DateTime).ro(),
    ];
    obj(
        "Task",
        "Task",
        "Tasks",
        "00T",
        with_system("Task", body, true, Vec::new()),
    )
}

fn user() -> Object {
    let mut body = vec![
        fd("Username", "Username", Ty::String).len(80).req(),
        fd("LastName", "Last Name", Ty::String).len(80).req(),
        fd("FirstName", "First Name", Ty::String).len(40),
        fd("Name", "Full Name", Ty::String)
            .len(121)
            .ro()
            .req()
            .defaulted()
            .name_field(),
        fd("CompanyName", "Company Name", Ty::String).len(80),
        fd("Division", "Division", Ty::String).len(80),
        fd("Department", "Department", Ty::String).len(80),
        fd("Title", "Title", Ty::String).len(80),
    ];
    body.extend(address("", ""));
    for f in body.iter_mut() {
        f.label = f.label.trim().to_owned();
    }
    body.extend([
        fd("Email", "Email", Ty::Email).len(128).req(),
        fd("Phone", "Phone", Ty::Phone),
        fd("MobilePhone", "Mobile", Ty::Phone),
        fd("Alias", "Alias", Ty::String).len(8).req(),
        fd("CommunityNickname", "Nickname", Ty::String)
            .len(40)
            .defaulted(),
        fd("IsActive", "Active", Ty::Boolean),
        fd("TimeZoneSidKey", "Time Zone", Ty::Picklist)
            .pick(&[
                "America/Los_Angeles",
                "America/Denver",
                "America/Chicago",
                "America/New_York",
                "Europe/London",
                "Europe/Paris",
                "Asia/Tokyo",
                "GMT",
            ])
            .req(),
        fd("UserRoleId", "Role ID", Ty::Reference).refs(&["UserRole"], "UserRole"),
        fd("LocaleSidKey", "Locale", Ty::Picklist)
            .pick(&["en_US", "en_GB", "fr_FR", "de_DE", "ja_JP"])
            .req(),
        fd("EmailEncodingKey", "Email Encoding", Ty::Picklist)
            .pick(&["UTF-8", "ISO-8859-1", "Shift_JIS"])
            .req(),
        fd("ProfileId", "Profile ID", Ty::Reference)
            .refs(&["Profile"], "Profile")
            .req(),
        fd("UserType", "User Type", Ty::Picklist)
            .pick(&["Standard", "PowerPartner", "CsnOnly", "Guest"])
            .ro()
            .defaulted(),
        fd("LanguageLocaleKey", "Language", Ty::Picklist)
            .pick(&["en_US", "fr", "de", "ja"])
            .req(),
        fd("EmployeeNumber", "Employee Number", Ty::String).len(20),
        fd("ManagerId", "Manager ID", Ty::Reference).refs(&["User"], "Manager"),
        fd("LastLoginDate", "Last Login", Ty::DateTime).ro(),
    ]);
    obj(
        "User",
        "User",
        "Users",
        "005",
        with_system_no_delete("User", body),
    )
}

fn with_system_no_delete(label: &str, body: Vec<Field>) -> Vec<Field> {
    let mut fields = vec![
        fd("Id", &format!("{label} ID"), Ty::Id)
            .ro()
            .req()
            .defaulted(),
    ];
    fields.extend(body);
    fields.extend(audit());
    fields
}

fn profile() -> Object {
    let body = vec![
        fd("Name", "Name", Ty::String).req().name_field(),
        fd("UserType", "User Type", Ty::Picklist)
            .pick(&["Standard", "PowerPartner", "Guest"])
            .ro(),
        fd("Description", "Description", Ty::String),
    ];
    let mut o = obj(
        "Profile",
        "Profile",
        "Profiles",
        "00e",
        with_system_no_delete("Profile", body),
    );
    o.createable = false;
    o.deletable = false;
    o.searchable = false;
    o
}

fn permission_set() -> Object {
    let body = vec![
        fd("Name", "Name", Ty::String).len(80).req().name_field(),
        fd("Label", "Label", Ty::String).len(80).req(),
        fd("IsOwnedByProfile", "Is Owned By Profile", Ty::Boolean).ro(),
        fd("ProfileId", "Profile ID", Ty::Reference)
            .refs(&["Profile"], "Profile")
            .ro(),
        fd("Description", "Description", Ty::String),
    ];
    let mut o = obj(
        "PermissionSet",
        "Permission Set",
        "Permission Sets",
        "0PS",
        with_system_no_delete("Permission Set", body),
    );
    o.searchable = false;
    o
}

fn field_permissions() -> Object {
    let body = vec![
        fd("ParentId", "Parent ID", Ty::Reference)
            .refs(&["PermissionSet"], "Parent")
            .child("FieldPerms")
            .req()
            .create_only(),
        fd("SobjectType", "Sobject Type Name", Ty::Picklist)
            .req()
            .create_only(),
        fd("Field", "Field Name", Ty::Picklist).req().create_only(),
        fd("PermissionsEdit", "Edit Field", Ty::Boolean),
        fd("PermissionsRead", "Read Field", Ty::Boolean),
    ];
    let mut o = obj(
        "FieldPermissions",
        "Field Permissions",
        "Field Permissions",
        "01k",
        with_system_no_delete("Field Permissions", body),
    );
    o.searchable = false;
    o
}

fn apex_class() -> Object {
    let body = vec![
        fd("NamespacePrefix", "Namespace Prefix", Ty::String)
            .len(15)
            .ro(),
        fd("Name", "Name", Ty::String).len(255).req().name_field(),
        fd("ApiVersion", "Api Version", Ty::Double)
            .precision(18, 1)
            .req(),
        fd("Status", "Status", Ty::Picklist)
            .pick(&["Inactive", "Active", "Deleted"])
            .default_to("Active"),
        fd("IsValid", "Is Valid", Ty::Boolean).ro(),
        fd("BodyCrc", "Body CRC", Ty::Double).ro(),
        fd("Body", "Body", Ty::TextArea).len(1_000_000).req(),
        fd("LengthWithoutComments", "Size Without Comments", Ty::Int).ro(),
    ];
    let mut o = obj(
        "ApexClass",
        "Apex Class",
        "Apex Classes",
        "01p",
        with_system_no_delete("Class", body),
    );
    o.tooling_api = true;
    o.searchable = false;
    o
}

fn apex_trigger() -> Object {
    let mut body = vec![
        fd("NamespacePrefix", "Namespace Prefix", Ty::String)
            .len(15)
            .ro(),
        fd("Name", "Name", Ty::String).len(255).req().name_field(),
        fd("TableEnumOrId", "Custom Object Definition ID", Ty::Picklist)
            .req()
            .create_only(),
    ];
    for (n, l) in [
        ("UsageBeforeInsert", "Before Insert"),
        ("UsageAfterInsert", "After Insert"),
        ("UsageBeforeUpdate", "Before Update"),
        ("UsageAfterUpdate", "After Update"),
        ("UsageBeforeDelete", "Before Delete"),
        ("UsageAfterDelete", "After Delete"),
        ("UsageIsBulk", "Is Bulk"),
        ("UsageAfterUndelete", "After Undelete"),
    ] {
        body.push(fd(n, l, Ty::Boolean).ro());
    }
    body.extend([
        fd("ApiVersion", "Api Version", Ty::Double)
            .precision(18, 1)
            .req(),
        fd("Status", "Status", Ty::Picklist)
            .pick(&["Inactive", "Active", "Deleted"])
            .default_to("Active"),
        fd("IsValid", "Is Valid", Ty::Boolean).ro(),
        fd("BodyCrc", "Body CRC", Ty::Double).ro(),
        fd("Body", "Body", Ty::TextArea).len(1_000_000).req(),
        fd("LengthWithoutComments", "Size Without Comments", Ty::Int).ro(),
    ]);
    let mut o = obj(
        "ApexTrigger",
        "Apex Trigger",
        "Apex Triggers",
        "01q",
        with_system_no_delete("Trigger", body),
    );
    o.tooling_api = true;
    o.searchable = false;
    o
}

fn apex_log() -> Object {
    let fields = vec![
        fd("Id", "Debug Log ID", Ty::Id).ro().req().defaulted(),
        fd("LogUserId", "User ID", Ty::Reference)
            .refs(&["User"], "LogUser")
            .ro(),
        fd("LogLength", "Size", Ty::Int).ro(),
        fd("LastModifiedDate", "Last Modified Date", Ty::DateTime).ro(),
        fd("Request", "Request Type", Ty::String).len(16).ro(),
        fd("Operation", "Operation", Ty::String).len(128).ro(),
        fd("Application", "Application", Ty::String).len(64).ro(),
        fd("Status", "Status", Ty::String).ro(),
        fd("DurationMilliseconds", "Duration", Ty::Int).ro(),
        fd("SystemModstamp", "System Modstamp", Ty::DateTime).ro(),
        fd("StartTime", "Start Time", Ty::DateTime).ro(),
        fd("Location", "Location", Ty::Picklist)
            .pick(&[
                "Monitoring",
                "Profiling",
                "SystemLog",
                "HeapDump",
                "Preserved",
            ])
            .ro(),
        fd("RequestIdentifier", "Request ID", Ty::String)
            .len(64)
            .ro(),
    ];
    let mut o = obj(
        "ApexLog",
        "Apex Debug Log",
        "Apex Debug Logs",
        "07L",
        fields,
    );
    o.tooling_api = true;
    o.createable = false;
    o.updateable = false;
    o.searchable = false;
    o
}

fn trace_flag() -> Object {
    let body = vec![
        fd("TracedEntityId", "Traced Entity ID", Ty::Reference)
            .refs(&["User", "ApexClass", "ApexTrigger"], "TracedEntity")
            .req(),
        fd("DebugLevelId", "Debug Level ID", Ty::Reference)
            .refs(&["DebugLevel"], "DebugLevel")
            .req(),
        fd("LogType", "Log Type", Ty::Picklist)
            .pick(&["USER_DEBUG", "DEVELOPER_LOG", "CLASS_TRACING", "PROFILING"])
            .req(),
        fd("StartDate", "Start Date", Ty::DateTime),
        fd("ExpirationDate", "Expiration Date", Ty::DateTime),
    ];
    let mut o = obj(
        "TraceFlag",
        "Trace Flag",
        "Trace Flags",
        "7tf",
        with_system_no_delete("Trace Flag", body),
    );
    o.data_api = false;
    o.tooling_api = true;
    o.searchable = false;
    o
}

fn debug_level() -> Object {
    let mut body = vec![
        fd("DeveloperName", "Developer Name", Ty::String)
            .len(80)
            .req(),
        fd("Language", "Language", Ty::Picklist)
            .pick(&["en_US"])
            .default_to("en_US"),
        fd("MasterLabel", "Label", Ty::String).len(80).req(),
    ];
    for name in [
        "Workflow",
        "Validation",
        "Callout",
        "ApexCode",
        "ApexProfiling",
        "Visualforce",
        "System",
        "Database",
        "Wave",
        "Nba",
    ] {
        body.push(
            fd(name, name, Ty::Picklist)
                .pick(LOG_LEVELS)
                .default_to("INFO"),
        );
    }
    let mut o = obj(
        "DebugLevel",
        "Debug Level",
        "Debug Levels",
        "7dl",
        with_system_no_delete("Debug Level", body),
    );
    o.data_api = false;
    o.tooling_api = true;
    o.searchable = false;
    o
}

fn user_role() -> Object {
    let body = vec![
        fd("Name", "Name", Ty::String).len(80).req().name_field(),
        fd("ParentRoleId", "Parent Role ID", Ty::Reference),
        fd("DeveloperName", "Developer Name", Ty::String).len(80),
    ];
    let mut o = obj(
        "UserRole",
        "Role",
        "Roles",
        "00E",
        with_system_no_delete("Role", body),
    );
    o.searchable = false;
    o
}

/// Objects that exist in every org but are only listed (no records modelled).
const LISTED: &[(&str, &str, &str, &str)] = &[
    (
        "AccountContactRelation",
        "Account Contact Relationship",
        "Account Contact Relationships",
        "07k",
    ),
    (
        "AccountHistory",
        "Account History",
        "Account History",
        "017",
    ),
    ("AccountShare", "Account Share", "Account Share", "00r"),
    (
        "AccountTeamMember",
        "Account Team Member",
        "Account Team Members",
        "01M",
    ),
    ("Asset", "Asset", "Assets", "02i"),
    ("Attachment", "Attachment", "Attachments", "00P"),
    ("Campaign", "Campaign", "Campaigns", "701"),
    (
        "CampaignMember",
        "Campaign Member",
        "Campaign Members",
        "00v",
    ),
    ("CaseComment", "Case Comment", "Case Comments", "00a"),
    ("CaseHistory", "Case History", "Case History", "017"),
    (
        "ContactHistory",
        "Contact History",
        "Contact History",
        "017",
    ),
    ("ContentDocument", "File", "Files", "069"),
    ("Contract", "Contract", "Contracts", "800"),
    ("EmailMessage", "Email Message", "Email Messages", "02s"),
    ("Entitlement", "Entitlement", "Entitlements", "550"),
    ("Event", "Event", "Events", "00U"),
    ("Group", "Group", "Groups", "00G"),
    ("LeadHistory", "Lead History", "Lead History", "017"),
    ("Note", "Note", "Notes", "002"),
    (
        "OpportunityContactRole",
        "Opportunity Contact Role",
        "Opportunity Contact Role",
        "00K",
    ),
    (
        "OpportunityHistory",
        "Opportunity History",
        "Opportunity History",
        "008",
    ),
    (
        "OpportunityLineItem",
        "Opportunity Product",
        "Opportunity Product",
        "00k",
    ),
    ("Order", "Order", "Orders", "801"),
    ("OrderItem", "Order Product", "Order Products", "802"),
    ("Organization", "Organization", "Organizations", "00D"),
    ("Pricebook2", "Price Book", "Price Books", "01s"),
    (
        "PricebookEntry",
        "Price Book Entry",
        "Price Book Entries",
        "01u",
    ),
    ("Product2", "Product", "Products", "01t"),
    ("Quote", "Quote", "Quotes", "0Q0"),
    ("RecordType", "Record Type", "Record Types", "012"),
    (
        "ServiceContract",
        "Service Contract",
        "Service Contracts",
        "810",
    ),
    ("Solution", "Solution", "Solutions", "501"),
    ("WorkOrder", "Work Order", "Work Orders", "0WO"),
];

fn listed(name: &str, label: &str, plural: &str, prefix: &str) -> Object {
    let mut body = Vec::new();
    if !name.ends_with("History") && !name.ends_with("Share") {
        body.push(fd("Name", "Name", Ty::String).len(80).name_field());
    }
    let mut o = obj(
        name,
        label,
        plural,
        prefix,
        with_system(label, body, false, Vec::new()),
    );
    o.modelled = false;
    o.searchable = !name.ends_with("History") && !name.ends_with("Share");
    o
}

fn standard() -> &'static [Object] {
    static STANDARD: OnceLock<Vec<Object>> = OnceLock::new();
    STANDARD.get_or_init(|| {
        let mut objects = vec![
            account(),
            contact(),
            lead(),
            opportunity(),
            case(),
            task(),
            user(),
            user_role(),
            profile(),
            permission_set(),
            field_permissions(),
            apex_class(),
            apex_trigger(),
            apex_log(),
            trace_flag(),
            debug_level(),
        ];
        objects.extend(LISTED.iter().map(|(n, l, p, k)| listed(n, l, p, k)));
        objects
    })
}

// ---------------------------------------------------------------------------------------
// Custom metadata overlay
// ---------------------------------------------------------------------------------------

/// Collections holding Metadata API definitions, keyed by full name.
pub const CUSTOM_OBJECTS: &str = "CustomObject";
pub const CUSTOM_FIELDS: &str = "CustomField";

fn text(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn truthy(v: &Value, key: &str) -> bool {
    matches!(v.get(key), Some(Value::Bool(true)))
        || text(v, key).is_some_and(|s| s.eq_ignore_ascii_case("true"))
}

fn num(v: &Value, key: &str) -> Option<u32> {
    text(v, key)
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| f as u32)
}

/// A describe field for a `CustomField` metadata definition.
pub fn field_from_metadata(md: &Value) -> Option<Field> {
    let full = text(md, "fullName")?;
    let name = full.split_once('.').map_or(full.as_str(), |(_, f)| f);
    let label = text(md, "label").unwrap_or_else(|| name.trim_end_matches("__c").to_owned());
    let kind = text(md, "type").unwrap_or_else(|| "Text".to_owned());
    let ty = match kind.as_str() {
        "Text" | "EncryptedText" | "AutoNumber" => Ty::String,
        "TextArea" | "LongTextArea" | "Html" => Ty::TextArea,
        "Number" => Ty::Double,
        "Currency" => Ty::Currency,
        "Percent" => Ty::Percent,
        "Checkbox" => Ty::Boolean,
        "Date" => Ty::Date,
        "DateTime" => Ty::DateTime,
        "Email" => Ty::Email,
        "Phone" => Ty::Phone,
        "Url" => Ty::Url,
        "Picklist" => Ty::Picklist,
        "MultiselectPicklist" => Ty::MultiPicklist,
        "Lookup" | "MasterDetail" => Ty::Reference,
        _ => Ty::String,
    };
    let mut f = fd(name, &label, ty);
    f.custom = true;
    if let Some(len) = num(md, "length") {
        f.length = len;
    }
    if ty.is_numeric()
        && let Some(p) = num(md, "precision")
    {
        f.precision = p;
        f.scale = num(md, "scale").unwrap_or(0);
    }
    f.unique = truthy(md, "unique");
    f.external_id = truthy(md, "externalId");
    f.help = text(md, "inlineHelpText");
    if truthy(md, "required") || kind == "MasterDetail" {
        f.nillable = false;
    }
    if kind == "AutoNumber" {
        f = f.auto_number();
    }
    if ty == Ty::Reference {
        let to = text(md, "referenceTo").unwrap_or_default();
        let rel =
            text(md, "relationshipName").unwrap_or_else(|| name.trim_end_matches("__c").to_owned());
        f.refs = vec![to];
        f.rel = Some(format!("{rel}__r"));
        f.child_rel = text(md, "relationshipName").map(|r| format!("{r}__r"));
        f.cascade = kind == "MasterDetail";
    }
    if let Some(values) = md
        .pointer("/valueSet/valueSetDefinition/value")
        .map(|v| match v {
            Value::Array(a) => a.clone(),
            other => vec![other.clone()],
        })
    {
        f.picklist = values
            .iter()
            .filter_map(|v| text(v, "fullName").or_else(|| text(v, "label")))
            .collect();
        f.default_pick = values
            .iter()
            .find(|v| truthy(v, "default"))
            .and_then(|v| text(v, "fullName"));
    }
    Some(f)
}

fn custom_object(md: &Value, prefix: String) -> Option<Object> {
    let name = text(md, "fullName")?;
    let label = text(md, "label").unwrap_or_else(|| name.trim_end_matches("__c").to_owned());
    let plural = text(md, "pluralLabel").unwrap_or_else(|| format!("{label}s"));
    let name_md = md.get("nameField").cloned().unwrap_or(Value::Null);
    let name_label = text(&name_md, "label").unwrap_or_else(|| format!("{label} Name"));
    let mut name_field = fd("Name", &name_label, Ty::String).len(80).name_field();
    if text(&name_md, "type").as_deref() == Some("AutoNumber") {
        name_field = name_field.auto_number();
    }
    let body = vec![name_field];
    let mut o = obj(
        &name,
        &label,
        &plural,
        &prefix,
        with_system(&label, body, true, Vec::new()),
    );
    o.custom = true;
    Some(o)
}

fn custom_prefix(index: usize) -> String {
    const B: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    format!("a{}{}", B[(index / 36) % 36] as char, B[index % 36] as char)
}

/// Every object visible in this org: standard ones plus Metadata API customizations.
#[derive(Debug, Clone)]
pub struct Schema {
    pub objects: Vec<Object>,
}

impl Schema {
    pub fn load(store: &Store) -> Self {
        let mut objects = standard().to_vec();
        if let Some(customs) = store.collection(CUSTOM_OBJECTS) {
            for (i, md) in customs.values().enumerate() {
                if let Some(o) = custom_object(md, custom_prefix(i)) {
                    objects.push(o);
                }
            }
        }
        if let Some(fields) = store.collection(CUSTOM_FIELDS) {
            for md in fields.values() {
                let Some(full) = text(md, "fullName") else {
                    continue;
                };
                let Some((object, _)) = full.split_once('.') else {
                    continue;
                };
                let Some(field) = field_from_metadata(md) else {
                    continue;
                };
                if let Some(o) = objects
                    .iter_mut()
                    .find(|o| o.name.eq_ignore_ascii_case(object))
                    && !o.has_field(&field.name)
                {
                    o.fields.push(field);
                }
            }
        }
        // Child relationships from every reference field.
        let mut rels: Vec<(String, ChildRel)> = Vec::new();
        for o in &objects {
            for f in &o.fields {
                for target in &f.refs {
                    rels.push((
                        target.clone(),
                        ChildRel {
                            child: o.name.clone(),
                            field: f.name.clone(),
                            name: f.child_rel.clone(),
                            cascade: f.cascade,
                        },
                    ));
                }
            }
        }
        for o in objects.iter_mut() {
            o.children = rels
                .iter()
                .filter(|(t, _)| *t == o.name)
                .map(|(_, r)| r.clone())
                .collect();
        }
        Self { objects }
    }

    pub fn get(&self, name: &str) -> Option<&Object> {
        self.objects
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case(name))
    }

    /// The object an Id belongs to, from its key prefix.
    pub fn by_prefix(&self, id: &str) -> Option<&Object> {
        let prefix = id.get(..3)?;
        self.objects
            .iter()
            .find(|o| o.prefix == prefix && o.modelled)
            .or_else(|| self.objects.iter().find(|o| o.prefix == prefix))
    }

    pub fn visible(&self, tooling: bool) -> impl Iterator<Item = &Object> {
        self.objects
            .iter()
            .filter(move |o| if tooling { o.tooling_api } else { o.data_api })
    }

    pub fn get_in(&self, name: &str, tooling: bool) -> Option<&Object> {
        self.get(name)
            .filter(|o| if tooling { o.tooling_api } else { o.data_api })
    }
}

// ---------------------------------------------------------------------------------------
// Describe payloads
// ---------------------------------------------------------------------------------------

fn base(version: &str, tooling: bool) -> String {
    if tooling {
        format!("/services/data/v{version}/tooling")
    } else {
        format!("/services/data/v{version}")
    }
}

fn object_urls(o: &Object, version: &str, tooling: bool) -> Value {
    let b = format!("{}/sobjects/{}", base(version, tooling), o.name);
    json!({
        "rowTemplate": format!("{b}/{{ID}}"),
        "defaultValues": format!("{b}/defaultValues?recordTypeId&fields"),
        "describe": format!("{b}/describe"),
        "sobject": b,
    })
}

fn flags(o: &Object) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("activateable".into(), json!(false));
    m.insert("associateEntityType".into(), Value::Null);
    m.insert("associateParentEntity".into(), Value::Null);
    m.insert("createable".into(), json!(o.createable));
    m.insert("custom".into(), json!(o.custom));
    m.insert("customSetting".into(), json!(false));
    m.insert("deepCloneable".into(), json!(false));
    m.insert("deletable".into(), json!(o.deletable));
    m.insert("deprecatedAndHidden".into(), json!(false));
    m.insert("feedEnabled".into(), json!(o.modelled && o.searchable));
    m.insert("hasSubtypes".into(), json!(false));
    m.insert("isInterface".into(), json!(false));
    m.insert("isSubtype".into(), json!(false));
    m.insert("keyPrefix".into(), json!(o.prefix));
    m.insert("label".into(), json!(o.label));
    m.insert("labelPlural".into(), json!(o.plural));
    m.insert("layoutable".into(), json!(o.searchable));
    m.insert(
        "mergeable".into(),
        json!(matches!(
            o.name.as_str(),
            "Account" | "Contact" | "Lead" | "Case"
        )),
    );
    m.insert("mruEnabled".into(), json!(o.searchable));
    m.insert("name".into(), json!(o.name));
    m.insert("queryable".into(), json!(true));
    m.insert("replicateable".into(), json!(true));
    m.insert("retrieveable".into(), json!(true));
    m.insert("searchable".into(), json!(o.searchable));
    m.insert("triggerable".into(), json!(o.searchable));
    m.insert("undeletable".into(), json!(o.deletable));
    m.insert("updateable".into(), json!(o.updateable));
    m
}

/// One entry of the global describe (`GET /sobjects`).
pub fn global_entry(o: &Object, version: &str, tooling: bool) -> Value {
    let mut m = flags(o);
    m.insert("urls".into(), object_urls(o, version, tooling));
    Value::Object(m)
}

pub fn global_describe(schema: &Schema, version: &str, tooling: bool) -> Value {
    let mut objects: Vec<&Object> = schema.visible(tooling).collect();
    objects.sort_by(|a, b| a.name.cmp(&b.name));
    json!({
        "encoding": "UTF-8",
        "maxBatchSize": 200,
        "sobjects": objects.iter().map(|o| global_entry(o, version, tooling)).collect::<Vec<_>>(),
    })
}

pub fn describe_field(f: &Field) -> Value {
    let picklist: Vec<Value> = f
        .picklist
        .iter()
        .map(|v| {
            json!({
                "active": true,
                "defaultValue": f.default_pick.as_deref() == Some(v.as_str()),
                "label": v,
                "validFor": null,
                "value": v,
            })
        })
        .collect();
    let byte_length = match f.ty {
        Ty::Id | Ty::Reference => 18,
        t if t.is_numeric() || matches!(t, Ty::Boolean | Ty::Date | Ty::DateTime) => 0,
        _ => f.length * 3,
    };
    json!({
        "aggregatable": f.ty.groupable() || f.ty.is_numeric(),
        "aiPredictionField": false,
        "autoNumber": f.auto_number,
        "byteLength": byte_length,
        "calculated": f.calculated,
        "calculatedFormula": null,
        "cascadeDelete": false,
        "caseSensitive": false,
        "compoundFieldName": null,
        "controllerName": null,
        "createable": f.createable,
        "custom": f.custom,
        "defaultValue": f.default_pick.clone().map(Value::String).unwrap_or(if f.ty == Ty::Boolean { json!(false) } else { Value::Null }),
        "defaultValueFormula": null,
        "defaultedOnCreate": f.defaulted,
        "dependentPicklist": false,
        "deprecatedAndHidden": false,
        "digits": f.digits,
        "displayLocationInDecimal": false,
        "encrypted": false,
        "externalId": f.external_id,
        "extraTypeInfo": match f.ty { Ty::TextArea if f.length <= 255 => json!("plaintextarea"), Ty::TextArea => json!("plaintextarea"), Ty::Url => json!("url"), _ => Value::Null },
        "filterable": f.ty.filterable(),
        "filteredLookupInfo": null,
        "formulaTreatNullNumberAsZero": false,
        "groupable": f.ty.groupable(),
        "highScaleNumber": false,
        "htmlFormatted": false,
        "idLookup": f.id_lookup(),
        "inlineHelpText": f.help,
        "label": f.label,
        "length": f.length,
        "mask": null,
        "maskType": null,
        "name": f.name,
        "nameField": f.name_field,
        "namePointing": f.refs.len() > 1,
        "nillable": f.nillable,
        "permissionable": f.nillable && !f.calculated && f.ty != Ty::Id,
        "picklistValues": picklist,
        "polymorphicForeignKey": f.refs.len() > 1,
        "precision": f.precision,
        "queryByDistance": false,
        "referenceTargetField": null,
        "referenceTo": f.refs,
        "relationshipName": f.rel,
        "relationshipOrder": null,
        "restrictedDelete": false,
        "restrictedPicklist": false,
        "scale": f.scale,
        "searchPrefilterable": f.ty == Ty::Reference,
        "soapType": f.ty.soap_type(),
        "sortable": f.ty.sortable(),
        "type": f.ty.api_name(),
        "unique": f.unique,
        "updateable": f.updateable,
        "writeRequiresMasterRead": false,
    })
}

/// `GET /sobjects/{name}/describe`.
pub fn describe(o: &Object, version: &str, tooling: bool) -> Value {
    let mut m = flags(o);
    let b = format!("{}/sobjects/{}", base(version, tooling), o.name);
    m.insert("actionOverrides".into(), json!([]));
    m.insert(
        "childRelationships".into(),
        Value::Array(
            o.children
                .iter()
                .map(|c| {
                    json!({
                        "cascadeDelete": c.cascade,
                        "childSObject": c.child,
                        "deprecatedAndHidden": false,
                        "field": c.field,
                        "junctionIdListNames": [],
                        "junctionReferenceTo": [],
                        "relationshipName": c.name,
                        "restrictedDelete": false,
                    })
                })
                .collect(),
        ),
    );
    m.insert("compactLayoutable".into(), json!(o.searchable));
    m.insert("defaultImplementation".into(), Value::Null);
    m.insert("extendedBy".into(), Value::Null);
    m.insert("extendsInterfaces".into(), Value::Null);
    m.insert(
        "fields".into(),
        Value::Array(o.fields.iter().map(describe_field).collect()),
    );
    m.insert("implementedBy".into(), Value::Null);
    m.insert("implementsInterfaces".into(), Value::Null);
    m.insert("listviewable".into(), Value::Null);
    m.insert("lookupLayoutable".into(), Value::Null);
    m.insert("namedLayoutInfos".into(), json!([]));
    m.insert("networkScopeFieldName".into(), Value::Null);
    m.insert(
        "recordTypeInfos".into(),
        if o.searchable {
            json!([{
                "active": true,
                "available": true,
                "defaultRecordTypeMapping": true,
                "developerName": "Master",
                "master": true,
                "name": "Master",
                "recordTypeId": "012000000000000AAA",
                "urls": { "layout": format!("{b}/describe/layouts/012000000000000AAA") }
            }])
        } else {
            json!([])
        },
    );
    m.insert("searchLayoutable".into(), json!(o.searchable));
    m.insert("sobjectDescribeOption".into(), json!("FULL"));
    m.insert(
        "supportedScopes".into(),
        if o.fields.iter().any(|f| f.name == "OwnerId") {
            json!([
                { "label": format!("All {}", o.plural.to_lowercase()), "name": "everything" },
                { "label": format!("My {}", o.plural.to_lowercase()), "name": "mine" }
            ])
        } else {
            json!([{ "label": format!("All {}", o.plural.to_lowercase()), "name": "everything" }])
        },
    );
    m.insert(
        "urls".into(),
        json!({
            "compactLayouts": format!("{b}/describe/compactLayouts"),
            "rowTemplate": format!("{b}/{{ID}}"),
            "approvalLayouts": format!("{b}/describe/approvalLayouts"),
            "uiDetailTemplate": "https://toolbench.my.salesforce.com/{ID}",
            "uiEditTemplate": "https://toolbench.my.salesforce.com/{ID}/e",
            "defaultValues": format!("{b}/defaultValues?recordTypeId&fields"),
            "listviews": format!("{b}/listviews"),
            "describe": format!("{b}/describe"),
            "uiNewRecord": format!("https://toolbench.my.salesforce.com/{}/e", o.prefix),
            "quickActions": format!("{b}/quickActions"),
            "layouts": format!("{b}/describe/layouts"),
            "sobject": b,
        }),
    );
    let mut sorted = Map::new();
    let mut keys: Vec<String> = m.keys().cloned().collect();
    keys.sort();
    for k in keys {
        sorted.insert(k.clone(), m[&k].clone());
    }
    Value::Object(sorted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_objects_have_relationships() {
        let schema = Schema::load(&Store::default());
        let account = schema.get("account").unwrap();
        assert_eq!(account.prefix, "001");
        assert!(account.child("Contacts").is_some());
        assert!(account.child("Opportunities").is_some());
        assert!(account.child("Tasks").is_some());
        let contact = schema.get("Contact").unwrap();
        assert_eq!(contact.relationship("account").unwrap().name, "AccountId");
        assert_eq!(schema.by_prefix("0058c00000ABCDE").unwrap().name, "User");
    }
}
