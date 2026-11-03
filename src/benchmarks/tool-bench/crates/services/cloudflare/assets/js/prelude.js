// Web platform globals the Workers sandbox gives agent code, implemented in plain JS for
// QuickJS. Only what Code Mode programs plausibly touch: text codecs, base64, Blob,
// FormData, URLSearchParams, URL, Headers, console, structuredClone. No timers, no
// randomness, no network: `cloudflare.request()` is the only way out.
(() => {
  const g = globalThis;

  // Number formatting as en-US (Node's default locale), used by the truncation notices.
  const plainLocale = Number.prototype.toLocaleString;
  Number.prototype.toLocaleString = function (locale, options) {
    const n = Number(this);
    if (!Number.isFinite(n) || options) return plainLocale.call(n, locale, options);
    const [int, frac] = String(Math.abs(Math.round(n * 1000) / 1000)).split('.');
    const grouped = int.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
    return (n < 0 ? '-' : '') + grouped + (frac ? '.' + frac : '');
  };

  function utf8Encode(text) {
    const out = [];
    for (let i = 0; i < text.length; i++) {
      let c = text.charCodeAt(i);
      if (c >= 0xd800 && c <= 0xdbff && i + 1 < text.length) {
        const d = text.charCodeAt(i + 1);
        if (d >= 0xdc00 && d <= 0xdfff) {
          c = 0x10000 + ((c - 0xd800) << 10) + (d - 0xdc00);
          i++;
        }
      }
      if (c < 0x80) out.push(c);
      else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 63));
      else if (c < 0x10000) out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
      else out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 63), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
    }
    return new Uint8Array(out);
  }

  function utf8Decode(bytes, fatal) {
    let out = '';
    for (let i = 0; i < bytes.length; ) {
      const b = bytes[i];
      let cp, n;
      if (b < 0x80) { cp = b; n = 1; }
      else if (b >= 0xc2 && b < 0xe0) { cp = b & 31; n = 2; }
      else if (b >= 0xe0 && b < 0xf0) { cp = b & 15; n = 3; }
      else if (b >= 0xf0 && b < 0xf5) { cp = b & 7; n = 4; }
      else {
        if (fatal) throw new TypeError('The encoded data was not valid for encoding utf-8');
        out += '�'; i++; continue;
      }
      let ok = i + n <= bytes.length;
      for (let k = 1; ok && k < n; k++) {
        const c = bytes[i + k];
        if ((c & 0xc0) !== 0x80) ok = false;
        else cp = (cp << 6) | (c & 63);
      }
      if (!ok) {
        if (fatal) throw new TypeError('The encoded data was not valid for encoding utf-8');
        out += '�'; i++; continue;
      }
      out += String.fromCodePoint(cp);
      i += n;
    }
    return out;
  }

  function toBytes(input) {
    if (input instanceof Uint8Array) return input;
    if (input instanceof ArrayBuffer) return new Uint8Array(input);
    if (ArrayBuffer.isView(input)) return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
    return utf8Encode(String(input));
  }

  class TextEncoder {
    get encoding() { return 'utf-8'; }
    encode(text = '') { return utf8Encode(String(text)); }
  }
  class TextDecoder {
    constructor(label = 'utf-8', options = {}) {
      this.fatal = !!options.fatal;
    }
    get encoding() { return 'utf-8'; }
    decode(input) { return input === undefined ? '' : utf8Decode(toBytes(input), this.fatal); }
  }

  const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  function btoa(binary) {
    binary = String(binary);
    let out = '';
    for (let i = 0; i < binary.length; i += 3) {
      const a = binary.charCodeAt(i), b = binary.charCodeAt(i + 1), c = binary.charCodeAt(i + 2);
      if (a > 255 || b > 255 || c > 255) throw new Error('Invalid character');
      const n = (a << 16) | ((b || 0) << 8) | (c || 0);
      out += B64[(n >> 18) & 63] + B64[(n >> 12) & 63] +
        (i + 1 < binary.length ? B64[(n >> 6) & 63] : '=') + (i + 2 < binary.length ? B64[n & 63] : '=');
    }
    return out;
  }
  function atob(text) {
    text = String(text).replace(/[\s=]/g, '');
    let out = '';
    for (let i = 0; i < text.length; i += 4) {
      const n = [0, 1, 2, 3].map((k) => (i + k < text.length ? B64.indexOf(text[i + k]) : 0));
      if (n.some((v) => v < 0)) throw new Error('The string to be decoded is not correctly encoded.');
      const v = (n[0] << 18) | (n[1] << 12) | (n[2] << 6) | n[3];
      out += String.fromCharCode((v >> 16) & 255);
      if (i + 2 < text.length) out += String.fromCharCode((v >> 8) & 255);
      if (i + 3 < text.length) out += String.fromCharCode(v & 255);
    }
    return out;
  }

  class Blob {
    constructor(parts = [], options = {}) {
      const chunks = parts.map((p) => (p instanceof Blob ? p._bytes : toBytes(p)));
      const size = chunks.reduce((n, c) => n + c.length, 0);
      const bytes = new Uint8Array(size);
      let at = 0;
      for (const c of chunks) { bytes.set(c, at); at += c.length; }
      this._bytes = bytes;
      this.type = options.type ? String(options.type).toLowerCase() : '';
    }
    get size() { return this._bytes.length; }
    async text() { return utf8Decode(this._bytes, false); }
    async arrayBuffer() { return this._bytes.slice().buffer; }
    async bytes() { return this._bytes.slice(); }
    slice(start, end, type) { return new Blob([this._bytes.slice(start, end)], { type }); }
  }
  class File extends Blob {
    constructor(parts, name, options = {}) {
      super(parts, options);
      this.name = String(name);
      this.lastModified = 0;
    }
  }

  class FormData {
    constructor() { this._entries = []; }
    _value(value, filename) {
      if (value instanceof Blob) {
        const name = filename !== undefined ? String(filename) : value instanceof File ? value.name : 'blob';
        return new File([value], name, { type: value.type });
      }
      return String(value);
    }
    append(name, value, filename) { this._entries.push([String(name), this._value(value, filename)]); }
    set(name, value, filename) { this.delete(name); this.append(name, value, filename); }
    delete(name) { this._entries = this._entries.filter(([n]) => n !== String(name)); }
    get(name) { const e = this._entries.find(([n]) => n === String(name)); return e ? e[1] : null; }
    getAll(name) { return this._entries.filter(([n]) => n === String(name)).map(([, v]) => v); }
    has(name) { return this._entries.some(([n]) => n === String(name)); }
    entries() { return this._entries[Symbol.iterator](); }
    keys() { return this._entries.map(([n]) => n)[Symbol.iterator](); }
    values() { return this._entries.map(([, v]) => v)[Symbol.iterator](); }
    forEach(fn, self) { for (const [n, v] of this._entries) fn.call(self, v, n, this); }
    [Symbol.iterator]() { return this.entries(); }
  }

  const encodeComponent = (s) => encodeURIComponent(s).replace(/%20/g, '+');
  const decodeComponent = (s) => decodeURIComponent(s.replace(/\+/g, ' '));
  class URLSearchParams {
    constructor(init) {
      this._list = [];
      if (init === undefined || init === null) return;
      if (typeof init === 'string') {
        for (const part of init.replace(/^\?/, '').split('&')) {
          if (!part) continue;
          const i = part.indexOf('=');
          this._list.push(i < 0 ? [decodeComponent(part), ''] : [decodeComponent(part.slice(0, i)), decodeComponent(part.slice(i + 1))]);
        }
      } else if (init instanceof URLSearchParams) {
        this._list = init._list.map(([k, v]) => [k, v]);
      } else if (typeof init[Symbol.iterator] === 'function') {
        for (const [k, v] of init) this._list.push([String(k), String(v)]);
      } else {
        for (const [k, v] of Object.entries(init)) this._list.push([k, String(v)]);
      }
    }
    append(k, v) { this._list.push([String(k), String(v)]); }
    set(k, v) {
      const i = this._list.findIndex(([n]) => n === String(k));
      if (i < 0) return this.append(k, v);
      this._list[i] = [String(k), String(v)];
      this._list = this._list.filter(([n], j) => j <= i || n !== String(k));
    }
    get(k) { const e = this._list.find(([n]) => n === String(k)); return e ? e[1] : null; }
    getAll(k) { return this._list.filter(([n]) => n === String(k)).map(([, v]) => v); }
    has(k) { return this._list.some(([n]) => n === String(k)); }
    delete(k) { this._list = this._list.filter(([n]) => n !== String(k)); }
    entries() { return this._list.map(([k, v]) => [k, v])[Symbol.iterator](); }
    keys() { return this._list.map(([k]) => k)[Symbol.iterator](); }
    values() { return this._list.map(([, v]) => v)[Symbol.iterator](); }
    forEach(fn, self) { for (const [k, v] of this._list) fn.call(self, v, k, this); }
    get size() { return this._list.length; }
    [Symbol.iterator]() { return this.entries(); }
    toString() { return this._list.map(([k, v]) => encodeComponent(k) + '=' + encodeComponent(v)).join('&'); }
  }

  class URL {
    constructor(url, base) {
      url = String(url);
      if (base !== undefined && !/^[a-z][a-z0-9+.-]*:/i.test(url)) {
        const b = new URL(base);
        url = url.startsWith('/') ? b.origin + url : b.origin + b.pathname.replace(/[^/]*$/, '') + url;
      }
      const m = /^([a-z][a-z0-9+.-]*:)\/\/([^/?#]*)([^?#]*)(\?[^#]*)?(#.*)?$/i.exec(url);
      if (!m) throw new TypeError('Invalid URL: ' + url);
      this.protocol = m[1].toLowerCase();
      this.host = m[2];
      this.hostname = m[2].replace(/:\d+$/, '');
      this.port = (/:(\d+)$/.exec(m[2]) || [])[1] || '';
      this.pathname = m[3] || '/';
      this.searchParams = new URLSearchParams(m[4] || '');
      this.hash = m[5] || '';
    }
    get origin() { return this.protocol + '//' + this.host; }
    get search() { const s = this.searchParams.toString(); return s ? '?' + s : ''; }
    get href() { return this.origin + this.pathname + this.search + this.hash; }
    toString() { return this.href; }
    toJSON() { return this.href; }
  }

  class Headers {
    constructor(init) {
      this._map = new Map();
      if (init) for (const [k, v] of init instanceof Headers ? init.entries() : Array.isArray(init) ? init : Object.entries(init)) this.append(k, v);
    }
    append(k, v) { const key = String(k).toLowerCase(); this._map.set(key, this._map.has(key) ? this._map.get(key) + ', ' + v : String(v)); }
    set(k, v) { this._map.set(String(k).toLowerCase(), String(v)); }
    get(k) { const v = this._map.get(String(k).toLowerCase()); return v === undefined ? null : v; }
    has(k) { return this._map.has(String(k).toLowerCase()); }
    delete(k) { this._map.delete(String(k).toLowerCase()); }
    entries() { return [...this._map.entries()].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))[Symbol.iterator](); }
    forEach(fn, self) { for (const [k, v] of this.entries()) fn.call(self, v, k, this); }
    [Symbol.iterator]() { return this.entries(); }
  }

  const noop = () => {};
  const console = { log: noop, info: noop, warn: noop, error: noop, debug: noop, trace: noop, dir: noop, table: noop };

  function structuredClone(value) {
    return value === undefined ? undefined : JSON.parse(JSON.stringify(value));
  }

  // Function-constructor and eval are disabled in Workers dynamic isolates.
  const fetch = async () => {
    throw new Error('Direct fetch is not available here; use cloudflare.request()');
  };

  Object.assign(g, { TextEncoder, TextDecoder, btoa, atob, Blob, File, FormData, URLSearchParams, URL, Headers, console, structuredClone, fetch });
  g.__bytes = { utf8Encode, utf8Decode, toBytes };
})();
