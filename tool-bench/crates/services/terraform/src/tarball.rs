//! Deterministic `.tar.gz` writer (ustar + stored deflate blocks) for Sentinel mock bundles.

use serde_json::Value;

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

fn octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    let s = format!("{value:0digits$o}");
    field[..digits].copy_from_slice(&s.as_bytes()[s.len() - digits..]);
    field[digits] = 0;
}

fn tar(files: &[(&str, String)], mtime: i64) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, content) in files {
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        octal(&mut header[100..108], 0o644);
        octal(&mut header[108..116], 0);
        octal(&mut header[116..124], 0);
        octal(&mut header[124..136], content.len() as u64);
        octal(&mut header[136..148], mtime.max(0) as u64);
        header[148..156].copy_from_slice(b"        ");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        let chk = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(chk.as_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(content.as_bytes());
        let pad = (512 - content.len() % 512) % 512;
        out.extend(std::iter::repeat_n(0u8, pad));
    }
    out.extend(std::iter::repeat_n(0u8, 1024));
    out
}

fn gzip_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0x00, 0xff];
    let mut chunks = data.chunks(65_535).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
    }
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        out.push(u8::from(last));
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

fn sentinel_value(v: &Value, indent: usize) -> String {
    let pad = "\t".repeat(indent + 1);
    let close = "\t".repeat(indent);
    match v {
        Value::Null => "null".to_owned(),
        Value::Object(map) if map.is_empty() => "{}".to_owned(),
        Value::Object(map) => {
            let items: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{pad}\"{k}\": {},", sentinel_value(v, indent + 1)))
                .collect();
            format!("{{\n{}\n{close}}}", items.join("\n"))
        }
        Value::Array(items) if items.is_empty() => "[]".to_owned(),
        Value::Array(items) => {
            let items: Vec<String> = items
                .iter()
                .map(|v| format!("{pad}{},", sentinel_value(v, indent + 1)))
                .collect();
            format!("[\n{}\n{close}]", items.join("\n"))
        }
        other => other.to_string(),
    }
}

/// A `sentinel-mock-bundle-v0` archive whose tfplan/v2 mock reflects the plan's resource changes.
pub fn sentinel_mock_bundle(plan_json: &Value, mtime: i64) -> Vec<u8> {
    let mut changes = serde_json::Map::new();
    for c in plan_json
        .get("resource_changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let address = c.get("address").and_then(Value::as_str).unwrap_or("");
        let mut entry = c.clone();
        entry["index"] = Value::Null;
        entry["module_address"] = Value::String(String::new());
        changes.insert(address.to_owned(), entry);
    }
    let tf = plan_json
        .get("terraform_version")
        .and_then(Value::as_str)
        .unwrap_or("1.9.5");
    let tfplan = format!(
        "terraform_version = \"{tf}\"\n\nplanned_values = {{\n\t\"outputs\":   {{}},\n\t\"resources\": {{}},\n}}\n\nresource_changes = {}\n\noutput_changes = {{}}\n\nraw = {{}}\n",
        sentinel_value(&Value::Object(changes), 0)
    );
    let files = [
        (
            "mock-tfconfig-v2.sentinel",
            "import \"strings\"\n\nproviders = {}\n\nresources = {}\n\nprovisioners = {}\n\nvariables = {}\n\noutputs = {}\n\nmodule_calls = {}\n\nstrip_index = func(addr) {\n\tparts = strings.split(addr, \".\")\n\treturn strings.join(parts, \".\")\n}\n".to_owned(),
        ),
        ("mock-tfplan-v2.sentinel", tfplan),
        (
            "mock-tfstate-v2.sentinel",
            format!("terraform_version = \"{tf}\"\n\noutputs = {{}}\n\nresources = {{}}\n"),
        ),
        (
            "mock-tfrun.sentinel",
            "id = \"run-mock\"\ncreated_at = \"\"\nmessage = \"\"\ncommit_sha = null\nspeculative = false\nis_destroy = false\nrefresh_only = false\nvariables = {}\n".to_owned(),
        ),
        (
            "sentinel.hcl",
            "mock \"tfconfig/v2\" {\n  module {\n    source = \"mock-tfconfig-v2.sentinel\"\n  }\n}\n\nmock \"tfplan/v2\" {\n  module {\n    source = \"mock-tfplan-v2.sentinel\"\n  }\n}\n\nmock \"tfstate/v2\" {\n  module {\n    source = \"mock-tfstate-v2.sentinel\"\n  }\n}\n\nmock \"tfrun\" {\n  module {\n    source = \"mock-tfrun.sentinel\"\n  }\n}\n".to_owned(),
        ),
    ];
    gzip_stored(&tar(&files, mtime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_reference() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn bundle_is_gzip_and_deterministic() {
        let plan = serde_json::json!({ "resource_changes": [{ "address": "aws_s3_bucket.logs", "change": { "actions": ["create"] } }] });
        let a = sentinel_mock_bundle(&plan, 1_700_000_000);
        let b = sentinel_mock_bundle(&plan, 1_700_000_000);
        assert_eq!(a, b);
        assert_eq!(&a[..2], &[0x1f, 0x8b]);
    }
}
