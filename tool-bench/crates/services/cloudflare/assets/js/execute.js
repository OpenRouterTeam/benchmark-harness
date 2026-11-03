// `cloudflare.request()` and the `execute` runner: the sandbox module of cloudflare/mcp@68c7ec0
// src/tools/execute.ts, with `fetch` replaced by the host binding `__cf_fetch`, which sends
// the request to the mock Cloudflare API in-process. Response handling is unchanged.
(() => {
  const { utf8Decode } = globalThis.__bytes;
  const hostFetch = globalThis.__cf_fetch;

  function apiError(method, path, response, details) {
    const hint = response.hint;
    const error = new Error("Cloudflare API error: " + String(method).toUpperCase() + " " + String(path).split("?")[0] +
      " returned HTTP " + response.status + (details ? ": " + details.replace(/[. ]+$/, "") : "") +
      (hint ? ". " + hint : ""));
    return error;
  }

  function mediaType(value) {
    return (value || "").split(";", 1)[0].trim().toLowerCase();
  }

  function isJsonType(type) {
    return /^application\/(?:[\w.-]+\+)?json$/.test(type);
  }

  const BINARY_TYPES = /^(?:image|audio|video|font)\/|^application\/(?:pdf|zip|gzip|x-gzip|x-tar|x-7z-compressed|wasm|vnd\.tcpdump\.pcap|vnd\.google\.protobuf|x-protobuf)$/;

  function decodeBody(bytes, type) {
    if (BINARY_TYPES.test(type)) return bytes;
    try {
      return utf8Decode(bytes, true);
    } catch {
      return bytes;
    }
  }

  function isRawBody(body) {
    return typeof body === "string" || body instanceof ArrayBuffer || ArrayBuffer.isView(body) ||
      body instanceof Blob || body instanceof FormData || body instanceof URLSearchParams;
  }

  function isJsonText(text) {
    try {
      JSON.parse(text);
      return true;
    } catch {
      return false;
    }
  }

  function encodeBody(body, contentType, rawBody) {
    if (body === undefined || body === null) {
      return { requestBody: undefined, requestType: contentType };
    }
    const asJson = !contentType || isJsonType(mediaType(contentType));
    if (rawBody || (isRawBody(body) && !(typeof body === "string" && asJson))) {
      const withoutBoundary = body instanceof FormData && contentType && !/boundary=/i.test(contentType);
      return { requestBody: body, requestType: withoutBoundary ? undefined : contentType };
    }
    const json = typeof body === "string" && isJsonText(body) ? body : JSON.stringify(body);
    return { requestBody: json, requestType: contentType || "application/json" };
  }

  function bytesOf(value) {
    if (value instanceof ArrayBuffer) value = new Uint8Array(value);
    return Array.from(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
  }

  // What fetch would put on the wire, as JSON the host can read.
  function wireBody(body, type) {
    if (body === undefined || body === null) return { body: { kind: "none" }, type };
    if (typeof body === "string") return { body: { kind: "text", text: body }, type: type || "text/plain;charset=UTF-8" };
    if (body instanceof URLSearchParams) {
      return { body: { kind: "text", text: body.toString() }, type: type || "application/x-www-form-urlencoded;charset=UTF-8" };
    }
    if (body instanceof Blob) return { body: { kind: "bytes", bytes: Array.from(body._bytes) }, type: type || body.type || undefined };
    if (body instanceof ArrayBuffer || ArrayBuffer.isView(body)) return { body: { kind: "bytes", bytes: bytesOf(body) }, type };
    if (body instanceof FormData) {
      const parts = [];
      for (const [name, value] of body.entries()) {
        if (typeof value === "string") parts.push({ name, text: value });
        else parts.push({ name, filename: value.name, type: value.type || "application/octet-stream", bytes: Array.from(value._bytes) });
      }
      return { body: { kind: "form", parts }, type: type || "multipart/form-data; boundary=----toolbench" };
    }
    return { body: { kind: "text", text: String(body) }, type: type || "text/plain;charset=UTF-8" };
  }

  function graphqlRefusal(errors) {
    const messages = errors.map((e) => String(e.message || ""));
    if (messages.some((m) => m.includes("'com.cloudflare.api.account.zone.analytics.read'"))) return "zone";
    if (messages.some((m) => /^authorization denied$/i.test(m.trim()) || m.includes("'com.cloudflare.api.account.analytics.read'"))) return "account";
    return undefined;
  }

  function encodeBytes(value, seen = new Set()) {
    if (value instanceof ArrayBuffer) value = new Uint8Array(value);
    if (ArrayBuffer.isView(value)) {
      const bytes = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
      let binary = "";
      for (let i = 0; i < bytes.length; i += 0x8000) {
        binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
      }
      return { base64: btoa(binary), byteLength: bytes.byteLength };
    }
    if (value === null || typeof value !== "object" || seen.has(value)) return value;
    const proto = Object.getPrototypeOf(value);
    if (!Array.isArray(value) && proto !== Object.prototype && proto !== null) return value;
    seen.add(value);
    let copy;
    for (const key of Object.keys(value)) {
      const encoded = encodeBytes(value[key], seen);
      if (encoded !== value[key]) {
        copy ??= Array.isArray(value) ? [...value] : { ...value };
        copy[key] = encoded;
      }
    }
    return copy ?? value;
  }

  const cloudflare = {
    async request(options) {
      const { method, path, query, body, contentType, rawBody } = options;

      const pairs = [];
      if (query) {
        for (const [key, value] of Object.entries(query)) {
          if (value !== undefined) {
            pairs.push([key, String(value)]);
          }
        }
      }

      const { requestBody, requestType } = encodeBody(body, contentType, rawBody);
      const wire = wireBody(requestBody, requestType);
      const response = JSON.parse(hostFetch(JSON.stringify({
        method: method === undefined ? "GET" : String(method),
        path: String(path),
        query: pairs,
        contentType: wire.type,
        body: wire.body,
      })));
      response.ok = response.status >= 200 && response.status < 300;
      const bytes = new Uint8Array(response.bytes);

      const responseType = mediaType(response.contentType);

      if (!isJsonType(responseType)) {
        if (!response.ok) {
          const text = utf8Decode(bytes, false);
          throw apiError(method, path, response, text.slice(0, 500));
        }
        const result = decodeBody(bytes, responseType);
        return { success: true, status: response.status, result };
      }

      const data = JSON.parse(utf8Decode(bytes, false));

      const cleanPath = String(path).split('?')[0].replace(/\/+$/, '');
      const isGraphQLEndpoint = cleanPath === '/graphql' || cleanPath.endsWith('/graphql');

      if (isGraphQLEndpoint) {
        const graphqlErrors = Array.isArray(data.errors) ? data.errors : [];
        const hasData = data.data !== null && data.data !== undefined;
        if (graphqlErrors.length > 0 && !hasData) {
          const msgs = graphqlErrors.map(e => e.message).join(", ");
          const error = new Error("GraphQL error: " + msgs);
          error.graphqlRefusal = graphqlRefusal(graphqlErrors);
          throw error;
        }
        return {
          success: graphqlErrors.length === 0,
          status: response.status,
          result: data.data,
          errors: graphqlErrors.map(e => ({
            code: e.extensions?.code || 0,
            message: e.message + (e.path ? ` (at ${e.path.join('.')})` : '')
          })),
          messages: graphqlErrors.length > 0 ? [{
            code: 0,
            message: `Partial response: ${graphqlErrors.length} error(s)`
          }] : []
        };
      }

      const isEnvelope = data !== null && typeof data === "object"
        && !Array.isArray(data) && typeof data.success === "boolean";
      if (!response.ok || (isEnvelope && !data.success)) {
        const errorList = Array.isArray(data?.errors) ? data.errors : [];
        const errors = errorList.map(e => (e.code === undefined ? "" : e.code + ": ") + e.message +
          (e.documentation_url ? " (" + e.documentation_url + ")" : "")).join(", ");
        throw apiError(method, path, response, errors);
      }

      return isEnvelope
        ? { ...data, status: response.status }
        : { success: true, status: response.status, result: data };
    }
  };

  globalThis.cloudflare = cloudflare;

  globalThis.__runExecute = async (fn, truncate) => {
    try {
      const result = await fn();
      globalThis.__outcome = JSON.stringify({ ok: true, text: __format(encodeBytes(result), truncate) });
    } catch (err) {
      globalThis.__outcome = JSON.stringify({ ok: false, message: err instanceof Error ? err.message : String(err) });
    }
  };

  // The `search` sandbox: no network, the spec as `spec`.
  globalThis.__runSearch = async (fn, truncate) => {
    try {
      const result = await fn();
      globalThis.__outcome = JSON.stringify({ ok: true, text: __format(result, truncate) });
    } catch (err) {
      globalThis.__outcome = JSON.stringify({ ok: false, message: err instanceof Error ? err.message : String(err) });
    }
  };
})();
