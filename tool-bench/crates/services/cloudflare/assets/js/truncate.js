// Tool result rendering: a type-stripped copy of cloudflare/mcp@68c7ec0 src/truncate.ts.
// Exposes globalThis.__format(value, truncate) -> string.
(() => {
  const CHARS_PER_TOKEN = 4;
  const MAX_TOKENS = 6000;
  const MAX_CHARS = MAX_TOKENS * CHARS_PER_TOKEN;
  const TRUNCATION_MARKER = '--- TRUNCATED ---';
  const MIN_SLOT = 128;

  function stringifyResponse(content) {
    if (typeof content === 'string') return content;
    return JSON.stringify(content, null, 2) ?? String(content);
  }

  function truncateResponse(content) {
    if (typeof content === 'string') {
      if (content.length <= MAX_CHARS) return content;
      const json = parseJsonContainer(content);
      return json === undefined ? truncateText(content) : shrinkToText(json);
    }
    const compact = JSON.stringify(content);
    if (compact === undefined) return String(content);
    if (compact.length > MAX_CHARS) return shrinkToText(JSON.parse(compact));
    const pretty = JSON.stringify(content, null, 2);
    return pretty.length <= MAX_CHARS ? pretty : compact;
  }

  function truncateText(text) {
    const estimatedTokens = Math.ceil(text.length / CHARS_PER_TOKEN);
    return `${text.slice(0, MAX_CHARS)}\n\n${TRUNCATION_MARKER}\nResponse was ~${estimatedTokens.toLocaleString()} tokens (limit: ${MAX_TOKENS.toLocaleString()}). Use more specific queries to reduce response size.`;
  }

  function parseJsonContainer(text) {
    if (!/^\s*[[{]/.test(text)) return undefined;
    try {
      return JSON.parse(text);
    } catch {
      return undefined;
    }
  }

  function shrinkToText(value) {
    return JSON.stringify(shrink(value, MAX_CHARS));
  }

  function size(value) {
    return JSON.stringify(value).length;
  }
  function skeleton(count) {
    return 2 + Math.max(0, count - 1);
  }
  function keyCost(key) {
    return size(key) + 1;
  }
  function sum(values) {
    return values.reduce((total, value) => total + value, 0);
  }
  function shrinkable(value) {
    return typeof value === 'string' || (typeof value === 'object' && value !== null);
  }
  function floorSize(value, maxChars) {
    const full = size(value);
    return shrinkable(value) ? Math.min(full, MIN_SLOT, Math.max(2, Math.floor(maxChars / 2))) : full;
  }
  function keyFloor(value, maxChars) {
    if (typeof value !== 'object' || value === null || Array.isArray(value)) {
      return floorSize(value, maxChars);
    }
    const entries = Object.entries(value);
    return skeleton(entries.length) + sum(entries.map(([k, v]) => keyCost(k) + keyFloor(v, maxChars)));
  }

  function shrink(value, maxChars) {
    if (size(value) <= maxChars) return value;
    if (typeof value === 'string') return shrinkString(value, maxChars);
    if (Array.isArray(value)) return shrinkArray(value, maxChars);
    if (typeof value === 'object' && value !== null) return shrinkObject(value, maxChars);
    return value;
  }

  function shrinkString(value, maxChars) {
    const suffix = ` ${TRUNCATION_MARKER} ${value.length.toLocaleString()} chars`;
    let keep = Math.max(0, maxChars - suffix.length - 2);
    let out = value.slice(0, keep) + suffix;
    while (keep > 0 && size(out) > maxChars) {
      keep = Math.max(0, keep - (size(out) - maxChars));
      out = value.slice(0, keep) + suffix;
    }
    return size(out) <= maxChars ? out : TRUNCATION_MARKER.slice(0, Math.max(0, maxChars - 2));
  }

  function allocate(sizes, floors, available) {
    const allocation = [...floors];
    const wants = sizes.map((full, i) => full - floors[i]);
    const order = wants.map((_, i) => i).sort((a, b) => wants[a] - wants[b]);
    let remaining = available - sum(floors);
    let count = order.length;
    for (const i of order) {
      const extra = Math.min(wants[i], Math.floor(remaining / count));
      allocation[i] += extra;
      remaining -= extra;
      count--;
    }
    return allocation;
  }

  function shrinkArray(items, maxChars) {
    const marker = (dropped) => `${TRUNCATION_MARKER} ${dropped.toLocaleString()} more items`;
    const floors = items.map((item) => keyFloor(item, maxChars));

    let keep = items.length;
    if (sum(floors) + skeleton(keep) > maxChars) {
      let used = skeleton(0) + size(marker(items.length)) + 1;
      keep = 0;
      while (keep < items.length && used + floors[keep] + 1 <= maxChars) {
        used += floors[keep] + 1;
        keep++;
      }
      const room = maxChars - used - 1;
      const cutDown = floorSize(items[keep], maxChars);
      if ((keep === 0 || room >= maxChars / 4) && cutDown <= room) {
        floors[keep] = cutDown;
        keep++;
      }
    }

    const kept = items.slice(0, keep);
    const dropped = items.length - keep;
    const tail = dropped > 0 ? size(marker(dropped)) + (keep > 0 ? 1 : 0) : 0;
    const allocation = allocate(kept.map(size), floors.slice(0, keep), maxChars - skeleton(keep) - tail);
    const out = kept.map((item, i) => shrink(item, allocation[i]));
    if (dropped > 0 && size([...out, marker(dropped)]) <= maxChars) {
      out.push(marker(dropped));
    }
    return out;
  }

  function shrinkObject(value, maxChars) {
    const entries = Object.entries(value);
    const valueSizes = entries.map(([, v]) => size(v));

    const keyFloors = entries.map(([, v]) => keyFloor(v, maxChars));
    const fixed = skeleton(entries.length) + sum(entries.map(([k]) => keyCost(k)));
    if (fixed + sum(keyFloors) <= maxChars) {
      const allocation = allocate(valueSizes, keyFloors, maxChars - fixed);
      return Object.fromEntries(entries.map(([k, v], i) => [k, shrink(v, allocation[i])]));
    }

    let markerKey = TRUNCATION_MARKER;
    while (Object.hasOwn(value, markerKey)) markerKey += ' ';
    const floors = entries.map(([k, v]) => keyCost(k) + floorSize(v, maxChars));
    const noteFloor = (chars) => Math.min(chars, MIN_SLOT, Math.max(2, Math.floor(maxChars / 2)));

    const byValueSize = entries.map((_, i) => i).sort((a, b) => valueSizes[b] - valueSizes[a]);
    const dropped = new Set();
    let present = entries.length;
    let floorsSum = sum(floors);
    let namesLength = 0;
    const noteLength = () =>
      `${dropped.size.toLocaleString()} keys omitted: `.length + namesLength + 2 * (dropped.size - 1);
    const cost = () => {
      const marker = dropped.size > 0 ? keyCost(markerKey) + noteFloor(noteLength() + 2) : 0;
      return skeleton(present + (dropped.size > 0 ? 1 : 0)) + floorsSum + marker;
    };
    for (const i of byValueSize) {
      if (cost() <= maxChars) break;
      dropped.add(i);
      present--;
      floorsSum -= floors[i];
      namesLength += size(entries[i][0]) - 2;
    }

    const kept = entries.filter((_, i) => !dropped.has(i));
    const omitted = [...dropped].sort((a, b) => a - b).map((i) => entries[i][0]);
    const note = `${omitted.length.toLocaleString()} keys omitted: ${omitted.join(', ')}`;
    if (cost() > maxChars) {
      const room = maxChars - 2 - keyCost(markerKey);
      return room >= 2 ? { [markerKey]: shrink(note, room) } : {};
    }
    const all = [...kept];
    if (omitted.length > 0) all.push([markerKey, note]);
    const values = all.map(([, v]) => v);
    const allocation = allocate(
      values.map(size),
      values.map((v) => floorSize(v, maxChars)),
      maxChars - skeleton(all.length) - sum(all.map(([k]) => keyCost(k)))
    );
    return Object.fromEntries(all.map(([k, v], i) => [k, shrink(v, allocation[i])]));
  }

  // Code Mode truncates unless the call's `truncate` argument is false.
  globalThis.__format = (value, truncate) =>
    truncate === false ? stringifyResponse(value) : truncateResponse(value);
})();
