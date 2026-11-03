"""Writes the trimmed Code Mode spec: the operations mock-cloudflare implements, verbatim
from cloudflare/mcp@68c7ec0 src/generated/spec/spec.json.txt (refs already resolved)."""
import json, sys

d = json.load(open('/tmp/cf-src-fork/repo/src/generated/spec/spec.json.txt'))
keep = {
    '/accounts': ['get'],
    '/accounts/{account_id}': ['get'],
    '/accounts/{account_id}/members': ['get', 'post'],
    '/accounts/{account_id}/members/{member_id}': ['get', 'put', 'delete'],
    '/accounts/{account_id}/tokens/verify': ['get'],
    '/user': ['get'],
    '/user/tokens/verify': ['get'],
    '/memberships': ['get'],
    '/memberships/{membership_id}': ['get'],
    '/zones': ['get', 'post'],
    '/zones/{zone_id}': ['get', 'patch', 'delete'],
    '/zones/{zone_id}/dns_records': ['get', 'post'],
    '/zones/{zone_id}/dns_records/batch': ['post'],
    '/zones/{zone_id}/dns_records/export': ['get'],
    '/zones/{zone_id}/dns_records/usage': ['get'],
    '/zones/{zone_id}/dns_records/{dns_record_id}': ['get', 'put', 'patch', 'delete'],
    '/zones/{zone_id}/purge_cache': ['post'],
    '/zones/{zone_id}/workers/routes': ['get', 'post'],
    '/zones/{zone_id}/workers/routes/{route_id}': ['get', 'put', 'delete'],
    '/accounts/{account_id}/workers/scripts': ['get'],
    '/accounts/{account_id}/workers/scripts/{script_name}': ['get', 'put', 'delete'],
    '/accounts/{account_id}/workers/scripts/{script_name}/content/v2': ['get'],
    '/accounts/{account_id}/workers/scripts/{script_name}/settings': ['get', 'patch'],
    '/accounts/{account_id}/workers/scripts/{script_name}/secrets': ['get', 'put'],
    '/accounts/{account_id}/workers/scripts/{script_name}/secrets/{secret_name}': ['get', 'delete'],
    '/accounts/{account_id}/storage/kv/namespaces': ['get', 'post'],
    '/accounts/{account_id}/storage/kv/namespaces/{namespace_id}': ['get', 'put', 'delete'],
    '/accounts/{account_id}/storage/kv/namespaces/{namespace_id}/bulk': ['put'],
    '/accounts/{account_id}/storage/kv/namespaces/{namespace_id}/bulk/delete': ['post'],
    '/accounts/{account_id}/storage/kv/namespaces/{namespace_id}/bulk/get': ['post'],
    '/accounts/{account_id}/storage/kv/namespaces/{namespace_id}/keys': ['get'],
    '/accounts/{account_id}/storage/kv/namespaces/{namespace_id}/metadata/{key_name}': ['get'],
    '/accounts/{account_id}/storage/kv/namespaces/{namespace_id}/values/{key_name}': ['get', 'put', 'delete'],
    '/accounts/{account_id}/r2/buckets': ['get', 'post'],
    '/accounts/{account_id}/r2/buckets/{bucket_name}': ['get', 'delete'],
    '/accounts/{account_id}/d1/database': ['get', 'post'],
    '/accounts/{account_id}/d1/database/{database_id}': ['get', 'delete'],
    '/accounts/{account_id}/d1/database/{database_id}/query': ['post'],
    '/accounts/{account_id}/d1/database/{database_id}/raw': ['post'],
}
out = {}
for path, item in d['paths'].items():
    if path in keep:
        out[path] = {m: item[m] for m in keep[path] if m in item}
        missing = [m for m in keep[path] if m not in item]
        if missing:
            print('missing', path, missing)
for p in keep:
    if p not in out:
        print('absent', p)
text = json.dumps({'paths': out}, separators=(',', ':'), ensure_ascii=False)
print(len(out), len(text))
open(sys.argv[1], 'w').write(text)
