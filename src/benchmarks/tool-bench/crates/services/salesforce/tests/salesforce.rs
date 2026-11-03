use std::path::Path;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use mock_core::{
    dataset::Dataset,
    serve::service_app,
    world::{SharedWorld, WorldState, shared},
};
use mock_salesforce::Salesforce;
use serde_json::{Value, json};
use tower::ServiceExt;

const V: &str = "/services/data/v50.0";
const NORTHGATE: &str = "0015g000008HFvFAAW";
const DANA: &str = "0035g00000p3z7WAAQ";
const KESTREL_RENEWAL: &str = "0065g000006FoSrAAK";
const BLUEFIN_CASE: &str = "5005g00000spz8MAAQ";

fn dataset() -> Dataset {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
    Dataset::load(&dir).unwrap()
}

fn world() -> SharedWorld {
    shared(WorldState::from_dataset(dataset()).unwrap())
}

fn app(w: &SharedWorld) -> Router {
    service_app(w.clone(), &Salesforce)
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }
}

async fn send(app: &Router, method: &str, uri: &str, body: &str, extra: &[(&str, &str)]) -> Resp {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer 00Dtoolbench!AQ0AQtoolbench")
        .header("content-type", "application/json");
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    Resp {
        status: parts.status,
        headers: parts.headers,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

async fn get(app: &Router, uri: &str) -> Resp {
    send(app, "GET", uri, "", &[]).await
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

async fn query(app: &Router, soql: &str) -> Resp {
    get(app, &format!("{V}/query?q={}", enc(soql))).await
}

fn names(body: &Value) -> Vec<String> {
    body["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["Name"].as_str().unwrap_or("").to_owned())
        .collect()
}

fn state(w: &SharedWorld) -> Value {
    w.lock().unwrap().snapshot()
}

// ----------------------------------------------------------------------------------------
// Discovery and auth
// ----------------------------------------------------------------------------------------

#[tokio::test]
async fn versions_resources_and_auth() {
    let w = world();
    let app = app(&w);
    let r = get(&app, "/services/data").await;
    assert_eq!(r.status, StatusCode::OK);
    let versions = r.json();
    assert!(
        versions
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["version"] == "50.0" && v["label"] == "Winter '21")
    );
    assert!(
        versions
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["version"] == "58.0" && v["label"] == "Summer '23")
    );

    let r = get(&app, V).await;
    assert_eq!(r.json()["sobjects"], "/services/data/v50.0/sobjects");

    let r = get(&app, "/services/data/v12.0/sobjects").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(
        r.json(),
        json!([{"message": "The requested resource does not exist", "errorCode": "NOT_FOUND", "fields": []}])
    );

    let req = Request::builder()
        .uri(format!("{V}/sobjects"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body[0]["errorCode"], "INVALID_SESSION_ID");
}

#[tokio::test]
async fn global_and_object_describe() {
    let w = world();
    let app = app(&w);
    let r = get(&app, &format!("{V}/sobjects")).await;
    let g = r.json();
    assert_eq!(g["maxBatchSize"], 200);
    let account = g["sobjects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["name"] == "Account")
        .unwrap();
    assert_eq!(account["keyPrefix"], "001");
    assert_eq!(
        account["urls"]["describe"],
        "/services/data/v50.0/sobjects/Account/describe"
    );
    assert!(
        !g["sobjects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["name"] == "TraceFlag")
    );

    let d = get(&app, &format!("{V}/sobjects/Opportunity/describe"))
        .await
        .json();
    assert_eq!(d["name"], "Opportunity");
    let stage = d["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "StageName")
        .unwrap();
    assert_eq!(stage["type"], "picklist");
    assert_eq!(stage["nillable"], false);
    assert!(
        stage["picklistValues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["value"] == "Closed Won")
    );
    let acct = d["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "AccountId")
        .unwrap();
    assert_eq!(acct["referenceTo"], json!(["Account"]));
    assert_eq!(acct["relationshipName"], "Account");

    let a = get(&app, &format!("{V}/sobjects/Account/describe"))
        .await
        .json();
    assert!(
        a["childRelationships"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["relationshipName"] == "Contacts" && c["childSObject"] == "Contact")
    );

    let missing = get(&app, &format!("{V}/sobjects/Nope__c/describe")).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.json()[0]["errorCode"], "NOT_FOUND");

    let info = get(&app, &format!("{V}/sobjects/Account")).await.json();
    assert_eq!(info["objectDescribe"]["name"], "Account");
    assert_eq!(info["recentItems"].as_array().unwrap().len(), 5);

    let tooling = get(&app, &format!("{V}/tooling/sobjects")).await.json();
    assert!(
        tooling["sobjects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["name"] == "TraceFlag")
    );
}

// ----------------------------------------------------------------------------------------
// SOQL
// ----------------------------------------------------------------------------------------

#[tokio::test]
async fn query_relationships_and_subqueries() {
    let w = world();
    let app = app(&w);
    let r = query(&app, "SELECT Name, Email, Account.Name, Account.Owner.Name FROM Contact WHERE LastName = 'whitfield' ORDER BY Name, Email").await;
    assert_eq!(r.status, StatusCode::OK);
    let body = r.json();
    assert_eq!(body["totalSize"], 3);
    assert_eq!(body["done"], true);
    let first = &body["records"][0];
    assert_eq!(first["attributes"]["type"], "Contact");
    assert!(
        first["attributes"]["url"]
            .as_str()
            .unwrap()
            .starts_with("/services/data/v50.0/sobjects/Contact/003")
    );
    assert_eq!(first["Name"], "Dan Whitfield");
    assert_eq!(first["Account"]["Name"], "Northgate Analytics");
    assert_eq!(first["Account"]["Owner"]["Name"], "Priya Raman");
    assert_eq!(first["Account"]["attributes"]["type"], "Account");

    let r = query(&app, "SELECT Name, (SELECT LastName FROM Contacts ORDER BY LastName), (SELECT Name FROM Opportunities WHERE IsClosed = false) FROM Account WHERE Name = 'Northgate Analytics'").await;
    let acct = &r.json()["records"][0];
    assert_eq!(acct["Contacts"]["totalSize"], 3);
    assert_eq!(acct["Contacts"]["records"][0]["LastName"], "Haddad");
    assert_eq!(acct["Opportunities"]["totalSize"], 3);

    let r = query(
        &app,
        "SELECT Name, (SELECT Id FROM Cases) FROM Account WHERE Name = 'Meridian Health Partners'",
    )
    .await;
    assert_eq!(r.json()["records"][0]["Cases"], Value::Null);

    let r = query(&app, "SELECT Subject, Who.Name, What.Name, What.Type FROM Task WHERE Subject = 'QBR prep with Dana'").await;
    let t = &r.json()["records"][0];
    assert_eq!(t["Who"]["Name"], "Dana Whitfield");
    assert_eq!(t["What"]["Type"], "Account");

    let r = query(
        &app,
        "SELECT Name, Parent.Name FROM Account WHERE ParentId != null",
    )
    .await;
    assert_eq!(
        r.json()["records"][0]["Parent"]["Name"],
        "Kestrel Aerospace"
    );
    let r = query(
        &app,
        "SELECT Name, Parent.Name FROM Account WHERE Name = 'Lumen Retail Group'",
    )
    .await;
    assert_eq!(r.json()["records"][0]["Parent"], Value::Null);
}

#[tokio::test]
async fn query_filters_and_ordering() {
    let w = world();
    let app = app(&w);
    let b = query(&app, "SELECT Name FROM Account WHERE BillingState IN ('WA', 'CO') AND (Type = 'Customer - Direct' OR Type = 'Customer - Channel') ORDER BY Name DESC").await.json();
    assert_eq!(
        names(&b),
        [
            "Kestrel Aerospace",
            "Bluefin Marine Supply",
            "Bluefin Logistics"
        ]
    );

    let b = query(&app, "SELECT Name FROM Account WHERE Name LIKE 'kestrel%' AND NOT Name LIKE '%UK%' ORDER BY Name").await.json();
    assert_eq!(names(&b), ["Kestrel Aerospace", "Kestrel Analytics"]);

    let b = query(&app, "SELECT Name FROM Account WHERE BillingState NOT IN ('WA', 'CO', 'IL', 'MA', 'TX') ORDER BY BillingState NULLS FIRST, Name").await.json();
    assert_eq!(names(&b)[0], "Kestrel Aerospace UK Ltd");

    let b = query(&app, "SELECT Name, Amount FROM Opportunity WHERE CloseDate = THIS_QUARTER AND IsClosed = false ORDER BY Amount DESC NULLS LAST LIMIT 3 OFFSET 1").await.json();
    assert_eq!(b["totalSize"], 3);
    assert_eq!(
        names(&b),
        [
            "Northgate Analytics - Platform Renewal 2026",
            "Bluefin Marine Supply - Store Dashboards",
            "Cedar & Pine Clinics - Patient Analytics"
        ]
    );

    let b = query(&app, "SELECT Name FROM Opportunity WHERE Amount = null")
        .await
        .json();
    assert_eq!(names(&b), ["Meridian Health Partners - Discovery"]);

    let b = query(&app, "SELECT Name FROM Opportunity WHERE CloseDate < 2026-01-01 AND CreatedDate >= 2025-01-01T00:00:00Z").await.json();
    assert_eq!(names(&b), ["Kestrel Aerospace - Renewal FY26"]);

    let b = query(
        &app,
        "SELECT Name FROM Lead WHERE CreatedDate = LAST_N_DAYS:30 ORDER BY CreatedDate",
    )
    .await
    .json();
    assert_eq!(names(&b), ["Olivia Grant", "Kenji Mori"]);

    let b = query(
        &app,
        "SELECT COUNT() FROM Contact WHERE Account.Name = 'Northgate Analytics'",
    )
    .await
    .json();
    assert_eq!(b, json!({"totalSize": 3, "done": true, "records": []}));

    let b = query(&app, "SELECT Name FROM Account WHERE Id IN (SELECT AccountId FROM Case WHERE Priority = 'High') ORDER BY Name").await.json();
    assert_eq!(names(&b), ["Bluefin Logistics", "Cedar & Pine Clinics"]);

    let b = query(
        &app,
        &format!("SELECT Name FROM Contact WHERE Id = '{}'", &DANA[..15]),
    )
    .await
    .json();
    assert_eq!(names(&b), ["Dana Whitfield"]);

    let b = query(
        &app,
        "SELECT FIELDS(STANDARD) FROM Case WHERE CaseNumber = '00001041'",
    )
    .await
    .json();
    assert_eq!(
        b["records"][0]["Subject"],
        "Label printer sync failing after warehouse upgrade"
    );
    assert!(b["records"][0].get("SystemModstamp").is_some());
}

#[tokio::test]
async fn query_aggregates() {
    let w = world();
    let app = app(&w);
    let b = query(&app, "SELECT Owner.Name, SUM(Amount) total, COUNT(Id) FROM Opportunity WHERE IsClosed = false AND CloseDate = THIS_QUARTER GROUP BY Owner.Name ORDER BY SUM(Amount) DESC").await.json();
    assert_eq!(
        b["records"][0],
        json!({"attributes": {"type": "AggregateResult"}, "Name": "Priya Raman", "total": 436500.0, "expr0": 4})
    );
    assert_eq!(b["totalSize"], 4);
    assert_eq!(b["records"][3]["total"], Value::Null);

    let b = query(&app, "SELECT StageName, COUNT(Id) n FROM Opportunity GROUP BY StageName HAVING COUNT(Id) > 2 ORDER BY StageName").await.json();
    assert_eq!(
        b["records"],
        json!([
            {"attributes": {"type": "AggregateResult"}, "StageName": "Closed Won", "n": 3},
            {"attributes": {"type": "AggregateResult"}, "StageName": "Negotiation/Review", "n": 4},
            {"attributes": {"type": "AggregateResult"}, "StageName": "Proposal/Price Quote", "n": 3}
        ])
    );

    let b = query(&app, "SELECT CALENDAR_YEAR(CloseDate), CALENDAR_QUARTER(CloseDate) q, SUM(Amount) FROM Opportunity WHERE IsWon = true GROUP BY CALENDAR_YEAR(CloseDate), CALENDAR_QUARTER(CloseDate) ORDER BY CALENDAR_YEAR(CloseDate)").await.json();
    assert_eq!(b["records"][0]["expr0"], 2025);
    assert_eq!(b["records"][0]["expr1"], 168000.0);
    assert_eq!(b["records"][1]["q"], 2);

    let b = query(&app, "SELECT MAX(Amount), MIN(CloseDate), AVG(Amount), COUNT_DISTINCT(OwnerId) FROM Opportunity WHERE IsClosed = true").await.json();
    let rec = &b["records"][0];
    assert_eq!(rec["expr0"], 280000.0);
    assert_eq!(rec["expr1"], "2025-06-27");
    assert_eq!(rec["expr3"], 4);

    let e = query(
        &app,
        "SELECT StageName, Name, COUNT(Id) FROM Opportunity GROUP BY StageName",
    )
    .await;
    assert_eq!(e.status, StatusCode::BAD_REQUEST);
    assert_eq!(e.json()[0]["errorCode"], "MALFORMED_QUERY");
    assert!(
        e.json()[0]["message"]
            .as_str()
            .unwrap()
            .ends_with("Field must be grouped or aggregated: Name")
    );

    let e = query(
        &app,
        "SELECT Amount, COUNT(Id) FROM Opportunity GROUP BY Amount",
    )
    .await;
    assert!(
        e.json()[0]["message"]
            .as_str()
            .unwrap()
            .ends_with("field 'Amount' can not be grouped in a query call")
    );
}

#[tokio::test]
async fn query_errors_match_salesforce() {
    let w = world();
    let app = app(&w);
    let e = query(&app, "SELECT Id, Foo FROM Account").await;
    assert_eq!(e.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        e.json(),
        json!([{
            "message": "SELECT Id, Foo FROM Account\n           ^\nERROR at Row:1:Column:12\nNo such column 'Foo' on entity 'Account'. If you are attempting to use a custom field, be sure to append the '__c' after the custom field name. Please reference your WSDL or the describe call for the appropriate names.",
            "errorCode": "INVALID_FIELD",
            "fields": []
        }])
    );
    let e = query(&app, "SELECT Id FROM Acount").await;
    assert_eq!(e.json()[0]["errorCode"], "INVALID_TYPE");
    assert!(
        e.json()[0]["message"]
            .as_str()
            .unwrap()
            .contains("sObject type 'Acount' is not supported.")
    );

    let e = query(
        &app,
        "SELECT Id FROM Account WHERE Name = 'x' AND Type = 'y' OR Rating = 'Hot'",
    )
    .await;
    assert_eq!(e.json()[0]["errorCode"], "MALFORMED_QUERY");
    assert!(
        e.json()[0]["message"]
            .as_str()
            .unwrap()
            .ends_with("unexpected token: 'OR'")
    );

    let e = query(
        &app,
        "SELECT Id FROM Opportunity WHERE CloseDate > '2026-01-01'",
    )
    .await;
    assert_eq!(e.json()[0]["errorCode"], "INVALID_FIELD");
    assert!(e.json()[0]["message"].as_str().unwrap().ends_with("value of filter criterion for field 'CloseDate' must be of type date and should not be enclosed in quotes"));

    let e = query(&app, "SELECT Id FROM Account WHERE Id = 'nope'").await;
    assert_eq!(e.json()[0]["errorCode"], "INVALID_QUERY_FILTER_OPERATOR");

    let e = query(&app, "SELECT Id FROM Account WHERE Description = 'x'").await;
    assert!(
        e.json()[0]["message"]
            .as_str()
            .unwrap()
            .ends_with("field 'Description' can not be filtered in a query call")
    );

    let e = query(&app, "SELECT Id FROM TraceFlag").await;
    assert_eq!(e.json()[0]["errorCode"], "INVALID_TYPE");

    let e = get(&app, &format!("{V}/query")).await;
    assert_eq!(e.json()[0]["errorCode"], "MALFORMED_QUERY");
}

#[tokio::test]
async fn query_all_includes_deleted_rows() {
    let w = world();
    let app = app(&w);
    let soql = "SELECT Name, IsDeleted FROM Opportunity WHERE Account.Name = 'Lumen Retail Group' ORDER BY Name";
    let live = query(&app, soql).await.json();
    assert_eq!(live["totalSize"], 2);
    let all = get(&app, &format!("{V}/queryAll?q={}", enc(soql)))
        .await
        .json();
    assert_eq!(all["totalSize"], 3);
    assert!(
        all["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["IsDeleted"] == true)
    );
}

#[tokio::test]
async fn query_more_pages_with_batch_size() {
    let mut ds = dataset();
    let leads = ds
        .services
        .get_mut("salesforce")
        .unwrap()
        .collection_mut("Lead");
    for i in 0..450u32 {
        let id = mock_salesforce::util::id18(&format!("00Q5g0000{:06}", 100_000 + i));
        leads.insert(id.clone(), json!({"Id": id, "IsDeleted": false, "FirstName": "Bulk", "LastName": format!("Lead {i:03}"), "Company": "Import Co", "Status": "Open - Not Contacted"}));
    }
    let w = shared(WorldState::from_dataset(ds).unwrap());
    let app = app(&w);
    let uri = format!(
        "{V}/query?q={}",
        enc("SELECT LastName FROM Lead WHERE Company = 'Import Co' ORDER BY LastName")
    );
    let p1 = send(
        &app,
        "GET",
        &uri,
        "",
        &[("Sforce-Query-Options", "batchSize=200")],
    )
    .await
    .json();
    assert_eq!(p1["totalSize"], 450);
    assert_eq!(p1["done"], false);
    assert_eq!(p1["records"].as_array().unwrap().len(), 200);
    let next = p1["nextRecordsUrl"].as_str().unwrap().to_owned();
    assert!(next.starts_with("/services/data/v50.0/query/01g"));
    assert!(next.ends_with("-200"));
    let p2 = send(
        &app,
        "GET",
        &next,
        "",
        &[("Sforce-Query-Options", "batchSize=200")],
    )
    .await
    .json();
    assert_eq!(p2["records"][0]["LastName"], "Lead 200");
    let p3 = send(
        &app,
        "GET",
        p2["nextRecordsUrl"].as_str().unwrap(),
        "",
        &[("Sforce-Query-Options", "batchSize=200")],
    )
    .await
    .json();
    assert_eq!(p3["done"], true);
    assert_eq!(p3["records"].as_array().unwrap().len(), 50);
    assert!(p3.get("nextRecordsUrl").is_none());

    let all = get(&app, &uri).await.json();
    assert_eq!(all["records"].as_array().unwrap().len(), 450);
    assert_eq!(all["done"], true);

    let bad = get(&app, &format!("{V}/query/01gZZ-200")).await;
    assert_eq!(bad.json()[0]["errorCode"], "INVALID_QUERY_LOCATOR");
}

// ----------------------------------------------------------------------------------------
// SOSL
// ----------------------------------------------------------------------------------------

#[tokio::test]
async fn search_finds_records_across_objects() {
    let w = world();
    let app = app(&w);
    let sosl = "FIND {Whitfield} IN ALL FIELDS RETURNING Contact(Name, Email WHERE Account.Name = 'Northgate Analytics' ORDER BY Name), Lead(Name)";
    let r = get(&app, &format!("{V}/search?q={}", enc(sosl))).await;
    assert_eq!(r.status, StatusCode::OK);
    let recs = r.json()["searchRecords"].as_array().unwrap().clone();
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0]["attributes"]["type"], "Contact");
    assert_eq!(recs[0]["Name"], "Dan Whitfield");

    let sosl =
        "FIND {kestrel*} IN NAME FIELDS RETURNING Account(Name, BillingCity ORDER BY Name LIMIT 2)";
    let recs = get(&app, &format!("{V}/search?q={}", enc(sosl)))
        .await
        .json()["searchRecords"]
        .clone();
    assert_eq!(recs.as_array().unwrap().len(), 2);
    assert_eq!(recs[1]["BillingCity"], "Bristol");

    let sosl =
        "FIND {\"label printer\"} RETURNING Case(CaseNumber, Subject WHERE IsClosed = false)";
    let recs = get(&app, &format!("{V}/search?q={}", enc(sosl)))
        .await
        .json()["searchRecords"]
        .clone();
    assert_eq!(recs.as_array().unwrap().len(), 2);

    let sosl = "FIND {northgateanalytics.com} IN EMAIL FIELDS RETURNING Contact(Email)";
    let recs = get(&app, &format!("{V}/search?q={}", enc(sosl)))
        .await
        .json()["searchRecords"]
        .clone();
    assert_eq!(recs.as_array().unwrap().len(), 3);

    let bad = get(
        &app,
        &format!("{V}/search?q={}", enc("FIND {a} RETURNING Account")),
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert_eq!(bad.json()[0]["errorCode"], "MALFORMED_SEARCH");
    let bad = get(
        &app,
        &format!("{V}/search?q={}", enc("FIND {acme} RETURNING Widget(Name)")),
    )
    .await;
    assert_eq!(bad.json()[0]["errorCode"], "INVALID_TYPE");
}

// ----------------------------------------------------------------------------------------
// sObject CRUD
// ----------------------------------------------------------------------------------------

#[tokio::test]
async fn create_get_update_delete_record() {
    let w = world();
    let app = app(&w);
    let r = send(&app, "POST", &format!("{V}/sobjects/Contact"), &format!(r#"{{"FirstName":"Mina","LastName":"Patel","AccountId":"{NORTHGATE}","Email":"mina.patel@northgateanalytics.com"}}"#), &[]).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let created = r.json();
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created, json!({"id": id, "success": true, "errors": []}));
    assert_eq!(id, "0038c0000000001AAA");
    assert_eq!(
        r.headers["location"],
        format!("/services/data/v50.0/sobjects/Contact/{id}")
    );

    let rec = get(&app, &format!("{V}/sobjects/Contact/{id}"))
        .await
        .json();
    assert_eq!(rec["Name"], "Mina Patel");
    assert_eq!(rec["CreatedDate"], "2026-05-12T15:00:07.000+0000");
    assert_eq!(rec["OwnerId"], "0055g00000yTKUGAA4");
    assert_eq!(rec["IsDeleted"], false);
    let some = get(
        &app,
        &format!("{V}/sobjects/Contact/{id}?fields=Name,Email"),
    )
    .await
    .json();
    assert_eq!(some.as_object().unwrap().len(), 3);

    let r = send(
        &app,
        "PATCH",
        &format!("{V}/sobjects/Contact/{}", &id[..15]),
        r#"{"Title":"Data Engineer"}"#,
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let s = state(&w);
    assert_eq!(
        s["services"]["salesforce"]["Contact"][&id]["Title"],
        "Data Engineer"
    );
    assert_eq!(
        s["services"]["salesforce"]["Contact"][&id]["LastModifiedDate"],
        "2026-05-12T15:00:14.000+0000"
    );

    let r = send(
        &app,
        "DELETE",
        &format!("{V}/sobjects/Contact/{id}"),
        "",
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        state(&w)["services"]["salesforce"]["Contact"][&id]["IsDeleted"],
        true
    );
    let gone = get(&app, &format!("{V}/sobjects/Contact/{id}")).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    assert_eq!(gone.json()[0]["errorCode"], "ENTITY_IS_DELETED");

    let missing = get(&app, &format!("{V}/sobjects/Contact/0035g00000zzzzzAAA")).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.json()[0]["errorCode"], "NOT_FOUND");
}

#[tokio::test]
async fn write_validation_errors() {
    let w = world();
    let app = app(&w);
    let cases = [
        (
            r#"{"LastName":"X","Foo":"bar"}"#,
            "INVALID_FIELD",
            "No such column 'Foo' on sobject of type Contact",
        ),
        (
            r#"{"FirstName":"X"}"#,
            "REQUIRED_FIELD_MISSING",
            "Required fields are missing: [LastName]",
        ),
        (
            r#"{"LastName":"X","Name":"Y"}"#,
            "INVALID_FIELD_FOR_INSERT_UPDATE",
            "Unable to create/update fields: Name. Please check the security settings of this field and verify that it is read/write for your profile or permission set.",
        ),
        (
            r#"{"LastName":"X","AccountId":"0035g00000p3z7WAAQ"}"#,
            "FIELD_INTEGRITY_EXCEPTION",
            "Account ID: id value of incorrect type: 0035g00000p3z7WAAQ",
        ),
        (
            r#"{"LastName":"X","Birthdate":"last week"}"#,
            "JSON_PARSER_ERROR",
            "Cannot deserialize instance of date from VALUE_STRING value last week or request may be missing a required field",
        ),
    ];
    for (body, code, message) in cases {
        let r = send(&app, "POST", &format!("{V}/sobjects/Contact"), body, &[]).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(r.json()[0]["errorCode"], code, "{body}");
        assert_eq!(r.json()[0]["message"], message, "{body}");
    }
    let r = send(
        &app,
        "POST",
        &format!("{V}/sobjects/Contact"),
        "not json",
        &[],
    )
    .await;
    assert_eq!(r.json()[0]["errorCode"], "JSON_PARSER_ERROR");
    let r = send(&app, "POST", &format!("{V}/sobjects/ApexLog"), "{}", &[]).await;
    assert_eq!(r.json()[0]["errorCode"], "INVALID_TYPE_FOR_OPERATION");
    assert!(
        state(&w)["services"]["salesforce"]["Contact"]
            .as_object()
            .unwrap()
            .len()
            == 10
    );
}

#[tokio::test]
async fn derived_fields_follow_salesforce_rules() {
    let w = world();
    let app = app(&w);
    let r = send(
        &app,
        "PATCH",
        &format!("{V}/sobjects/Opportunity/{KESTREL_RENEWAL}"),
        r#"{"StageName":"Closed Won","CloseDate":"2026-05-12"}"#,
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let opp = state(&w)["services"]["salesforce"]["Opportunity"][KESTREL_RENEWAL].clone();
    assert_eq!(opp["IsWon"], true);
    assert_eq!(opp["IsClosed"], true);
    assert_eq!(opp["Probability"], 100.0);
    assert_eq!(opp["ForecastCategoryName"], "Closed");
    assert_eq!(opp["ExpectedRevenue"], 184000.0);

    send(
        &app,
        "PATCH",
        &format!("{V}/sobjects/Case/{BLUEFIN_CASE}"),
        r#"{"Status":"Closed"}"#,
        &[],
    )
    .await;
    let c = state(&w)["services"]["salesforce"]["Case"][BLUEFIN_CASE].clone();
    assert_eq!(c["IsClosed"], true);
    assert_eq!(c["ClosedDate"], "2026-05-12T15:00:14.000+0000");

    let r = send(&app, "POST", &format!("{V}/sobjects/Task"), &format!(r#"{{"Subject":"Call","WhoId":"{DANA}","Status":"Completed","ActivityDate":"2026-05-12"}}"#), &[]).await;
    let id = r.json()["id"].as_str().unwrap().to_owned();
    let s = state(&w);
    let t = &s["services"]["salesforce"]["Task"][&id];
    assert_eq!(t["AccountId"], NORTHGATE);
    assert_eq!(t["Priority"], "Normal");
    assert_eq!(t["IsClosed"], true);
    assert_eq!(
        s["services"]["salesforce"]["Contact"][DANA]["LastActivityDate"],
        "2026-05-12"
    );

    let r = send(
        &app,
        "POST",
        &format!("{V}/sobjects/Case"),
        r#"{"Subject":"New one"}"#,
        &[],
    )
    .await;
    let id = r.json()["id"].as_str().unwrap().to_owned();
    assert_eq!(
        state(&w)["services"]["salesforce"]["Case"][&id]["CaseNumber"],
        "00001045"
    );
    assert_eq!(
        state(&w)["services"]["salesforce"]["Case"][&id]["Status"],
        "New"
    );
}

#[tokio::test]
async fn composite_collections_and_all_or_none() {
    let w = world();
    let app = app(&w);
    let body = json!({"allOrNone": false, "records": [
        {"attributes": {"type": "Lead"}, "LastName": "Ng", "Company": "Ng Labs"},
        {"attributes": {"type": "Lead"}, "LastName": "NoCompany"}
    ]});
    let r = send(
        &app,
        "POST",
        &format!("{V}/composite/sobjects"),
        &body.to_string(),
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let res = r.json();
    assert_eq!(res[0]["success"], true);
    assert_eq!(
        res[1],
        json!({"success": false, "errors": [{"statusCode": "REQUIRED_FIELD_MISSING", "message": "Required fields are missing: [Company]", "fields": ["Company"]}]})
    );
    let lead_id = res[0]["id"].as_str().unwrap().to_owned();

    let body = json!({"allOrNone": true, "records": [
        {"attributes": {"type": "Lead"}, "id": lead_id, "Status": "Working - Contacted"},
        {"attributes": {"type": "Lead"}, "id": "00Q5g00000zzzzzEAA", "Status": "Working - Contacted"}
    ]});
    let res = send(
        &app,
        "PATCH",
        &format!("{V}/composite/sobjects"),
        &body.to_string(),
        &[],
    )
    .await
    .json();
    assert_eq!(
        res[0]["errors"][0]["statusCode"],
        "ALL_OR_NONE_OPERATION_ROLLED_BACK"
    );
    assert_eq!(
        res[1]["errors"][0]["statusCode"],
        "INVALID_CROSS_REFERENCE_KEY"
    );
    assert_eq!(
        state(&w)["services"]["salesforce"]["Lead"][&lead_id]["Status"],
        "Open - Not Contacted"
    );

    let body = json!({"records": [{"attributes": {"type": "Lead"}, "id": lead_id, "Status": "Working - Contacted"}]});
    let res = send(
        &app,
        "PATCH",
        &format!("{V}/composite/sobjects"),
        &body.to_string(),
        &[],
    )
    .await
    .json();
    assert_eq!(res, json!([{"id": lead_id, "success": true, "errors": []}]));
    assert_eq!(
        state(&w)["services"]["salesforce"]["Lead"][&lead_id]["Status"],
        "Working - Contacted"
    );

    let got = send(
        &app,
        "POST",
        &format!("{V}/composite/sobjects/Lead"),
        &json!({"ids": [lead_id, "00Q5g00000zzzzzEAA"], "fields": ["Name", "Status"]}).to_string(),
        &[],
    )
    .await
    .json();
    assert_eq!(got[0]["Name"], "Ng");
    assert_eq!(got[1], Value::Null);

    let res = send(
        &app,
        "DELETE",
        &format!("{V}/composite/sobjects?ids={lead_id},bad"),
        "",
        &[],
    )
    .await
    .json();
    assert_eq!(
        res[0],
        json!({"id": lead_id, "success": true, "errors": []})
    );
    assert_eq!(res[1]["errors"][0]["statusCode"], "MALFORMED_ID");
    assert_eq!(
        state(&w)["services"]["salesforce"]["Lead"][&lead_id]["IsDeleted"],
        true
    );
}

#[tokio::test]
async fn deleting_an_account_cascades_to_contacts_and_opportunities() {
    let w = world();
    let app = app(&w);
    let r = send(
        &app,
        "DELETE",
        &format!("{V}/sobjects/Account/{NORTHGATE}"),
        "",
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let s = state(&w);
    assert_eq!(
        s["services"]["salesforce"]["Contact"][DANA]["IsDeleted"],
        true
    );
    let live = query(
        &app,
        "SELECT COUNT() FROM Opportunity WHERE AccountId = '0015g000008HFvFAAW'",
    )
    .await
    .json();
    assert_eq!(live["totalSize"], 0);
}

// ----------------------------------------------------------------------------------------
// Metadata API and upsert on a custom external Id
// ----------------------------------------------------------------------------------------

fn soap(op: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/" xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><soapenv:Header xmlns="http://soap.sforce.com/2006/04/metadata"><SessionHeader><sessionId>00Dtoolbench!AQ0AQtoolbench</sessionId></SessionHeader></soapenv:Header><soapenv:Body xmlns="http://soap.sforce.com/2006/04/metadata">{op}</soapenv:Body></soapenv:Envelope>"#
    )
}

#[tokio::test]
async fn metadata_api_creates_fields_and_objects() {
    let w = world();
    let app = app(&w);
    let create = soap(
        r#"<createMetadata><metadata xsi:type="CustomField"><fullName>Account.Erp_Id__c</fullName><label>ERP Id</label><type>Text</type><length>40</length><externalId>true</externalId><unique>true</unique></metadata></createMetadata>"#,
    );
    let r = send(
        &app,
        "POST",
        "/services/Soap/m/50.0",
        &create,
        &[("content-type", "text/xml")],
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/xml")
    );
    assert!(r.text.contains("<createMetadataResponse><result><fullName>Account.Erp_Id__c</fullName><success>true</success></result></createMetadataResponse>"));

    let dup = send(&app, "POST", "/services/Soap/m/50.0", &create, &[]).await;
    assert!(
        dup.text
            .contains("<statusCode>DUPLICATE_DEVELOPER_NAME</statusCode>")
    );
    assert!(dup.text.contains("<success>false</success>"));

    let d = get(&app, &format!("{V}/sobjects/Account/describe"))
        .await
        .json();
    let f = d["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "Erp_Id__c")
        .unwrap()
        .clone();
    assert_eq!(f["custom"], true);
    assert_eq!(f["externalId"], true);
    assert_eq!(f["length"], 40);

    // Upsert by the new external Id: create, then update.
    let r = send(
        &app,
        "PATCH",
        &format!("{V}/sobjects/Account/Erp_Id__c/ERP-77"),
        r#"{"Name":"Granite Peak Outfitters"}"#,
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json()["created"], true);
    let id = r.json()["id"].as_str().unwrap().to_owned();
    let r = send(
        &app,
        "PATCH",
        &format!("{V}/sobjects/Account/Erp_Id__c/ERP-77"),
        r#"{"Industry":"Retail"}"#,
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.json(),
        json!({"id": id, "success": true, "errors": [], "created": false})
    );
    let b = query(
        &app,
        "SELECT Name, Industry, Erp_Id__c FROM Account WHERE Erp_Id__c = 'ERP-77'",
    )
    .await
    .json();
    assert_eq!(b["records"][0]["Industry"], "Retail");
    let r = send(
        &app,
        "PATCH",
        &format!("{V}/sobjects/Account/Name/Acme"),
        "{}",
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    let obj = soap(
        r#"<createMetadata><metadata xsi:type="CustomObject"><fullName>Customer_Feedback__c</fullName><label>Customer Feedback</label><pluralLabel>Customer Feedback</pluralLabel><nameField><label>Feedback Name</label><type>Text</type></nameField><deploymentStatus>Deployed</deploymentStatus><sharingModel>ReadWrite</sharingModel></metadata></createMetadata>"#,
    );
    let r = send(&app, "POST", "/services/Soap/m/50.0", &obj, &[]).await;
    assert!(r.text.contains("<success>true</success>"));
    let read = send(&app, "POST", "/services/Soap/m/50.0", &soap("<readMetadata><type>CustomObject</type><fullNames>Customer_Feedback__c</fullNames></readMetadata>"), &[]).await;
    assert!(read.text.contains(r#"<records xsi:type="CustomObject"><fullName>Customer_Feedback__c</fullName><label>Customer Feedback</label>"#));
    let none = send(
        &app,
        "POST",
        "/services/Soap/m/50.0",
        &soap(
            "<readMetadata><type>CustomObject</type><fullNames>Nope__c</fullNames></readMetadata>",
        ),
        &[],
    )
    .await;
    assert!(
        none.text
            .contains("<readMetadataResponse><result></result></readMetadataResponse>")
    );

    let r = send(
        &app,
        "POST",
        &format!("{V}/sobjects/Customer_Feedback__c"),
        r#"{"Name":"QBR notes"}"#,
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert!(r.json()["id"].as_str().unwrap().starts_with("a00"));
    let s = state(&w);
    assert_eq!(
        s["services"]["salesforce"]["CustomObject"]["Customer_Feedback__c"]["label"],
        "Customer Feedback"
    );
    assert_eq!(
        s["services"]["salesforce"]["CustomField"]["Account.Erp_Id__c"]["externalId"],
        "true"
    );

    let bad = send(&app, "POST", "/services/Soap/m/50.0", &soap(r#"<createMetadata><metadata xsi:type="CustomField"><fullName>Account.Score__c</fullName><label>Score</label><type>Checkbox</type></metadata></createMetadata>"#), &[]).await;
    assert!(
        bad.text
            .contains("Must specify &apos;defaultValue&apos; for a CustomField of type Checkbox")
    );

    let fault = send(
        &app,
        "POST",
        "/services/Soap/m/50.0",
        &soap("<retrieve><request/></retrieve>"),
        &[],
    )
    .await;
    assert_eq!(fault.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        fault
            .text
            .contains("<soapenv:Fault><faultcode>soapenv:Client</faultcode>")
    );
}

// ----------------------------------------------------------------------------------------
// Tooling API
// ----------------------------------------------------------------------------------------

#[tokio::test]
async fn tooling_apex_and_debug_logs() {
    let w = world();
    let app = app(&w);
    let q = get(
        &app,
        &format!(
            "{V}/tooling/query?q={}",
            enc("SELECT Name, TableEnumOrId FROM ApexTrigger ORDER BY Name")
        ),
    )
    .await
    .json();
    assert_eq!(
        q["records"][0]["attributes"]["url"]
            .as_str()
            .unwrap()
            .split('/')
            .nth(4),
        Some("tooling")
    );
    assert_eq!(q["records"][0]["TableEnumOrId"], "Case");

    let body = json!({"Name": "RenewalMath", "Body": "public class RenewalMath {\n  // uplift\n  public static Decimal up(Decimal a) { return a * 1.05; }\n}", "ApiVersion": "58.0", "Status": "Active"});
    let r = send(
        &app,
        "POST",
        &format!("{V}/tooling/sobjects/ApexClass"),
        &body.to_string(),
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let created = r.json();
    assert_eq!(created["warnings"], json!([]));
    let id = created["id"].as_str().unwrap().to_owned();
    let cls = state(&w)["services"]["salesforce"]["ApexClass"][&id].clone();
    assert_eq!(cls["ApiVersion"], 58.0);
    assert_eq!(cls["IsValid"], true);
    let dup = send(
        &app,
        "POST",
        &format!("{V}/tooling/sobjects/ApexClass"),
        &body.to_string(),
        &[],
    )
    .await;
    assert_eq!(dup.json()[0]["errorCode"], "DUPLICATE_VALUE");
    let data_api = send(
        &app,
        "POST",
        &format!("{V}/sobjects/ApexClass"),
        &body.to_string(),
        &[],
    )
    .await;
    assert_eq!(
        data_api.json()[0]["errorCode"],
        "INVALID_TYPE_FOR_OPERATION"
    );

    // No trace flag yet: anonymous Apex runs without writing a log.
    let anon = get(
        &app,
        &format!(
            "{V}/tooling/executeAnonymous?anonymousBody={}",
            enc("System.debug('hi');")
        ),
    )
    .await
    .json();
    assert_eq!(
        anon,
        json!({"line": -1, "column": -1, "compiled": true, "success": true, "compileProblem": null, "exceptionStackTrace": null, "exceptionMessage": null})
    );
    assert_eq!(
        state(&w)["services"]["salesforce"]["ApexLog"]
            .as_object()
            .unwrap()
            .len(),
        1
    );

    let level = send(
        &app,
        "POST",
        &format!("{V}/tooling/sobjects/DebugLevel"),
        r#"{"DeveloperName":"UserDebug_1","MasterLabel":"User Debug","ApexCode":"FINEST"}"#,
        &[],
    )
    .await
    .json();
    let flag = json!({"TracedEntityId": "0055g00000yTKUGAA4", "DebugLevelId": level["id"], "LogType": "USER_DEBUG", "StartDate": "2026-05-12T15:00:00.000Z", "ExpirationDate": "2026-05-12T15:30:00.000Z"});
    let r = send(
        &app,
        "POST",
        &format!("{V}/tooling/sobjects/TraceFlag"),
        &flag.to_string(),
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);

    let anon = get(
        &app,
        &format!(
            "{V}/tooling/executeAnonymous?anonymousBody={}",
            enc("System.debug('Renewals: ' + 'ok');")
        ),
    )
    .await
    .json();
    assert_eq!(anon["success"], true);
    let logs = get(&app, &format!("{V}/query?q={}", enc("SELECT Id, Operation, LogLength FROM ApexLog ORDER BY LastModifiedDate DESC LIMIT 1"))).await.json();
    let log_id = logs["records"][0]["Id"].as_str().unwrap().to_owned();
    assert_eq!(
        logs["records"][0]["Operation"],
        "/services/data/v50.0/tooling/executeAnonymous/"
    );
    let body = get(
        &app,
        &format!("/services/data/v58.0/tooling/sobjects/ApexLog/{log_id}/Body"),
    )
    .await;
    assert!(
        body.headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    assert!(body.text.starts_with("50.0 APEX_CODE,FINEST;"));
    assert!(body.text.contains("|USER_DEBUG|[1]|DEBUG|Renewals: ok\n"));

    let bad = get(
        &app,
        &format!(
            "{V}/tooling/executeAnonymous?anonymousBody={}",
            enc("System.debug('x'")
        ),
    )
    .await
    .json();
    assert_eq!(bad["compiled"], false);
    assert_eq!(bad["compileProblem"], "Unexpected token '<EOF>'.");

    let r = send(
        &app,
        "DELETE",
        &format!("{V}/tooling/sobjects/TraceFlag/{}", r_id(&w, "TraceFlag")),
        "",
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(
        state(&w)["services"]["salesforce"]["TraceFlag"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

fn r_id(w: &SharedWorld, collection: &str) -> String {
    state(w)["services"]["salesforce"][collection]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone()
}

#[tokio::test]
async fn field_permissions_use_builtin_profiles() {
    let w = world();
    let app = app(&w);
    let profiles = query(&app, "SELECT Id, Name FROM Profile WHERE Name IN ('System Administrator', 'Standard User') ORDER BY Name").await.json();
    assert_eq!(names(&profiles), ["Standard User", "System Administrator"]);
    let admin = profiles["records"][1]["Id"].as_str().unwrap().to_owned();
    let ps = query(&app, &format!("SELECT Id FROM PermissionSet WHERE IsOwnedByProfile = true AND ProfileId = '{admin}' LIMIT 1")).await.json();
    let ps_id = ps["records"][0]["Id"].as_str().unwrap().to_owned();
    let r = send(&app, "POST", &format!("{V}/sobjects/FieldPermissions"), &json!({"ParentId": ps_id, "SobjectType": "Account", "Field": "Account.Rating", "PermissionsRead": true, "PermissionsEdit": false}).to_string(), &[]).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let fp = query(&app, &format!("SELECT Id, Parent.Profile.Name, PermissionsEdit FROM FieldPermissions WHERE ParentId IN (SELECT Id FROM PermissionSet WHERE IsOwnedByProfile = true AND ProfileId = '{admin}') AND Field = 'Account.Rating'")).await.json();
    assert_eq!(
        fp["records"][0]["Parent"]["Profile"]["Name"],
        "System Administrator"
    );
    assert_eq!(fp["records"][0]["PermissionsEdit"], false);
}
