//! Customizations: brands, themes, sign-in/error/sign-out pages, email templates and
//! customizations, custom domains, email domains, and device assurance policies.
//!
//! An org always has a default brand and theme; when the dataset has no `brands`
//! collection they are created on first access.

use axum::http::{Method, StatusCode};
use mock_core::world::WorldState;
use serde_json::{Map, Value, json};

use crate::util::{
    Reply, Req, Res, endpoint_not_found, error, get, href, id_of, list, log_event, merge, new_id,
    not_found, page, put, remove, svc, ts, validation,
};

const DEFAULT_BRAND: &str = "bnd1toolbench0000001";
const DEFAULT_THEME: &str = "thd1toolbench0000001";

pub const EMAIL_TEMPLATES: [&str; 24] = [
    "ADForgotPassword",
    "ADForgotPasswordDenied",
    "ADSelfServiceUnlock",
    "ADUserActivation",
    "AuthenticatorEnrolled",
    "AuthenticatorReset",
    "ChangeEmailConfirmation",
    "EmailChallenge",
    "EmailChangeConfirmation",
    "EmailFactorVerification",
    "ForgotPassword",
    "ForgotPasswordDenied",
    "LDAPForgotPassword",
    "LDAPForgotPasswordDenied",
    "LDAPSelfServiceUnlock",
    "LDAPUserActivation",
    "NewSignOnNotification",
    "OktaVerifyActivation",
    "PasswordChanged",
    "PasswordResetByAdmin",
    "PendingEmailChange",
    "RegistrationActivation",
    "RegistrationEmailVerification",
    "UserActivation",
];

const WIDGET_VERSIONS: [&str; 6] = ["7.21", "7.22", "7.23", "7.24", "7.25", "7.26"];

fn ensure_defaults(w: &mut WorldState) {
    if svc(w).store.collection("brands").is_some() {
        return;
    }
    let brand = json!({
        "id": DEFAULT_BRAND,
        "name": "octo-corp_default",
        "isDefault": true,
        "agreeToCustomPrivacyPolicy": false,
        "customPrivacyPolicyUrl": null,
        "removePoweredByOkta": false,
        "locale": "en",
        "emailDomainId": null,
        "defaultApp": { "appInstanceId": null, "appLinkName": null, "classicApplicationUri": null }
    });
    put(w, "brands", DEFAULT_BRAND, brand);
    if svc(w).store.collection("themes").is_none() {
        put(
            w,
            "themes",
            &format!("{DEFAULT_BRAND}/{DEFAULT_THEME}"),
            default_theme(DEFAULT_THEME),
        );
    }
}

fn default_theme(id: &str) -> Value {
    json!({
        "id": id,
        "logo": "https://ok12static.oktacdn.com/assets/img/logos/okta-logo.47066819ac7db5c13f4c431b2687cef6.png",
        "favicon": "https://ok12static.oktacdn.com/assets/img/favicons/favicon.8a4b1d2c4e3f5a6b.ico",
        "backgroundImage": null,
        "primaryColorHex": "#1662dd",
        "primaryColorContrastHex": "#ffffff",
        "secondaryColorHex": "#ebebed",
        "secondaryColorContrastHex": "#000000",
        "signInPageTouchPointVariant": "OKTA_DEFAULT",
        "endUserDashboardTouchPointVariant": "OKTA_DEFAULT",
        "errorPageTouchPointVariant": "OKTA_DEFAULT",
        "emailTemplateTouchPointVariant": "OKTA_DEFAULT",
        "loadingPageTouchPointVariant": "OKTA_DEFAULT"
    })
}

fn brand(w: &mut WorldState, id: &str) -> Result<Value, Reply> {
    ensure_defaults(w);
    get(w, "brands", id).ok_or_else(|| not_found(id, "Brand"))
}

fn render_brand(b: &Value) -> Value {
    let mut out = b.clone();
    let id = id_of(b);
    out["_links"] = json!({
        "themes": href(&format!("/api/v1/brands/{id}/themes")),
        "self": href(&format!("/api/v1/brands/{id}"))
    });
    out
}

fn render_theme(brand_id: &str, t: &Value) -> Value {
    let mut out = t.clone();
    let id = id_of(t);
    out["_links"] = json!({ "self": href(&format!("/api/v1/brands/{brand_id}/themes/{id}")) });
    out
}

pub fn handle(w: &mut WorldState, req: &Req) -> Res {
    let s: Vec<&str> = req.segs.iter().map(String::as_str).collect();
    match s.first().copied() {
        Some("brands") => brands(w, req, &s[1..]),
        Some("domains") => domains(w, req, &s[1..]),
        Some("email-domains") => email_domains(w, req, &s[1..]),
        Some("device-assurances") => device_assurances(w, req, &s[1..]),
        _ => Err(endpoint_not_found()),
    }
}

fn brands(w: &mut WorldState, req: &Req, s: &[&str]) -> Res {
    let m = &req.method;
    match s {
        [] if m == Method::GET => {
            ensure_defaults(w);
            let q = req.q("q").map(str::to_lowercase);
            let items: Vec<Value> = list(w, "brands")
                .into_iter()
                .map(|(_, b)| b)
                .filter(|b| {
                    q.as_ref().is_none_or(|q| {
                        b["name"]
                            .as_str()
                            .is_some_and(|n| n.to_lowercase().contains(q.as_str()))
                    })
                })
                .map(|b| render_brand(&b))
                .collect();
            Ok(page(req, items, 20, 200, id_of))
        }
        [] if m == Method::POST => {
            ensure_defaults(w);
            let body = req.object()?;
            let Some(name) = body
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
            else {
                return Err(validation(&["name: The field cannot be left blank"]));
            };
            if list(w, "brands").iter().any(|(_, b)| b["name"] == name) {
                return Err(error(
                    StatusCode::CONFLICT,
                    "E0000197",
                    "You can't use this brand name because it's already in use.",
                    &[],
                ));
            }
            let id = new_id(w, "brand", "bnd");
            let theme_id = new_id(w, "theme", "thd");
            let b = json!({
                "id": id,
                "name": name,
                "isDefault": false,
                "agreeToCustomPrivacyPolicy": false,
                "customPrivacyPolicyUrl": null,
                "removePoweredByOkta": false,
                "locale": "en",
                "emailDomainId": null,
                "defaultApp": { "appInstanceId": null, "appLinkName": null, "classicApplicationUri": null }
            });
            put(w, "brands", &id, b.clone());
            put(
                w,
                "themes",
                &format!("{id}/{theme_id}"),
                default_theme(&theme_id),
            );
            log_event(
                w,
                "brand.create",
                "Create brand",
                vec![
                    json!({ "id": id, "type": "Brand", "alternateId": "unknown", "displayName": name, "detailEntry": null }),
                ],
            );
            Ok(Reply::created(render_brand(&b)))
        }
        [id] if m == Method::GET => Ok(Reply::ok(render_brand(&brand(w, id)?))),
        [id] if m == Method::PUT => {
            let mut b = brand(w, id)?;
            let body = req.object()?;
            if body
                .get("name")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                return Err(validation(&["name: The field cannot be left blank"]));
            }
            for (k, v) in body {
                if k != "id" && k != "isDefault" {
                    b[k] = v;
                }
            }
            put(w, "brands", id, b.clone());
            Ok(Reply::ok(render_brand(&b)))
        }
        [id] if m == Method::DELETE => {
            let b = brand(w, id)?;
            if b["isDefault"] == true {
                return Err(error(
                    StatusCode::CONFLICT,
                    "E0000201",
                    "A default brand cannot be deleted.",
                    &[],
                ));
            }
            if list(w, "domains").iter().any(|(_, d)| d["brandId"] == *id) {
                return Err(error(
                    StatusCode::CONFLICT,
                    "E0000202",
                    "A brand associated with a domain cannot be deleted.",
                    &[],
                ));
            }
            remove(w, "brands", id);
            for (k, _) in list(w, "themes") {
                if k.starts_with(&format!("{id}/")) {
                    remove(w, "themes", &k);
                }
            }
            Ok(Reply::no_content())
        }
        [id, "domains"] if m == Method::GET => {
            brand(w, id)?;
            let items: Vec<Value> = list(w, "domains")
                .into_iter()
                .map(|(_, d)| d)
                .filter(|d| d["brandId"] == *id)
                .map(|d| render_domain(&d))
                .collect();
            Ok(Reply::ok(json!({ "domains": items })))
        }
        [id, "themes", rest @ ..] => themes(w, req, id, rest),
        [id, "pages", rest @ ..] => pages(w, req, id, rest),
        [id, "templates", "email", rest @ ..] => templates(w, req, id, rest),
        _ => Err(endpoint_not_found()),
    }
}

fn themes(w: &mut WorldState, req: &Req, bid: &str, s: &[&str]) -> Res {
    brand(w, bid)?;
    let m = &req.method;
    let find = |w: &mut WorldState, tid: &str| {
        get(w, "themes", &format!("{bid}/{tid}")).ok_or_else(|| not_found(tid, "Theme"))
    };
    match s {
        [] if m == Method::GET => {
            let prefix = format!("{bid}/");
            let items: Vec<Value> = list(w, "themes")
                .into_iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(_, t)| render_theme(bid, &t))
                .collect();
            Ok(Reply::ok(Value::Array(items)))
        }
        [tid] if m == Method::GET => Ok(Reply::ok(render_theme(bid, &find(w, tid)?))),
        [tid] if m == Method::PUT => {
            let mut t = find(w, tid)?;
            let body = req.object()?;
            let mut causes = Vec::new();
            for k in [
                "primaryColorHex",
                "secondaryColorHex",
                "primaryColorContrastHex",
                "secondaryColorContrastHex",
            ] {
                if let Some(v) = body.get(k).and_then(Value::as_str)
                    && !(v.len() == 7
                        && v.starts_with('#')
                        && v[1..].chars().all(|c| c.is_ascii_hexdigit()))
                {
                    causes.push(format!("{k}: Invalid hex color code"));
                }
            }
            if !causes.is_empty() {
                let refs: Vec<&str> = causes.iter().map(String::as_str).collect();
                return Err(validation(&refs));
            }
            for (k, v) in body {
                if !matches!(k.as_str(), "id" | "logo" | "favicon" | "backgroundImage") {
                    t[k] = v;
                }
            }
            put(w, "themes", &format!("{bid}/{tid}"), t.clone());
            Ok(Reply::ok(render_theme(bid, &t)))
        }
        [tid, asset] if matches!(*asset, "logo" | "favicon" | "background-image") => {
            let mut t = find(w, tid)?;
            let field = match *asset {
                "logo" => "logo",
                "favicon" => "favicon",
                _ => "backgroundImage",
            };
            if m == Method::POST {
                if req.raw_body.is_empty() {
                    return Err(validation(&["file: The file cannot be empty"]));
                }
                let n = svc(w).ids.next_prefixed("asset", "fs0", 17);
                let url = format!("https://ok12static.oktacdn.com/bc/image/fileStoreRecord?id={n}");
                t[field] = json!(url);
                put(w, "themes", &format!("{bid}/{tid}"), t);
                Ok(Reply::created(json!({ "url": url })))
            } else if m == Method::DELETE {
                t[field] = match field {
                    "backgroundImage" => Value::Null,
                    _ => default_theme(tid)[field].clone(),
                };
                put(w, "themes", &format!("{bid}/{tid}"), t);
                Ok(Reply::no_content())
            } else {
                Err(endpoint_not_found())
            }
        }
        _ => Err(endpoint_not_found()),
    }
}

fn default_page(kind: &str) -> Value {
    match kind {
        "sign-in" => json!({
            "pageContent": "<!DOCTYPE html>\n<html>\n<head>\n  <meta charset=\"utf-8\" />\n  <title>{{orgName}} - {{pageTitle}}</title>\n  {{{SignInWidgetResources}}}\n</head>\n<body>\n  <div id=\"okta-login-container\"></div>\n  <script>\n    var config = OktaUtil.getSignInWidgetConfig();\n    var oktaSignIn = new OktaSignIn(config);\n    oktaSignIn.renderEl({ el: '#okta-login-container' }, OktaUtil.completeLogin);\n  </script>\n</body>\n</html>\n",
            "widgetVersion": "^7",
            "widgetCustomizations": { "widgetGeneration": "G3", "signInLabel": null, "usernameLabel": null, "passwordLabel": null, "showPasswordVisibilityToggle": true, "showUserIdentifier": true },
            "contentSecurityPolicySetting": { "mode": "report_only", "reportUri": null, "srcList": [] }
        }),
        _ => json!({
            "pageContent": "<!DOCTYPE html>\n<html>\n<head>\n  <title>{{orgName}} - {{errorSummary}}</title>\n</head>\n<body>\n  <h1>{{errorSummary}}</h1>\n  <p>{{errorDescription}}</p>\n  <a href=\"{{back}}\">Go back</a>\n</body>\n</html>\n",
            "contentSecurityPolicySetting": { "mode": "report_only", "reportUri": null, "srcList": [] }
        }),
    }
}

fn pages(w: &mut WorldState, req: &Req, bid: &str, s: &[&str]) -> Res {
    brand(w, bid)?;
    let m = &req.method;
    match s {
        ["sign-in", "widget-versions"] if m == Method::GET => Ok(Reply::ok(json!(WIDGET_VERSIONS))),
        ["sign-out", "customized"] if m == Method::GET => Ok(Reply::ok(
            get(w, "sign_out_pages", bid).unwrap_or(json!({ "type": "OKTA_DEFAULT" })),
        )),
        ["sign-out", "customized"] if m == Method::PUT => {
            let body = Value::Object(req.object()?);
            match body["type"].as_str() {
                Some("OKTA_DEFAULT") => {}
                Some("EXTERNALLY_HOSTED")
                    if body["url"]
                        .as_str()
                        .is_some_and(|u| u.starts_with("https://") || u.starts_with("http://")) => {
                }
                Some("EXTERNALLY_HOSTED") => {
                    return Err(validation(&["url: The field cannot be left blank"]));
                }
                _ => return Err(validation(&["type: The field cannot be left blank"])),
            }
            put(w, "sign_out_pages", bid, body.clone());
            Ok(Reply::ok(body))
        }
        [kind] if m == Method::GET && matches!(*kind, "sign-in" | "error") => {
            let base = format!("/api/v1/brands/{bid}/pages/{kind}");
            let mut out = json!({
                "_links": {
                    "default": href(&format!("{base}/default")),
                    "customized": href(&format!("{base}/customized")),
                    "preview": href(&format!("{base}/preview")),
                    "self": href(&base)
                }
            });
            if let Some(expand) = req.q("expand") {
                let mut embedded = Map::new();
                for part in expand.split(',') {
                    let part = part.trim();
                    let value = match part {
                        "default" => Some(default_page(kind)),
                        "customized" | "preview" => {
                            get(w, "pages", &format!("{bid}/{kind}/{part}"))
                        }
                        "customizedUrl" => {
                            Some(json!(format!("https://octo-corp.okta.com/login/{kind}")))
                        }
                        "previewUrl" => Some(json!(format!(
                            "https://octo-corp.okta.com/login/{kind}/preview"
                        ))),
                        _ => None,
                    };
                    if let Some(v) = value {
                        embedded.insert(part.to_owned(), v);
                    }
                }
                out["_embedded"] = Value::Object(embedded);
            }
            Ok(Reply::ok(out))
        }
        [kind, "default"] if m == Method::GET && matches!(*kind, "sign-in" | "error") => {
            Ok(Reply::ok(default_page(kind)))
        }
        [kind, variant]
            if matches!(*kind, "sign-in" | "error")
                && matches!(*variant, "customized" | "preview") =>
        {
            let key = format!("{bid}/{kind}/{variant}");
            if m == Method::GET {
                get(w, "pages", &key)
                    .map(Reply::ok)
                    .ok_or_else(|| not_found(variant, "Page"))
            } else if m == Method::PUT {
                let mut body = Value::Object(req.object()?);
                if body["pageContent"].as_str().is_none_or(str::is_empty) {
                    return Err(validation(&["pageContent: The field cannot be left blank"]));
                }
                if *kind == "sign-in" && body.get("widgetVersion").is_none() {
                    body["widgetVersion"] = json!("^7");
                }
                if body.get("contentSecurityPolicySetting").is_none() {
                    body["contentSecurityPolicySetting"] =
                        json!({ "mode": "report_only", "reportUri": null, "srcList": [] });
                }
                put(w, "pages", &key, body.clone());
                Ok(Reply::ok(body))
            } else if m == Method::DELETE {
                remove(w, "pages", &key);
                Ok(Reply::no_content())
            } else {
                Err(endpoint_not_found())
            }
        }
        _ => Err(endpoint_not_found()),
    }
}

fn template_links(bid: &str, name: &str) -> Value {
    let base = format!("/api/v1/brands/{bid}/templates/email/{name}");
    json!({
        "self": href(&base),
        "settings": href(&format!("{base}/settings")),
        "defaultContent": href(&format!("{base}/default-content")),
        "customizations": href(&format!("{base}/customizations")),
        "test": href(&format!("{base}/test"))
    })
}

fn default_content(name: &str) -> Value {
    let (subject, line) = match name {
        "UserActivation" => (
            "Welcome to Okta!",
            "Your organization is using Okta to manage your web applications. Click the link below to activate your account.",
        ),
        "ForgotPassword" => (
            "Account password reset",
            "A password reset request was made for your Okta account. Click the link below to reset your password.",
        ),
        "PasswordChanged" => (
            "Your password was changed",
            "The password for your Okta account was recently changed.",
        ),
        "EmailChallenge" => (
            "One-time verification code",
            "Enter this code to verify your identity: ${verificationToken}",
        ),
        "NewSignOnNotification" => (
            "New sign-on notification",
            "Your account was just used to sign in from a new device or location.",
        ),
        _ => (
            "Okta notification",
            "This is an automated message from Okta.",
        ),
    };
    json!({
        "subject": subject,
        "body": format!("<!DOCTYPE html><html><body><p>Hi ${{user.profile.firstName}},</p><p>{line}</p><p>${{org.name}}</p></body></html>")
    })
}

fn substitute(text: &str) -> String {
    text.replace("${user.profile.firstName}", "Morgan")
        .replace("${user.profile.lastName}", "Lee")
        .replace("${user.profile.login}", "morgan.lee@octo-corp.example")
        .replace("${org.name}", "Octo Corp")
        .replace("${verificationToken}", "123456")
}

fn templates(w: &mut WorldState, req: &Req, bid: &str, s: &[&str]) -> Res {
    brand(w, bid)?;
    let m = &req.method;
    let check = |name: &str| {
        if EMAIL_TEMPLATES.contains(&name) {
            Ok(())
        } else {
            Err(not_found(name, "EmailTemplate"))
        }
    };
    let custs = |w: &mut WorldState, name: &str| -> Vec<(String, Value)> {
        let prefix = format!("{bid}/{name}/");
        list(w, "email_customizations")
            .into_iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .collect()
    };
    let render_cust = |name: &str, c: &Value| {
        let mut out = c.clone();
        let id = id_of(c);
        let base = format!("/api/v1/brands/{bid}/templates/email/{name}/customizations/{id}");
        out["_links"] = json!({
            "self": href(&base),
            "template": href(&format!("/api/v1/brands/{bid}/templates/email/{name}")),
            "preview": href(&format!("{base}/preview")),
            "test": href(&format!("/api/v1/brands/{bid}/templates/email/{name}/test"))
        });
        out
    };
    match s {
        [] if m == Method::GET => {
            let items: Vec<Value> = EMAIL_TEMPLATES
                .iter()
                .map(|n| json!({ "name": n, "_links": template_links(bid, n) }))
                .collect();
            Ok(page(req, items, 20, 100, |t| {
                t["name"].as_str().unwrap_or_default().to_owned()
            }))
        }
        [name] if m == Method::GET => {
            check(name)?;
            Ok(Reply::ok(
                json!({ "name": name, "_links": template_links(bid, name) }),
            ))
        }
        [name, "default-content"] if m == Method::GET => {
            check(name)?;
            let mut c = default_content(name);
            c["_links"] = json!({ "self": href(&format!("/api/v1/brands/{bid}/templates/email/{name}/default-content")) });
            Ok(Reply::ok(c))
        }
        [name, "default-content", "preview"] if m == Method::GET => {
            check(name)?;
            let c = default_content(name);
            Ok(Reply::ok(json!({
                "subject": substitute(c["subject"].as_str().unwrap_or_default()),
                "body": substitute(c["body"].as_str().unwrap_or_default()),
                "_links": { "self": href(&format!("/api/v1/brands/{bid}/templates/email/{name}/default-content/preview")) }
            })))
        }
        [name, "settings"] if m == Method::GET => {
            check(name)?;
            Ok(Reply::ok(
                get(w, "email_settings", &format!("{bid}/{name}"))
                    .unwrap_or(json!({ "recipients": "ALL_USERS" })),
            ))
        }
        [name, "settings"] if m == Method::PUT => {
            check(name)?;
            let body = Value::Object(req.object()?);
            if !matches!(
                body["recipients"].as_str(),
                Some("ALL_USERS" | "ADMINS_ONLY" | "NO_USERS")
            ) {
                return Err(validation(&["recipients: Invalid value"]));
            }
            put(w, "email_settings", &format!("{bid}/{name}"), body.clone());
            Ok(Reply::ok(body))
        }
        [name, "test"] if m == Method::POST => {
            check(name)?;
            log_event(
                w,
                "system.email.send_test",
                &format!("Send test email for {name}"),
                vec![],
            );
            Ok(Reply::no_content())
        }
        [name, "customizations"] if m == Method::GET => {
            check(name)?;
            let items: Vec<Value> = custs(w, name)
                .iter()
                .map(|(_, c)| render_cust(name, c))
                .collect();
            Ok(page(req, items, 20, 100, id_of))
        }
        [name, "customizations"] if m == Method::POST => {
            check(name)?;
            let body = req.object()?;
            let mut causes = Vec::new();
            for f in ["language", "subject", "body"] {
                if body
                    .get(f)
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                {
                    causes.push(format!("{f}: The field cannot be left blank"));
                }
            }
            if !causes.is_empty() {
                let refs: Vec<&str> = causes.iter().map(String::as_str).collect();
                return Err(validation(&refs));
            }
            let existing = custs(w, name);
            if existing
                .iter()
                .any(|(_, c)| c["language"] == body["language"])
            {
                return Err(error(
                    StatusCode::CONFLICT,
                    "E0000182",
                    "A customization for that language already exists.",
                    &[],
                ));
            }
            let now = ts(w);
            let id = new_id(w, "email_customization", "oel");
            let is_default = existing.is_empty() || body.get("isDefault") == Some(&json!(true));
            if is_default {
                for (k, mut c) in existing {
                    c["isDefault"] = json!(false);
                    put(w, "email_customizations", &k, c);
                }
            }
            let c = json!({
                "id": id,
                "language": body["language"],
                "isDefault": is_default,
                "subject": body["subject"],
                "body": body["body"],
                "created": now,
                "lastUpdated": now
            });
            put(
                w,
                "email_customizations",
                &format!("{bid}/{name}/{id}"),
                c.clone(),
            );
            Ok(Reply::created(render_cust(name, &c)))
        }
        [name, "customizations"] if m == Method::DELETE => {
            check(name)?;
            for (k, _) in custs(w, name) {
                remove(w, "email_customizations", &k);
            }
            Ok(Reply::no_content())
        }
        [name, "customizations", cid, rest @ ..] => {
            check(name)?;
            let key = format!("{bid}/{name}/{cid}");
            let c = get(w, "email_customizations", &key)
                .ok_or_else(|| not_found(cid, "EmailCustomization"))?;
            match rest {
                [] if m == Method::GET => Ok(Reply::ok(render_cust(name, &c))),
                [] if m == Method::PUT => {
                    let body = req.object()?;
                    let mut c = c;
                    for f in ["language", "subject", "body", "isDefault"] {
                        if let Some(v) = body.get(f) {
                            c[f] = v.clone();
                        }
                    }
                    c["lastUpdated"] = json!(ts(w));
                    put(w, "email_customizations", &key, c.clone());
                    Ok(Reply::ok(render_cust(name, &c)))
                }
                [] if m == Method::DELETE => {
                    if c["isDefault"] == true && custs(w, name).len() > 1 {
                        return Err(error(
                            StatusCode::CONFLICT,
                            "E0000184",
                            "The default email customization can't be deleted.",
                            &[],
                        ));
                    }
                    remove(w, "email_customizations", &key);
                    Ok(Reply::no_content())
                }
                ["preview"] if m == Method::GET => Ok(Reply::ok(json!({
                    "subject": substitute(c["subject"].as_str().unwrap_or_default()),
                    "body": substitute(c["body"].as_str().unwrap_or_default()),
                    "_links": { "self": href(&format!("/api/v1/brands/{bid}/templates/email/{name}/customizations/{cid}/preview")) }
                }))),
                _ => Err(endpoint_not_found()),
            }
        }
        _ => Err(endpoint_not_found()),
    }
}

fn render_domain(d: &Value) -> Value {
    let mut out = d.clone();
    let id = id_of(d);
    out["_links"] = json!({
        "self": href(&format!("/api/v1/domains/{id}")),
        "verify": href(&format!("/api/v1/domains/{id}/verify")),
        "certificate": href(&format!("/api/v1/domains/{id}/certificate")),
        "brand": href(&format!("/api/v1/brands/{}", d["brandId"].as_str().unwrap_or(DEFAULT_BRAND)))
    });
    out
}

fn domains(w: &mut WorldState, req: &Req, s: &[&str]) -> Res {
    let m = &req.method;
    let find =
        |w: &mut WorldState, id: &str| get(w, "domains", id).ok_or_else(|| not_found(id, "Domain"));
    match s {
        [] if m == Method::GET => {
            let items: Vec<Value> = list(w, "domains")
                .iter()
                .map(|(_, d)| render_domain(d))
                .collect();
            Ok(Reply::ok(json!({ "domains": items })))
        }
        [] if m == Method::POST => {
            ensure_defaults(w);
            let body = req.object()?;
            let domain = body
                .get("domain")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase();
            if domain.is_empty() || !domain.contains('.') {
                return Err(validation(&["domain: The field cannot be left blank"]));
            }
            let source = body
                .get("certificateSourceType")
                .and_then(Value::as_str)
                .unwrap_or("MANUAL")
                .to_owned();
            if !matches!(source.as_str(), "MANUAL" | "OKTA_MANAGED") {
                return Err(validation(&["certificateSourceType: Invalid value"]));
            }
            if list(w, "domains")
                .iter()
                .any(|(_, d)| d["domain"] == domain.as_str())
            {
                return Err(validation(&[
                    "domain: An object with this field already exists in the current organization",
                ]));
            }
            let id = new_id(w, "domain", "OcD");
            let token = svc(w).ids.next_prefixed("dns_token", "", 40);
            let brand_id = list(w, "brands")
                .into_iter()
                .find(|(_, b)| b["isDefault"] == true)
                .map(|(k, _)| k)
                .unwrap_or_else(|| DEFAULT_BRAND.to_owned());
            let d = json!({
                "id": id,
                "domain": domain,
                "certificateSourceType": source,
                "validationStatus": "NOT_STARTED",
                "brandId": brand_id,
                "dnsRecords": [
                    { "recordType": "TXT", "fqdn": format!("_oktaverification.{domain}"), "values": [token] },
                    { "recordType": "CNAME", "fqdn": domain, "values": ["octo-corp.customdomains.okta.com"] }
                ],
                "publicCertificate": null
            });
            put(w, "domains", &id, d.clone());
            Ok(Reply::ok(render_domain(&d)))
        }
        [id] if m == Method::GET => Ok(Reply::ok(render_domain(&find(w, id)?))),
        [id] if m == Method::PUT => {
            let mut d = find(w, id)?;
            let body = req.object()?;
            let Some(bid) = body.get("brandId").and_then(Value::as_str) else {
                return Err(validation(&["brandId: The field cannot be left blank"]));
            };
            brand(w, bid)?;
            d["brandId"] = json!(bid);
            put(w, "domains", id, d.clone());
            Ok(Reply::ok(render_domain(&d)))
        }
        [id] if m == Method::DELETE => {
            find(w, id)?;
            remove(w, "domains", id);
            Ok(Reply::no_content())
        }
        [id, "certificate"] if m == Method::PUT => {
            let mut d = find(w, id)?;
            let body = req.object()?;
            let mut causes = Vec::new();
            for f in ["certificate", "certificateChain", "privateKey", "type"] {
                if body
                    .get(f)
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                {
                    causes.push(format!("{f}: The field cannot be left blank"));
                }
            }
            if !causes.is_empty() {
                let refs: Vec<&str> = causes.iter().map(String::as_str).collect();
                return Err(validation(&refs));
            }
            d["certificateSourceType"] = json!("MANUAL");
            d["publicCertificate"] = json!({
                "subject": format!("CN={}", d["domain"].as_str().unwrap_or_default()),
                "fingerprint": "73:1B:7B:4C:2C:8D:9E:5A:13:6B:0F:9A:5E:2D:7C:44:91:08:3E:AA",
                "expiration": "2027-03-14T09:00:00.000Z"
            });
            put(w, "domains", id, d);
            Ok(Reply::no_content())
        }
        [id, "verify"] if m == Method::POST => {
            let mut d = find(w, id)?;
            d["validationStatus"] = json!("VERIFIED");
            put(w, "domains", id, d.clone());
            Ok(Reply::ok(render_domain(&d)))
        }
        _ => Err(endpoint_not_found()),
    }
}

fn email_domains(w: &mut WorldState, req: &Req, s: &[&str]) -> Res {
    let m = &req.method;
    let find = |w: &mut WorldState, id: &str| {
        get(w, "email_domains", id).ok_or_else(|| not_found(id, "EmailDomain"))
    };
    let render = |w: &mut WorldState, d: &Value| {
        let mut out = d.clone();
        if req.q("expand").is_some_and(|e| e.contains("brands")) {
            let id = id_of(d);
            let brands: Vec<Value> = list(w, "brands")
                .into_iter()
                .filter(|(_, b)| b["emailDomainId"] == id.as_str())
                .map(|(_, b)| render_brand(&b))
                .collect();
            out["_embedded"] = json!({ "brands": brands });
        }
        out
    };
    match s {
        [] if m == Method::GET => {
            let docs: Vec<Value> = list(w, "email_domains")
                .into_iter()
                .map(|(_, d)| d)
                .collect();
            let items: Vec<Value> = docs.iter().map(|d| render(w, d)).collect();
            Ok(Reply::ok(Value::Array(items)))
        }
        [] if m == Method::POST => {
            let body = req.object()?;
            let mut causes = Vec::new();
            for f in ["domain", "displayName", "userName", "brandId"] {
                if body
                    .get(f)
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                {
                    causes.push(format!("{f}: The field cannot be left blank"));
                }
            }
            if !causes.is_empty() {
                let refs: Vec<&str> = causes.iter().map(String::as_str).collect();
                return Err(validation(&refs));
            }
            let bid = body["brandId"].as_str().unwrap_or_default().to_owned();
            let mut b = brand(w, &bid)?;
            let domain = body["domain"].as_str().unwrap_or_default().to_lowercase();
            if list(w, "email_domains")
                .iter()
                .any(|(_, d)| d["domain"] == domain.as_str())
            {
                return Err(error(
                    StatusCode::CONFLICT,
                    "E0000197",
                    "The email domain is already in use.",
                    &[],
                ));
            }
            let id = new_id(w, "email_domain", "OeD");
            let token = svc(w).ids.next_prefixed("email_dns", "", 32);
            let d = json!({
                "id": id,
                "domain": domain,
                "displayName": body["displayName"],
                "userName": body["userName"],
                "validationSubdomain": "mail",
                "validationStatus": "NOT_STARTED",
                "dnsValidationRecords": [
                    { "recordType": "TXT", "fqdn": format!("_oktaverification.{domain}"), "verificationValue": token },
                    { "recordType": "TXT", "fqdn": format!("mail.{domain}"), "verificationValue": "v=spf1 include:_spf.oktaemail.com ~all" },
                    { "recordType": "CNAME", "fqdn": format!("oktavr1._domainkey.{domain}"), "verificationValue": "oktavr1.domainkey.oktaemail.com" },
                    { "recordType": "CNAME", "fqdn": format!("oktavr2._domainkey.{domain}"), "verificationValue": "oktavr2.domainkey.oktaemail.com" }
                ]
            });
            put(w, "email_domains", &id, d.clone());
            b["emailDomainId"] = json!(id);
            put(w, "brands", &bid, b);
            Ok(Reply::ok(d))
        }
        [id] if m == Method::GET => {
            let d = find(w, id)?;
            Ok(Reply::ok(render(w, &d)))
        }
        [id] if m == Method::PUT => {
            let mut d = find(w, id)?;
            let body = req.object()?;
            for f in ["displayName", "userName"] {
                match body.get(f).and_then(Value::as_str) {
                    Some(v) if !v.is_empty() => d[f] = json!(v),
                    _ => {
                        return Err(validation(&[&format!(
                            "{f}: The field cannot be left blank"
                        )]));
                    }
                }
            }
            put(w, "email_domains", id, d.clone());
            Ok(Reply::ok(d))
        }
        [id] if m == Method::DELETE => {
            find(w, id)?;
            if list(w, "brands")
                .iter()
                .any(|(_, b)| b["emailDomainId"] == *id)
            {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    "E0000001",
                    "Api validation failed: emailDomain",
                    &[
                        "emailDomain: The email domain is in use by a brand and can't be deleted."
                            .to_owned(),
                    ],
                ));
            }
            remove(w, "email_domains", id);
            Ok(Reply::no_content())
        }
        [id, "verify"] if m == Method::POST => {
            let mut d = find(w, id)?;
            d["validationStatus"] = json!("VERIFIED");
            put(w, "email_domains", id, d.clone());
            Ok(Reply::ok(
                json!({ "validationStatus": "VERIFIED", "dnsValidationRecords": d["dnsValidationRecords"] }),
            ))
        }
        _ => Err(endpoint_not_found()),
    }
}

fn render_da(d: &Value) -> Value {
    let mut out = d.clone();
    let id = id_of(d);
    out["_links"] = json!({ "self": href(&format!("/api/v1/device-assurances/{id}")) });
    out
}

fn device_assurances(w: &mut WorldState, req: &Req, s: &[&str]) -> Res {
    let m = &req.method;
    let find = |w: &mut WorldState, id: &str| {
        get(w, "device_assurances", id).ok_or_else(|| not_found(id, "DeviceAssurance"))
    };
    match s {
        [] if m == Method::GET => {
            let items: Vec<Value> = list(w, "device_assurances")
                .iter()
                .map(|(_, d)| render_da(d))
                .collect();
            Ok(Reply::ok(Value::Array(items)))
        }
        [] if m == Method::POST => {
            let mut body = Value::Object(req.object()?);
            let mut causes = Vec::new();
            if body["name"].as_str().is_none_or(str::is_empty) {
                causes.push("name: The field cannot be left blank");
            }
            if !matches!(
                body["platform"].as_str(),
                Some("ANDROID" | "CHROMEOS" | "IOS" | "MACOS" | "WINDOWS")
            ) {
                causes.push("platform: The field cannot be left blank");
            }
            if !causes.is_empty() {
                return Err(validation(&causes));
            }
            if list(w, "device_assurances")
                .iter()
                .any(|(_, d)| d["name"] == body["name"])
            {
                return Err(validation(&[
                    "name: An object with this field already exists in the current organization",
                ]));
            }
            let now = ts(w);
            let id = new_id(w, "device_assurance", "dae");
            body["id"] = json!(id);
            body["createdBy"] = json!("00utoolbench0000000");
            body["createdDate"] = json!(now);
            body["lastUpdate"] = json!(now);
            body["lastUpdatedBy"] = json!("00utoolbench0000000");
            put(w, "device_assurances", &id, body.clone());
            Ok(Reply::ok(render_da(&body)))
        }
        [id] if m == Method::GET => Ok(Reply::ok(render_da(&find(w, id)?))),
        [id] if m == Method::PUT => {
            let old = find(w, id)?;
            let mut body = Value::Object(req.object()?);
            if body["platform"]
                .as_str()
                .is_some_and(|p| p != old["platform"].as_str().unwrap_or_default())
            {
                return Err(validation(&["platform: The platform cannot be changed"]));
            }
            let mut merged = old.clone();
            merge(&mut merged, &body);
            body = merged;
            for k in ["id", "createdBy", "createdDate", "platform"] {
                body[k] = old[k].clone();
            }
            body["lastUpdate"] = json!(ts(w));
            put(w, "device_assurances", id, body.clone());
            Ok(Reply::ok(render_da(&body)))
        }
        [id] if m == Method::DELETE => {
            find(w, id)?;
            let in_use = list(w, "policy_rules")
                .iter()
                .any(|(_, r)| r.to_string().contains(&format!("\"{id}\"")));
            if in_use {
                return Err(error(
                    StatusCode::CONFLICT,
                    "E0000187",
                    "Cannot delete device assurance policy that is used by a policy rule.",
                    &[],
                ));
            }
            remove(w, "device_assurances", id);
            Ok(Reply::no_content())
        }
        _ => Err(endpoint_not_found()),
    }
}
