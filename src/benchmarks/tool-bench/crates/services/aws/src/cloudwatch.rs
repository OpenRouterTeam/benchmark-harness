//! CloudWatch (botocore 1.43 picks JSON 1.0: `X-Amz-Target: GraniteServiceVersion20100801.<Op>`;
//! the service is awsQueryCompatible, so errors carry `x-amzn-query-error`).
//!
//! - `alarms` keyed by `AlarmName`: DescribeAlarms fields (`AlarmRule` marks a composite alarm).
//! - `alarm_history`: `AlarmName`, `AlarmType`, `Timestamp`, `HistoryItemType`, `HistorySummary`, `HistoryData`.
//! - `metrics`: `Namespace`, `MetricName`, `Dimensions`, `Unit`, and either `Datapoints`
//!   (`[{Timestamp, Value}]`) or `Series` (`{Start, Interval, Values}`).
//! - `dashboards` keyed by name: `DashboardBody` (string or object), `LastModified`.

use axum::{http::StatusCode, response::Response};
use serde_json::{Map, Value, json};

use crate::{
    Ctx, Req,
    util::{iso, json_response, s, value_time, with_header},
};

const CT: &str = "application/x-amz-json-1.0";

type Out = Result<Value, (StatusCode, &'static str, String)>;

/// Labelled series of `(timestamp, value)` points.
type Labelled = Vec<(String, Vec<(i64, f64)>)>;

fn invalid(msg: impl Into<String>) -> (StatusCode, &'static str, String) {
    (StatusCode::BAD_REQUEST, "InvalidParameterValue", msg.into())
}

fn missing(name: &str) -> (StatusCode, &'static str, String) {
    (
        StatusCode::BAD_REQUEST,
        "MissingParameter",
        format!("The parameter {name} is required."),
    )
}

pub fn handle(ctx: &mut Ctx, req: &Req) -> Response {
    let op = req.target_op().unwrap_or_default();
    let input = req.body_json();
    let result = match op.as_str() {
        "DescribeAlarms" => describe_alarms(ctx, &input),
        "DescribeAlarmsForMetric" => describe_alarms_for_metric(ctx, &input),
        "DescribeAlarmHistory" => describe_alarm_history(ctx, &input),
        "PutMetricAlarm" => put_metric_alarm(ctx, &input),
        "PutCompositeAlarm" => put_composite_alarm(ctx, &input),
        "DeleteAlarms" => delete_alarms(ctx, &input),
        "SetAlarmState" => set_alarm_state(ctx, &input),
        "EnableAlarmActions" => alarm_actions(ctx, &input, true),
        "DisableAlarmActions" => alarm_actions(ctx, &input, false),
        "GetMetricData" => get_metric_data(ctx, &input),
        "GetMetricStatistics" => get_metric_statistics(ctx, &input),
        "ListMetrics" => list_metrics(ctx, &input),
        "PutMetricData" => put_metric_data(ctx, &input),
        "ListDashboards" => list_dashboards(ctx, &input),
        "GetDashboard" => get_dashboard(ctx, &input),
        "PutDashboard" => put_dashboard(ctx, &input),
        "DeleteDashboards" => delete_dashboards(ctx, &input),
        "ListTagsForResource" => list_tags(ctx, &input),
        "TagResource" => tag_resource(ctx, &input),
        "UntagResource" => untag_resource(ctx, &input),
        "DescribeAnomalyDetectors" => Ok(json!({"AnomalyDetectors": []})),
        "DescribeInsightRules" => Ok(json!({"InsightRules": []})),
        "ListMetricStreams" => Ok(json!({"Entries": []})),
        "ListManagedInsightRules" => Ok(json!({"ManagedRules": []})),
        "" => Err((
            StatusCode::BAD_REQUEST,
            "UnknownOperationException",
            String::new(),
        )),
        _ => Ok(json!({})),
    };
    match result {
        Ok(v) => json_response(StatusCode::OK, CT, &v),
        Err((status, code, msg)) => {
            let fault = if status.is_server_error() {
                "Receiver"
            } else {
                "Sender"
            };
            with_header(
                json_response(
                    status,
                    CT,
                    &json!({"__type": format!("com.amazonaws.cloudwatch#{code}"), "message": msg}),
                ),
                "x-amzn-query-error",
                &format!("{code};{fault}"),
            )
        }
    }
}

fn epoch(v: &Value) -> Value {
    v_time(v).map(|t| json!(t)).unwrap_or(Value::Null)
}

fn v_time(v: &Value) -> Option<i64> {
    value_time(v)
}

fn alarm_arn(ctx: &Ctx, name: &str) -> String {
    format!(
        "arn:aws:cloudwatch:{}:{}:alarm:{name}",
        ctx.region, ctx.account
    )
}

/// Dataset alarm into its DescribeAlarms shape (timestamps as epoch seconds).
fn alarm_out(ctx: &Ctx, a: &Value) -> Value {
    let mut out = Map::new();
    let name = s(a, "AlarmName");
    for (k, v) in a.as_object().cloned().unwrap_or_default() {
        if k.starts_with('_') || k == "Tags" {
            continue;
        }
        if k.ends_with("Timestamp") {
            out.insert(k, epoch(&v));
        } else {
            out.insert(k, v);
        }
    }
    out.entry("AlarmArn")
        .or_insert_with(|| json!(alarm_arn(ctx, &name)));
    out.entry("ActionsEnabled").or_insert(json!(true));
    for k in ["OKActions", "AlarmActions", "InsufficientDataActions"] {
        out.entry(k).or_insert(json!([]));
    }
    if a.get("AlarmRule").is_none() {
        out.entry("Dimensions").or_insert(json!([]));
    }
    Value::Object(out)
}

fn is_composite(a: &Value) -> bool {
    a.get("AlarmRule").is_some()
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|i| i.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn paged(items: Vec<Value>, input: &Value, default_max: usize) -> (Vec<Value>, Option<String>) {
    let max = input
        .get("MaxRecords")
        .or_else(|| input.get("MaxResults"))
        .and_then(Value::as_u64)
        .map(|m| m as usize)
        .unwrap_or(default_max)
        .max(1);
    let start = input
        .get("NextToken")
        .and_then(Value::as_str)
        .and_then(|t| t.parse::<usize>().ok())
        .unwrap_or(0);
    let total = items.len();
    let page: Vec<Value> = items.into_iter().skip(start).take(max).collect();
    let next = (start + max < total).then(|| (start + max).to_string());
    (page, next)
}

fn describe_alarms(ctx: &mut Ctx, input: &Value) -> Out {
    let names = strings(input, "AlarmNames");
    let prefix = s(input, "AlarmNamePrefix");
    let state = s(input, "StateValue");
    let action_prefix = s(input, "ActionPrefix");
    let types = strings(input, "AlarmTypes");
    let want_metric = types.is_empty() || types.iter().any(|t| t == "MetricAlarm");
    let want_composite = types.iter().any(|t| t == "CompositeAlarm");
    let all: Vec<Value> = ctx
        .docs("alarms")
        .into_iter()
        .map(|(_, a)| a)
        .filter(|a| ctx.in_region(a))
        .filter(|a| names.is_empty() || names.contains(&s(a, "AlarmName")))
        .filter(|a| s(a, "AlarmName").starts_with(&prefix))
        .filter(|a| state.is_empty() || s(a, "StateValue") == state)
        .filter(|a| {
            action_prefix.is_empty()
                || strings(a, "AlarmActions")
                    .iter()
                    .any(|x| x.starts_with(&action_prefix))
        })
        .collect();
    let mut sorted = all;
    sorted.sort_by_key(|a| s(a, "AlarmName"));
    let (page, next) = paged(sorted, input, 100);
    let metric: Vec<Value> = page
        .iter()
        .filter(|a| !is_composite(a) && want_metric)
        .map(|a| alarm_out(ctx, a))
        .collect();
    let composite: Vec<Value> = page
        .iter()
        .filter(|a| is_composite(a) && want_composite)
        .map(|a| alarm_out(ctx, a))
        .collect();
    let mut out = json!({"MetricAlarms": metric, "CompositeAlarms": composite});
    if let Some(n) = next {
        out["NextToken"] = json!(n);
    }
    Ok(out)
}

fn dims(v: &Value) -> Vec<(String, String)> {
    let mut d: Vec<(String, String)> = v
        .get("Dimensions")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|x| (s(x, "Name"), s(x, "Value"))).collect())
        .unwrap_or_default();
    d.sort();
    d
}

fn describe_alarms_for_metric(ctx: &mut Ctx, input: &Value) -> Out {
    let want = dims(input);
    let alarms: Vec<Value> = ctx
        .docs("alarms")
        .into_iter()
        .map(|(_, a)| a)
        .filter(|a| !is_composite(a) && ctx.in_region(a))
        .filter(|a| {
            s(a, "MetricName") == s(input, "MetricName")
                && s(a, "Namespace") == s(input, "Namespace")
        })
        .filter(|a| input.get("Dimensions").is_none() || dims(a) == want)
        .filter(|a| {
            input.get("Statistic").is_none() || a.get("Statistic") == input.get("Statistic")
        })
        .filter(|a| input.get("Period").is_none() || a.get("Period") == input.get("Period"))
        .map(|a| alarm_out(ctx, &a))
        .collect();
    Ok(json!({"MetricAlarms": alarms}))
}

fn describe_alarm_history(ctx: &mut Ctx, input: &Value) -> Out {
    let name = s(input, "AlarmName");
    let kind = s(input, "HistoryItemType");
    let types = strings(input, "AlarmTypes");
    let start = input.get("StartDate").and_then(v_time);
    let end = input.get("EndDate").and_then(v_time);
    let mut items: Vec<Value> = ctx
        .docs("alarm_history")
        .into_iter()
        .map(|(_, h)| h)
        .filter(|h| name.is_empty() || s(h, "AlarmName") == name)
        .filter(|h| kind.is_empty() || s(h, "HistoryItemType") == kind)
        .filter(|h| {
            types.is_empty()
                || types.contains(
                    &h.get("AlarmType")
                        .and_then(Value::as_str)
                        .unwrap_or("MetricAlarm")
                        .to_owned(),
                )
        })
        .filter(|h| {
            let t = h.get("Timestamp").and_then(v_time).unwrap_or(0);
            start.is_none_or(|st| t >= st) && end.is_none_or(|e| t <= e)
        })
        .collect();
    items.sort_by_key(|h| h.get("Timestamp").and_then(v_time).unwrap_or(0));
    if s(input, "ScanBy") != "TimestampAscending" {
        items.reverse();
    }
    let items = items
        .into_iter()
        .map(|mut h| {
            h["Timestamp"] = epoch(&h["Timestamp"]);
            if h.get("AlarmType").is_none() {
                h["AlarmType"] = json!("MetricAlarm");
            }
            if let Value::Object(hd) = &h["HistoryData"] {
                h["HistoryData"] = json!(Value::Object(hd.clone()).to_string());
            }
            h
        })
        .collect();
    let (page, next) = paged(items, input, 100);
    let mut out = json!({"AlarmHistoryItems": page});
    if let Some(n) = next {
        out["NextToken"] = json!(n);
    }
    Ok(out)
}

const COMPARISONS: [&str; 7] = [
    "GreaterThanOrEqualToThreshold",
    "GreaterThanThreshold",
    "LessThanThreshold",
    "LessThanOrEqualToThreshold",
    "LessThanLowerOrGreaterThanUpperThreshold",
    "LessThanLowerThreshold",
    "GreaterThanUpperThreshold",
];

fn add_history(ctx: &mut Ctx, name: &str, kind: &str, summary: String, data: Value) {
    let n = ctx.svc.ids.next_int("alarm-history", 1);
    let key = format!("{name}#{n}");
    ctx.put(
        "alarm_history",
        &key,
        json!({
            "AlarmName": name,
            "AlarmType": "MetricAlarm",
            "Timestamp": iso(ctx.now),
            "HistoryItemType": kind,
            "HistorySummary": summary,
            "HistoryData": data,
        }),
    );
}

fn put_metric_alarm(ctx: &mut Ctx, input: &Value) -> Out {
    let name = s(input, "AlarmName");
    if name.is_empty() {
        return Err(missing("AlarmName"));
    }
    if input
        .get("EvaluationPeriods")
        .and_then(Value::as_i64)
        .is_none()
    {
        return Err(missing("EvaluationPeriods"));
    }
    let cmp = s(input, "ComparisonOperator");
    if !COMPARISONS.contains(&cmp.as_str()) {
        return Err(invalid(format!(
            "1 validation error detected: Value '{cmp}' at 'comparisonOperator' failed to satisfy constraint: Member must satisfy enum value set: [{}]",
            COMPARISONS.join(", ")
        )));
    }
    let has_metrics = input
        .get("Metrics")
        .is_some_and(|m| m.as_array().is_some_and(|a| !a.is_empty()));
    if !has_metrics {
        if s(input, "MetricName").is_empty() {
            return Err(missing("MetricName"));
        }
        if s(input, "Namespace").is_empty() {
            return Err(missing("Namespace"));
        }
        if input.get("Statistic").is_none() && input.get("ExtendedStatistic").is_none() {
            return Err(invalid(
                "One of Statistic or ExtendedStatistic must be specified.",
            ));
        }
        if input.get("Period").and_then(Value::as_i64).is_none() {
            return Err(missing("Period"));
        }
        if let Some(st) = input.get("Statistic").and_then(Value::as_str)
            && !["SampleCount", "Average", "Sum", "Minimum", "Maximum"].contains(&st)
        {
            return Err(invalid(format!(
                "1 validation error detected: Value '{st}' at 'statistic' failed to satisfy constraint: Member must satisfy enum value set: [Maximum, SampleCount, Sum, Minimum, Average]"
            )));
        }
    }
    if input.get("Threshold").is_none() && input.get("ThresholdMetricId").is_none() {
        return Err(missing("Threshold"));
    }
    let existing = ctx.get("alarms", &name);
    let mut alarm = Map::new();
    alarm.insert("AlarmName".into(), json!(name));
    alarm.insert("AlarmArn".into(), json!(alarm_arn(ctx, &name)));
    for k in [
        "AlarmDescription",
        "ActionsEnabled",
        "OKActions",
        "AlarmActions",
        "InsufficientDataActions",
        "MetricName",
        "Namespace",
        "Statistic",
        "ExtendedStatistic",
        "Dimensions",
        "Period",
        "Unit",
        "EvaluationPeriods",
        "DatapointsToAlarm",
        "Threshold",
        "ComparisonOperator",
        "TreatMissingData",
        "EvaluateLowSampleCountPercentile",
        "Metrics",
        "ThresholdMetricId",
    ] {
        if let Some(v) = input.get(k).filter(|v| !v.is_null()) {
            alarm.insert(k.into(), v.clone());
        }
    }
    if !has_metrics {
        alarm.entry("Dimensions").or_insert(json!([]));
    }
    alarm.entry("ActionsEnabled").or_insert(json!(true));
    alarm.insert(
        "AlarmConfigurationUpdatedTimestamp".into(),
        json!(iso(ctx.now)),
    );
    match &existing {
        Some(prev) => {
            for k in [
                "StateValue",
                "StateReason",
                "StateReasonData",
                "StateUpdatedTimestamp",
                "StateTransitionedTimestamp",
            ] {
                if let Some(v) = prev.get(k) {
                    alarm.insert(k.into(), v.clone());
                }
            }
        }
        None => {
            alarm.insert("StateValue".into(), json!("INSUFFICIENT_DATA"));
            alarm.insert(
                "StateReason".into(),
                json!("Unchecked: Initial alarm creation"),
            );
            alarm.insert("StateUpdatedTimestamp".into(), json!(iso(ctx.now)));
            alarm.insert("StateTransitionedTimestamp".into(), json!(iso(ctx.now)));
        }
    }
    if let Some(tags) = input.get("Tags").filter(|t| t.is_array()) {
        alarm.insert("Tags".into(), tags.clone());
    } else if let Some(prev_tags) = existing.as_ref().and_then(|e| e.get("Tags")) {
        alarm.insert("Tags".into(), prev_tags.clone());
    }
    let verb = if existing.is_some() {
        "updated"
    } else {
        "created"
    };
    let alarm = Value::Object(alarm);
    ctx.put("alarms", &name, alarm.clone());
    let data = json!({"version": "1.0", "type": if verb == "created" {"Create"} else {"Update"}, "createdAlarm": alarm_out(ctx, &alarm)});
    add_history(
        ctx,
        &name,
        "ConfigurationUpdate",
        format!("Alarm \"{name}\" {verb}"),
        data,
    );
    Ok(json!({}))
}

fn put_composite_alarm(ctx: &mut Ctx, input: &Value) -> Out {
    let name = s(input, "AlarmName");
    if name.is_empty() {
        return Err(missing("AlarmName"));
    }
    if s(input, "AlarmRule").is_empty() {
        return Err(missing("AlarmRule"));
    }
    let mut alarm = input.clone();
    alarm["AlarmArn"] = json!(alarm_arn(ctx, &name));
    alarm["AlarmConfigurationUpdatedTimestamp"] = json!(iso(ctx.now));
    if ctx.get("alarms", &name).is_none() {
        alarm["StateValue"] = json!("INSUFFICIENT_DATA");
        alarm["StateReason"] = json!("Unchecked: Initial alarm creation");
        alarm["StateUpdatedTimestamp"] = json!(iso(ctx.now));
    }
    ctx.put("alarms", &name, alarm);
    Ok(json!({}))
}

fn delete_alarms(ctx: &mut Ctx, input: &Value) -> Out {
    let names = strings(input, "AlarmNames");
    if let Some(missing_name) = names.iter().find(|n| ctx.get("alarms", n).is_none()) {
        return Err((
            StatusCode::NOT_FOUND,
            "ResourceNotFound",
            format!(
                "1 validation error detected: The following alarm names could not be found: [{missing_name}]."
            ),
        ));
    }
    for n in names {
        ctx.remove("alarms", &n);
    }
    Ok(json!({}))
}

fn set_alarm_state(ctx: &mut Ctx, input: &Value) -> Out {
    let name = s(input, "AlarmName");
    let value = s(input, "StateValue");
    if !["OK", "ALARM", "INSUFFICIENT_DATA"].contains(&value.as_str()) {
        return Err(invalid(format!(
            "1 validation error detected: Value '{value}' at 'stateValue' failed to satisfy constraint: Member must satisfy enum value set: [INSUFFICIENT_DATA, ALARM, OK]"
        )));
    }
    let Some(mut alarm) = ctx.get("alarms", &name) else {
        return Err((
            StatusCode::NOT_FOUND,
            "ResourceNotFound",
            format!("Alarm {name} does not exist"),
        ));
    };
    let old = s(&alarm, "StateValue");
    alarm["StateValue"] = json!(value);
    alarm["StateReason"] = json!(s(input, "StateReason"));
    alarm["StateUpdatedTimestamp"] = json!(iso(ctx.now));
    if old != value {
        alarm["StateTransitionedTimestamp"] = json!(iso(ctx.now));
    }
    ctx.put("alarms", &name, alarm);
    add_history(
        ctx,
        &name,
        "StateUpdate",
        format!("Alarm updated from {old} to {value}"),
        json!({"version": "1.0", "oldState": {"stateValue": old}, "newState": {"stateValue": value, "stateReason": s(input, "StateReason")}}),
    );
    Ok(json!({}))
}

fn alarm_actions(ctx: &mut Ctx, input: &Value, enabled: bool) -> Out {
    for n in strings(input, "AlarmNames") {
        if let Some(a) = ctx.get_mut("alarms", &n) {
            a["ActionsEnabled"] = json!(enabled);
        }
    }
    Ok(json!({}))
}

// ───────────────────────────── metrics ─────────────────────────────

struct Series {
    namespace: String,
    name: String,
    dims: Vec<(String, String)>,
    unit: Option<String>,
    points: Vec<(i64, f64)>,
}

fn all_series(ctx: &Ctx) -> Vec<Series> {
    ctx.docs("metrics")
        .into_iter()
        .filter(|(_, m)| ctx.in_region(m))
        .map(|(_, m)| {
            let mut points: Vec<(i64, f64)> = m
                .get("Datapoints")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|p| {
                            Some((
                                p.get("Timestamp").and_then(v_time)?,
                                p.get("Value")?.as_f64()?,
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if let Some(series) = m.get("Series") {
                let start = series.get("Start").and_then(v_time).unwrap_or(0);
                let interval = series.get("Interval").and_then(Value::as_i64).unwrap_or(60);
                if let Some(values) = series.get("Values").and_then(Value::as_array) {
                    for (i, v) in values.iter().enumerate() {
                        if let Some(f) = v.as_f64() {
                            points.push((start + i as i64 * interval, f));
                        }
                    }
                }
            }
            points.sort_by_key(|p| p.0);
            Series {
                namespace: s(&m, "Namespace"),
                name: s(&m, "MetricName"),
                dims: dims(&m),
                unit: m.get("Unit").and_then(Value::as_str).map(str::to_owned),
                points,
            }
        })
        .collect()
}

fn stat(values: &[f64], stat: &str) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let sum: f64 = values.iter().sum();
    let lower = stat.to_ascii_lowercase();
    Some(match lower.as_str() {
        "average" | "avg" => sum / values.len() as f64,
        "sum" => sum,
        "minimum" | "min" => values.iter().copied().fold(f64::INFINITY, f64::min),
        "maximum" | "max" => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        "samplecount" | "count" => values.len() as f64,
        p if p.starts_with('p') => {
            let q: f64 = p[1..].parse().ok()?;
            let mut sorted = values.to_vec();
            sorted.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
            let rank = ((q / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
            sorted[rank.min(sorted.len()) - 1]
        }
        _ => return None,
    })
}

/// `(bucket start, statistic)` for each period with data in `[start, end)`.
fn aggregate(
    points: &[(i64, f64)],
    start: i64,
    end: i64,
    period: i64,
    stat_name: &str,
) -> Vec<(i64, f64)> {
    let period = period.max(1);
    let mut buckets: Vec<(i64, Vec<f64>)> = Vec::new();
    for (t, v) in points.iter().filter(|(t, _)| *t >= start && *t < end) {
        let b = t - (t - start).rem_euclid(period);
        match buckets.iter_mut().find(|(k, _)| *k == b) {
            Some((_, vs)) => vs.push(*v),
            None => buckets.push((b, vec![*v])),
        }
    }
    buckets
        .into_iter()
        .filter_map(|(b, vs)| stat(&vs, stat_name).map(|x| (b, round(x))))
        .collect()
}

fn round(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

fn find_series<'a>(all: &'a [Series], metric: &Value) -> Option<&'a Series> {
    let want = dims(metric);
    all.iter().find(|m| {
        m.namespace == s(metric, "Namespace") && m.name == s(metric, "MetricName") && m.dims == want
    })
}

struct Computed {
    id: String,
    label: String,
    points: Vec<(i64, f64)>,
    messages: Vec<Value>,
    status: &'static str,
}

fn get_metric_data(ctx: &mut Ctx, input: &Value) -> Out {
    let start = input
        .get("StartTime")
        .and_then(v_time)
        .ok_or_else(|| missing("StartTime"))?;
    let end = input
        .get("EndTime")
        .and_then(v_time)
        .ok_or_else(|| missing("EndTime"))?;
    if end <= start {
        return Err(invalid(
            "The parameter StartTime must be less than the parameter EndTime.",
        ));
    }
    let queries = input
        .get("MetricDataQueries")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| missing("MetricDataQueries"))?;
    let all = all_series(ctx);
    let mut computed: Vec<(Computed, bool)> = Vec::new();
    // Metric stats first, then expressions (which can reference them).
    for q in queries.iter().filter(|q| q.get("MetricStat").is_some()) {
        let ms = &q["MetricStat"];
        let metric = &ms["Metric"];
        let period = ms.get("Period").and_then(Value::as_i64).unwrap_or(60);
        let stat_name = s(ms, "Stat");
        let points = find_series(&all, metric)
            .map(|m| aggregate(&m.points, start, end, period, &stat_name))
            .unwrap_or_default();
        computed.push((
            Computed {
                id: s(q, "Id"),
                label: q
                    .get("Label")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| s(metric, "MetricName")),
                points,
                messages: Vec::new(),
                status: "Complete",
            },
            q.get("ReturnData").and_then(Value::as_bool).unwrap_or(true),
        ));
    }
    for q in queries.iter().filter(|q| q.get("Expression").is_some()) {
        let expr = s(q, "Expression");
        let period = q.get("Period").and_then(Value::as_i64).unwrap_or(60);
        let ret = q.get("ReturnData").and_then(Value::as_bool).unwrap_or(true);
        if expr.trim_start().to_ascii_uppercase().starts_with("SELECT") {
            match metrics_insights(&all, &expr, start, end, period) {
                Ok(groups) if groups.is_empty() => computed.push((
                    Computed {
                        id: s(q, "Id"),
                        label: s(q, "Label"),
                        points: Vec::new(),
                        messages: Vec::new(),
                        status: "Complete",
                    },
                    ret,
                )),
                Ok(groups) => {
                    for (label, points) in groups {
                        computed.push((
                            Computed {
                                id: s(q, "Id"),
                                label,
                                points,
                                messages: Vec::new(),
                                status: "Complete",
                            },
                            ret,
                        ));
                    }
                }
                Err(msg) => {
                    return Err((StatusCode::BAD_REQUEST, "ValidationError", msg));
                }
            }
            continue;
        }
        let refs: Vec<(String, Vec<(i64, f64)>)> = computed
            .iter()
            .map(|(c, _)| (c.id.clone(), c.points.clone()))
            .collect();
        let (points, messages, status) = match eval_expression(&expr, &refs) {
            Some(p) => (p, Vec::new(), "Complete"),
            None => (
                Vec::new(),
                vec![
                    json!({"Code": "InternalError", "Value": format!("Unsupported or invalid expression: {expr}")}),
                ],
                "InternalError",
            ),
        };
        computed.push((
            Computed {
                id: s(q, "Id"),
                label: q
                    .get("Label")
                    .and_then(Value::as_str)
                    .unwrap_or(&s(q, "Id"))
                    .to_owned(),
                points,
                messages,
                status,
            },
            ret,
        ));
    }
    let ascending = s(input, "ScanBy") == "TimestampAscending";
    // Results follow the request's query order.
    let order: Vec<String> = queries.iter().map(|q| s(q, "Id")).collect();
    let mut results = Vec::new();
    for id in &order {
        for (c, ret) in computed.iter().filter(|(c, _)| c.id == *id) {
            if !ret {
                continue;
            }
            let mut pts = c.points.clone();
            pts.sort_by_key(|p| p.0);
            if !ascending {
                pts.reverse();
            }
            results.push(json!({
                "Id": c.id,
                "Label": c.label,
                "Timestamps": pts.iter().map(|p| p.0).collect::<Vec<_>>(),
                "Values": pts.iter().map(|p| p.1).collect::<Vec<_>>(),
                "StatusCode": c.status,
                "Messages": c.messages,
            }));
        }
    }
    Ok(json!({"MetricDataResults": results, "Messages": []}))
}

/// `SELECT AVG("CPUUtilization") FROM SCHEMA("AWS/EC2", "InstanceId") WHERE "k"='v'
///  GROUP BY "InstanceId" ORDER BY AVG() DESC LIMIT 10`
fn metrics_insights(
    all: &[Series],
    expr: &str,
    start: i64,
    end: i64,
    period: i64,
) -> Result<Labelled, String> {
    let re = regex::Regex::new(
        r#"(?is)^\s*SELECT\s+(\w+)\(\s*"?([^")]+?)"?\s*\)\s+FROM\s+(?:SCHEMA\(\s*"?([^",)]+)"?\s*((?:,\s*"?[^",)]+"?\s*)*)\)|"?([^"\s]+)"?)(?:\s+WHERE\s+(.+?))?(?:\s+GROUP\s+BY\s+(.+?))?(?:\s+ORDER\s+BY\s+(\w+)\(\)\s*(ASC|DESC)?)?(?:\s+LIMIT\s+(\d+))?\s*$"#,
    )
    .expect("valid regex");
    let Some(c) = re.captures(expr) else {
        return Err(format!("Metrics Insights query is invalid: {expr}"));
    };
    let func = c[1].to_ascii_uppercase();
    if !["AVG", "SUM", "MIN", "MAX", "COUNT"].contains(&func.as_str()) {
        return Err(format!(
            "Unsupported function {func} in Metrics Insights query"
        ));
    }
    let metric = c[2].trim().to_owned();
    let namespace = c
        .get(3)
        .or(c.get(5))
        .map(|m| m.as_str().trim().to_owned())
        .unwrap_or_default();
    let schema_keys: Option<Vec<String>> = c.get(3).map(|_| {
        c.get(4)
            .map(|m| {
                m.as_str()
                    .split(',')
                    .map(|k| k.trim().trim_matches('"').to_owned())
                    .filter(|k| !k.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    });
    let mut wheres: Vec<(String, String, bool)> = Vec::new();
    if let Some(w) = c.get(6) {
        let cond =
            regex::Regex::new(r#"(?i)"?([\w.:/-]+)"?\s*(!=|=)\s*'([^']*)'"#).expect("valid regex");
        for m in cond.captures_iter(w.as_str()) {
            wheres.push((m[1].to_owned(), m[3].to_owned(), &m[2] == "="));
        }
    }
    let group_by: Vec<String> = c
        .get(7)
        .map(|g| {
            g.as_str()
                .split(',')
                .map(|k| k.trim().trim_matches('"').to_owned())
                .collect()
        })
        .unwrap_or_default();
    let order_desc = c
        .get(9)
        .is_none_or(|d| d.as_str().eq_ignore_ascii_case("DESC"));
    let order = c.get(8).map(|o| o.as_str().to_ascii_uppercase());
    let limit: Option<usize> = c.get(10).and_then(|l| l.as_str().parse().ok());
    let stat_name = match func.as_str() {
        "AVG" => "Average",
        "SUM" => "Sum",
        "MIN" => "Minimum",
        "MAX" => "Maximum",
        _ => "SampleCount",
    };
    let matching: Vec<&Series> = all
        .iter()
        .filter(|m| m.namespace == namespace && m.name == metric)
        .filter(|m| {
            schema_keys.as_ref().is_none_or(|keys| {
                let mut names: Vec<&String> = m.dims.iter().map(|(k, _)| k).collect();
                names.sort();
                let mut want: Vec<&String> = keys.iter().collect();
                want.sort();
                names == want
            })
        })
        .filter(|m| {
            wheres.iter().all(|(k, v, eq)| {
                let has = m.dims.iter().any(|(dk, dv)| dk == k && dv == v);
                has == *eq
            })
        })
        .collect();
    // Group series by the GROUP BY dimension values.
    let mut groups: Vec<(String, Vec<(i64, f64)>)> = Vec::new();
    for m in matching {
        let label = if group_by.is_empty() {
            metric.clone()
        } else {
            group_by
                .iter()
                .map(|g| {
                    m.dims
                        .iter()
                        .find(|(k, _)| k == g)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        match groups.iter_mut().find(|(l, _)| *l == label) {
            Some((_, pts)) => pts.extend(m.points.iter().copied()),
            None => groups.push((label, m.points.clone())),
        }
    }
    #[allow(clippy::type_complexity)]
    let mut out: Vec<(String, Vec<(i64, f64)>, f64)> = groups
        .into_iter()
        .map(|(label, pts)| {
            let window: Vec<f64> = pts
                .iter()
                .filter(|(t, _)| *t >= start && *t < end)
                .map(|p| p.1)
                .collect();
            let overall = order
                .as_deref()
                .and_then(|o| stat(&window, o))
                .unwrap_or(0.0);
            (
                label,
                aggregate(&pts, start, end, period, stat_name),
                overall,
            )
        })
        .collect();
    if order.is_some() {
        out.sort_by(|a, b| {
            let o = a.2.partial_cmp(&b.2).expect("finite");
            if order_desc { o.reverse() } else { o }
        });
    }
    if let Some(l) = limit {
        out.truncate(l);
    }
    Ok(out.into_iter().map(|(l, p, _)| (l, p)).collect())
}

/// Arithmetic over other query ids and numbers: `100 * (m1 / m2)`, `m1 + m2`.
fn eval_expression(expr: &str, refs: &[(String, Vec<(i64, f64)>)]) -> Option<Vec<(i64, f64)>> {
    #[derive(Clone)]
    enum V {
        Num(f64),
        Ser(Vec<(i64, f64)>),
    }
    fn combine(a: V, b: V, op: char) -> V {
        let f = |x: f64, y: f64| match op {
            '+' => x + y,
            '-' => x - y,
            '*' => x * y,
            _ => {
                if y == 0.0 {
                    f64::NAN
                } else {
                    x / y
                }
            }
        };
        match (a, b) {
            (V::Num(x), V::Num(y)) => V::Num(f(x, y)),
            (V::Ser(s), V::Num(y)) => V::Ser(s.into_iter().map(|(t, x)| (t, f(x, y))).collect()),
            (V::Num(x), V::Ser(s)) => V::Ser(s.into_iter().map(|(t, y)| (t, f(x, y))).collect()),
            (V::Ser(a), V::Ser(b)) => V::Ser(
                a.into_iter()
                    .filter_map(|(t, x)| {
                        b.iter()
                            .find(|(tb, _)| *tb == t)
                            .map(|(_, y)| (t, f(x, *y)))
                    })
                    .collect(),
            ),
        }
    }
    let tokens: Vec<String> = {
        let mut t = Vec::new();
        let mut cur = String::new();
        for ch in expr.chars() {
            if "+-*/()".contains(ch) {
                if !cur.trim().is_empty() {
                    t.push(cur.trim().to_owned());
                }
                cur.clear();
                t.push(ch.to_string());
            } else if ch.is_whitespace() {
                if !cur.trim().is_empty() {
                    t.push(cur.trim().to_owned());
                }
                cur.clear();
            } else {
                cur.push(ch);
            }
        }
        if !cur.trim().is_empty() {
            t.push(cur.trim().to_owned());
        }
        t
    };
    fn parse_expr(t: &[String], i: &mut usize, refs: &[(String, Vec<(i64, f64)>)]) -> Option<V> {
        let mut left = parse_term(t, i, refs)?;
        while *i < t.len() && (t[*i] == "+" || t[*i] == "-") {
            let op = t[*i].chars().next()?;
            *i += 1;
            let right = parse_term(t, i, refs)?;
            left = combine(left, right, op);
        }
        Some(left)
    }
    fn parse_term(t: &[String], i: &mut usize, refs: &[(String, Vec<(i64, f64)>)]) -> Option<V> {
        let mut left = parse_atom(t, i, refs)?;
        while *i < t.len() && (t[*i] == "*" || t[*i] == "/") {
            let op = t[*i].chars().next()?;
            *i += 1;
            let right = parse_atom(t, i, refs)?;
            left = combine(left, right, op);
        }
        Some(left)
    }
    fn parse_atom(t: &[String], i: &mut usize, refs: &[(String, Vec<(i64, f64)>)]) -> Option<V> {
        let tok = t.get(*i)?.clone();
        *i += 1;
        if tok == "(" {
            let v = parse_expr(t, i, refs)?;
            if t.get(*i)? != ")" {
                return None;
            }
            *i += 1;
            return Some(v);
        }
        if tok == "-" {
            return parse_atom(t, i, refs).map(|v| combine(V::Num(0.0), v, '-'));
        }
        if let Ok(n) = tok.parse::<f64>() {
            return Some(V::Num(n));
        }
        refs.iter()
            .find(|(id, _)| *id == tok)
            .map(|(_, s)| V::Ser(s.clone()))
    }
    let mut i = 0;
    let v = parse_expr(&tokens, &mut i, refs)?;
    if i != tokens.len() {
        return None;
    }
    match v {
        V::Ser(s) => Some(
            s.into_iter()
                .filter(|(_, x)| x.is_finite())
                .map(|(t, x)| (t, round(x)))
                .collect(),
        ),
        V::Num(_) => None,
    }
}

fn get_metric_statistics(ctx: &mut Ctx, input: &Value) -> Out {
    let start = input
        .get("StartTime")
        .and_then(v_time)
        .ok_or_else(|| missing("StartTime"))?;
    let end = input
        .get("EndTime")
        .and_then(v_time)
        .ok_or_else(|| missing("EndTime"))?;
    let period = input
        .get("Period")
        .and_then(Value::as_i64)
        .ok_or_else(|| missing("Period"))?;
    let mut stats = strings(input, "Statistics");
    let ext = strings(input, "ExtendedStatistics");
    if stats.is_empty() && ext.is_empty() {
        return Err(missing("Statistics"));
    }
    let all = all_series(ctx);
    let series = find_series(&all, input);
    let mut datapoints: Vec<Value> = Vec::new();
    if let Some(m) = series {
        let mut starts: Vec<i64> = aggregate(&m.points, start, end, period, "SampleCount")
            .into_iter()
            .map(|(b, _)| b)
            .collect();
        starts.sort();
        stats.dedup();
        for b in starts {
            let window: Vec<f64> = m
                .points
                .iter()
                .filter(|(t, _)| *t >= b && *t < b + period)
                .map(|p| p.1)
                .collect();
            let mut dp = json!({"Timestamp": b});
            for st in &stats {
                if let Some(v) = stat(&window, st) {
                    dp[st] = json!(round(v));
                }
            }
            if !ext.is_empty() {
                let mut e = Map::new();
                for st in &ext {
                    if let Some(v) = stat(&window, st) {
                        e.insert(st.clone(), json!(round(v)));
                    }
                }
                dp["ExtendedStatistics"] = Value::Object(e);
            }
            dp["Unit"] = json!(m.unit.clone().unwrap_or_else(|| "None".into()));
            datapoints.push(dp);
        }
    }
    Ok(json!({"Label": s(input, "MetricName"), "Datapoints": datapoints}))
}

fn list_metrics(ctx: &mut Ctx, input: &Value) -> Out {
    let ns = s(input, "Namespace");
    let name = s(input, "MetricName");
    let filters: Vec<(String, Option<String>)> = input
        .get("Dimensions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|d| {
                    (
                        s(d, "Name"),
                        d.get("Value").and_then(Value::as_str).map(str::to_owned),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let metrics: Vec<Value> = all_series(ctx)
        .into_iter()
        .filter(|m| ns.is_empty() || m.namespace == ns)
        .filter(|m| name.is_empty() || m.name == name)
        .filter(|m| {
            filters.iter().all(|(k, v)| {
                m.dims.iter().any(|(dk, dv)| dk == k && v.as_ref().is_none_or(|v| v == dv))
            })
        })
        .map(|m| {
            json!({
                "Namespace": m.namespace,
                "MetricName": m.name,
                "Dimensions": m.dims.iter().map(|(k, v)| json!({"Name": k, "Value": v})).collect::<Vec<_>>(),
            })
        })
        .collect();
    let (page, next) = paged(metrics, &json!({"NextToken": input.get("NextToken")}), 500);
    let mut out = json!({"Metrics": page});
    if let Some(n) = next {
        out["NextToken"] = json!(n);
    }
    Ok(out)
}

fn put_metric_data(ctx: &mut Ctx, input: &Value) -> Out {
    let ns = s(input, "Namespace");
    if ns.is_empty() {
        return Err(missing("Namespace"));
    }
    if ns.starts_with("AWS/") {
        return Err(invalid(
            "The value AWS/ for parameter Namespace is invalid.",
        ));
    }
    for d in input
        .get("MetricData")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let mut metric = json!({"Namespace": ns, "MetricName": s(&d, "MetricName"), "Dimensions": d.get("Dimensions").cloned().unwrap_or(json!([]))});
        let want = dims(&metric);
        let key = format!(
            "{ns}|{}|{}",
            s(&d, "MetricName"),
            want.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let t = d.get("Timestamp").and_then(v_time).unwrap_or(ctx.now);
        let value = d.get("Value").cloned().unwrap_or(json!(0));
        match ctx.get_mut("metrics", &key) {
            Some(existing) => {
                if let Some(points) = existing.get_mut("Datapoints").and_then(Value::as_array_mut) {
                    points.push(json!({"Timestamp": iso(t), "Value": value}));
                } else {
                    existing["Datapoints"] = json!([{"Timestamp": iso(t), "Value": value}]);
                }
            }
            None => {
                metric["Unit"] = d.get("Unit").cloned().unwrap_or(json!("None"));
                metric["Datapoints"] = json!([{"Timestamp": iso(t), "Value": value}]);
                ctx.put("metrics", &key, metric);
            }
        }
    }
    Ok(json!({}))
}

fn dashboard_arn(ctx: &Ctx, name: &str) -> String {
    format!("arn:aws:cloudwatch::{}:dashboard/{name}", ctx.account)
}

fn body_string(d: &Value) -> String {
    match d.get("DashboardBody") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => "{\"widgets\":[]}".into(),
    }
}

fn list_dashboards(ctx: &mut Ctx, input: &Value) -> Out {
    let prefix = s(input, "DashboardNamePrefix");
    let entries: Vec<Value> = ctx
        .docs("dashboards")
        .iter()
        .filter(|(n, _)| n.starts_with(&prefix))
        .map(|(n, d)| {
            json!({
                "DashboardName": n,
                "DashboardArn": dashboard_arn(ctx, n),
                "LastModified": d.get("LastModified").and_then(v_time).unwrap_or(ctx.now),
                "Size": body_string(d).len(),
            })
        })
        .collect();
    Ok(json!({"DashboardEntries": entries}))
}

fn get_dashboard(ctx: &mut Ctx, input: &Value) -> Out {
    let name = s(input, "DashboardName");
    match ctx.get("dashboards", &name) {
        Some(d) => Ok(
            json!({"DashboardArn": dashboard_arn(ctx, &name), "DashboardBody": body_string(&d), "DashboardName": name}),
        ),
        None => Err((
            StatusCode::NOT_FOUND,
            "ResourceNotFound",
            format!("Dashboard {name} does not exist"),
        )),
    }
}

fn put_dashboard(ctx: &mut Ctx, input: &Value) -> Out {
    let name = s(input, "DashboardName");
    let body = s(input, "DashboardBody");
    if serde_json::from_str::<Value>(&body).is_err() {
        return Err((
            StatusCode::BAD_REQUEST,
            "InvalidParameterInput",
            "The field DashboardBody must be a valid JSON object".into(),
        ));
    }
    ctx.put(
        "dashboards",
        &name,
        json!({"DashboardBody": body, "LastModified": iso(ctx.now)}),
    );
    Ok(json!({"DashboardValidationMessages": []}))
}

fn delete_dashboards(ctx: &mut Ctx, input: &Value) -> Out {
    let names = strings(input, "DashboardNames");
    if let Some(n) = names.iter().find(|n| ctx.get("dashboards", n).is_none()) {
        return Err((
            StatusCode::NOT_FOUND,
            "ResourceNotFound",
            format!("Dashboard {n} does not exist"),
        ));
    }
    for n in names {
        ctx.remove("dashboards", &n);
    }
    Ok(json!({}))
}

fn alarm_for_arn(ctx: &Ctx, arn: &str) -> Option<String> {
    ctx.docs("alarms")
        .into_iter()
        .find(|(n, a)| {
            a.get("AlarmArn")
                .and_then(Value::as_str)
                .unwrap_or(&alarm_arn(ctx, n))
                == arn
        })
        .map(|(n, _)| n)
}

fn not_found_arn(arn: &str) -> (StatusCode, &'static str, String) {
    (
        StatusCode::NOT_FOUND,
        "ResourceNotFoundException",
        format!("{arn} not found"),
    )
}

fn list_tags(ctx: &mut Ctx, input: &Value) -> Out {
    let arn = s(input, "ResourceARN");
    let name = alarm_for_arn(ctx, &arn).ok_or_else(|| not_found_arn(&arn))?;
    let a = ctx.get("alarms", &name).expect("found");
    Ok(json!({"Tags": a.get("Tags").cloned().unwrap_or(json!([]))}))
}

fn tag_resource(ctx: &mut Ctx, input: &Value) -> Out {
    let arn = s(input, "ResourceARN");
    let name = alarm_for_arn(ctx, &arn).ok_or_else(|| not_found_arn(&arn))?;
    let a = ctx.get_mut("alarms", &name).expect("found");
    let mut tags: Vec<Value> = a
        .get("Tags")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for t in input
        .get("Tags")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        tags.retain(|e| e["Key"] != t["Key"]);
        tags.push(t);
    }
    a["Tags"] = Value::Array(tags);
    Ok(json!({}))
}

fn untag_resource(ctx: &mut Ctx, input: &Value) -> Out {
    let arn = s(input, "ResourceARN");
    let name = alarm_for_arn(ctx, &arn).ok_or_else(|| not_found_arn(&arn))?;
    let keys = strings(input, "TagKeys");
    let a = ctx.get_mut("alarms", &name).expect("found");
    if let Some(tags) = a.get_mut("Tags").and_then(Value::as_array_mut) {
        tags.retain(|t| !keys.iter().any(|k| t["Key"] == k.as_str()));
    }
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistics() {
        let v = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(stat(&v, "Average"), Some(2.5));
        assert_eq!(stat(&v, "Maximum"), Some(4.0));
        assert_eq!(stat(&v, "p50"), Some(2.0));
        assert_eq!(stat(&v, "SampleCount"), Some(4.0));
        assert_eq!(stat(&[], "Sum"), None);
    }

    #[test]
    fn metrics_insights_group_order_limit() {
        let series = |id: &str, v: f64| Series {
            namespace: "AWS/EC2".into(),
            name: "CPUUtilization".into(),
            dims: vec![("InstanceId".into(), id.into())],
            unit: None,
            points: vec![(10, v), (20, v)],
        };
        let all = vec![series("i-1", 5.0), series("i-2", 50.0), series("i-3", 20.0)];
        let q = "SELECT AVG(\"CPUUtilization\") FROM SCHEMA(\"AWS/EC2\", \"InstanceId\") GROUP BY \"InstanceId\" ORDER BY AVG() DESC LIMIT 2";
        let out = metrics_insights(&all, q, 0, 100, 100).unwrap();
        let labels: Vec<&str> = out.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["i-2", "i-3"]);
        let filtered = metrics_insights(
            &all,
            "SELECT MAX(CPUUtilization) FROM SCHEMA(\"AWS/EC2\", InstanceId) WHERE InstanceId = 'i-1'",
            0,
            100,
            100,
        )
        .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1, vec![(0, 5.0)]);
    }

    #[test]
    fn expressions() {
        let refs = vec![
            ("m1".to_owned(), vec![(0, 2.0), (60, 4.0)]),
            ("m2".to_owned(), vec![(0, 4.0), (60, 8.0)]),
        ];
        assert_eq!(
            eval_expression("100*(m1/m2)", &refs),
            Some(vec![(0, 50.0), (60, 50.0)])
        );
        assert_eq!(
            eval_expression("m1 + m2", &refs),
            Some(vec![(0, 6.0), (60, 12.0)])
        );
        assert_eq!(eval_expression("ANOMALY_DETECTION_BAND(m1)", &refs), None);
    }
}
