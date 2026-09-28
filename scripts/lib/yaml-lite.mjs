/**
 * yaml-lite.mjs — a minimal parser for the YAML subset used by
 * `weftos-package.yaml`, `team.yaml`, and AGENT.md/SKILL.md frontmatter
 * (AD-1). Not a general YAML parser: no anchors/aliases, no multi-document
 * streams, no complex flow collections beyond a single-line `[a, b, c]` or
 * `{}` / `[]`. Deliberately small — see docs/research/agent-directory/design.md
 * §1.1 ("Parse YAML minimally without adding npm deps").
 *
 * Supports: block maps, block lists (of scalars or maps), inline flow lists
 * (`[a, b]`), quoted and unquoted scalars, folded (`>`) and literal (`|`)
 * block scalars with optional `-` chomping, plain-scalar line continuation
 * (a value that keeps flowing onto more-indented following lines), and
 * trailing ` # comment` stripping outside quotes.
 */

class YamlError extends Error {
  constructor(message, lineNo) {
    super(lineNo ? `${message} (line ${lineNo})` : message);
    this.name = "YamlError";
  }
}

function indentOf(line) {
  const m = /^ */.exec(line);
  return m[0].length;
}

function isBlankOrComment(line) {
  const t = line.trim();
  return t === "" || t.startsWith("#");
}

/** Strip a trailing ` #comment` that isn't inside a quoted string. */
function stripInlineComment(line) {
  let inSingle = false;
  let inDouble = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (c === "'" && !inDouble) inSingle = !inSingle;
    else if (c === '"' && !inSingle) inDouble = !inDouble;
    else if (c === "#" && !inSingle && !inDouble) {
      if (i === 0 || /\s/.test(line[i - 1])) {
        return line.slice(0, i).replace(/\s+$/, "");
      }
    }
  }
  return line;
}

function parseScalar(raw) {
  const s = raw.trim();
  if (s === "") return null;
  if (s === "null" || s === "~") return null;
  if (s === "true") return true;
  if (s === "false") return false;
  if (/^-?\d+$/.test(s)) return Number(s);
  if (
    (s.startsWith('"') && s.endsWith('"') && s.length >= 2) ||
    (s.startsWith("'") && s.endsWith("'") && s.length >= 2)
  ) {
    return s.slice(1, -1);
  }
  if (s.startsWith("[") && s.endsWith("]")) {
    const inner = s.slice(1, -1).trim();
    if (inner === "") return [];
    return inner.split(",").map((v) => parseScalar(v.trim()));
  }
  if (s === "{}") return {};
  return s;
}

/** Find the next non-blank, non-comment physical line at or after `from`. */
function nextContentIndex(lines, from) {
  let i = from;
  while (i < lines.length && isBlankOrComment(lines[i])) i++;
  return i < lines.length ? i : null;
}

function parseBlockScalar(lines, cursor, marker, keyIndent) {
  const folded = marker[0] === ">";
  const strip = marker.includes("-");
  const keep = marker.includes("+");
  const contentLines = [];
  let blockIndent = null;
  while (cursor.i < lines.length) {
    const line = lines[cursor.i];
    if (line.trim() === "") {
      contentLines.push("");
      cursor.i++;
      continue;
    }
    const ind = indentOf(line);
    if (ind <= keyIndent) break;
    if (blockIndent === null) blockIndent = ind;
    contentLines.push(line.slice(blockIndent));
    cursor.i++;
  }
  while (contentLines.length && contentLines[contentLines.length - 1] === "") {
    contentLines.pop();
  }
  let text;
  if (folded) {
    const paragraphs = [];
    let current = [];
    for (const l of contentLines) {
      if (l === "") {
        paragraphs.push(current.join(" "));
        current = [];
      } else {
        current.push(l);
      }
    }
    paragraphs.push(current.join(" "));
    text = paragraphs.join("\n").trim();
  } else {
    text = contentLines.join("\n");
  }
  // Chomping (`-`/`+`) distinctions don't matter for our purposes — this
  // parser feeds config/validation logic, not round-tripped YAML — so we
  // always return trimmed text.
  void keep;
  return text.replace(/\s+$/, "");
}

/** Consume plain-scalar continuation lines more indented than `keyIndent`. */
function consumeContinuation(lines, cursor, keyIndent, initial) {
  let value = initial;
  while (cursor.i < lines.length) {
    const idx = nextContentIndex(lines, cursor.i);
    if (idx === null) break;
    const ind = indentOf(lines[idx]);
    if (ind <= keyIndent) break;
    value += " " + stripInlineComment(lines[idx]).trim();
    cursor.i = idx + 1;
  }
  return value.trim();
}

function parseMap(lines, cursor, indent) {
  const result = {};
  while (cursor.i < lines.length) {
    const idx = nextContentIndex(lines, cursor.i);
    if (idx === null) break;
    cursor.i = idx;
    const line = lines[cursor.i];
    const ind = indentOf(line);
    if (ind < indent) break;
    if (ind > indent) {
      throw new YamlError(`unexpected indent inside map`, cursor.i + 1);
    }
    const stripped = stripInlineComment(line).trim();
    if (stripped.startsWith("- ") || stripped === "-") {
      throw new YamlError(`expected a map key but found a list item`, cursor.i + 1);
    }
    const colon = stripped.indexOf(":");
    if (colon === -1) {
      throw new YamlError(`expected "key: value"`, cursor.i + 1);
    }
    const key = stripped.slice(0, colon).trim().replace(/^["']|["']$/g, "");
    let valuePart = stripped.slice(colon + 1).trim();
    cursor.i++;

    if (valuePart === "") {
      const nIdx = nextContentIndex(lines, cursor.i);
      if (nIdx === null) {
        result[key] = null;
        continue;
      }
      const nInd = indentOf(lines[nIdx]);
      if (nInd <= indent) {
        result[key] = null;
        continue;
      }
      const nTrim = stripInlineComment(lines[nIdx]).trim();
      result[key] = nTrim.startsWith("- ") || nTrim === "-"
        ? parseList(lines, cursor, nInd)
        : parseMap(lines, cursor, nInd);
    } else if (/^[|>][+-]?$/.test(valuePart)) {
      result[key] = parseBlockScalar(lines, cursor, valuePart, ind);
    } else {
      const scalar = parseScalar(valuePart);
      result[key] = typeof scalar === "string"
        ? consumeContinuation(lines, cursor, ind, scalar)
        : scalar;
    }
  }
  return result;
}

function parseList(lines, cursor, indent) {
  const result = [];
  while (cursor.i < lines.length) {
    const idx = nextContentIndex(lines, cursor.i);
    if (idx === null) break;
    cursor.i = idx;
    const line = lines[cursor.i];
    const ind = indentOf(line);
    if (ind < indent) break;
    if (ind > indent) {
      throw new YamlError(`unexpected indent inside list`, cursor.i + 1);
    }
    const stripped = stripInlineComment(line).trim();
    if (!(stripped.startsWith("- ") || stripped === "-")) break;
    const rest = stripped.slice(1).trim();
    const dashCol = ind + (line.slice(ind).indexOf("-")) + 2; // column where item content starts
    cursor.i++;

    if (rest === "") {
      const nIdx = nextContentIndex(lines, cursor.i);
      if (nIdx !== null && indentOf(lines[nIdx]) > ind) {
        const nInd = indentOf(lines[nIdx]);
        const nTrim = stripInlineComment(lines[nIdx]).trim();
        result.push(
          nTrim.startsWith("- ") || nTrim === "-"
            ? parseList(lines, cursor, nInd)
            : parseMap(lines, cursor, nInd)
        );
      } else {
        result.push(null);
      }
      continue;
    }

    const colon = rest.indexOf(":");
    const looksLikeKey = colon !== -1 && (colon === rest.length - 1 || rest[colon + 1] === " ");
    if (looksLikeKey) {
      // Rewind: re-parse this list item as a one-line-started map, whose
      // key column is `dashCol`. Build a synthetic first line and splice it
      // in place of consumed content by parsing manually here.
      const key = rest.slice(0, colon).trim();
      let valuePart = rest.slice(colon + 1).trim();
      const item = {};
      if (valuePart === "") {
        const nIdx = nextContentIndex(lines, cursor.i);
        if (nIdx !== null && indentOf(lines[nIdx]) > dashCol) {
          const nInd = indentOf(lines[nIdx]);
          const nTrim = stripInlineComment(lines[nIdx]).trim();
          item[key] = nTrim.startsWith("- ") || nTrim === "-"
            ? parseList(lines, cursor, nInd)
            : parseMap(lines, cursor, nInd);
        } else {
          item[key] = null;
        }
      } else if (/^[|>][+-]?$/.test(valuePart)) {
        item[key] = parseBlockScalar(lines, cursor, valuePart, dashCol);
      } else {
        const scalar = parseScalar(valuePart);
        item[key] = typeof scalar === "string"
          ? consumeContinuation(lines, cursor, dashCol, scalar)
          : scalar;
      }
      // Sibling keys of this same map item, at the dash's content column.
      const rest2 = parseMap(lines, cursor, dashCol);
      Object.assign(item, rest2);
      result.push(item);
    } else {
      result.push(parseScalar(rest));
    }
  }
  return result;
}

/**
 * Parse a YAML-subset document into a plain JS value (object, array,
 * scalar, or null for an empty document).
 */
export function parseYaml(text) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const cursor = { i: 0 };
  const idx = nextContentIndex(lines, 0);
  if (idx === null) return null;
  cursor.i = idx;
  const indent = indentOf(lines[cursor.i]);
  const trimmed = stripInlineComment(lines[cursor.i]).trim();
  const value = trimmed.startsWith("- ") || trimmed === "-"
    ? parseList(lines, cursor, indent)
    : parseMap(lines, cursor, indent);
  return value;
}

/**
 * Split a markdown file with `---` YAML frontmatter into
 * `{ frontmatter, body }`. Returns `frontmatter: null` if the file has no
 * frontmatter block.
 */
export function splitFrontmatter(text) {
  if (!text.startsWith("---")) return { frontmatter: null, body: text };
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (lines[0].trim() !== "---") return { frontmatter: null, body: text };
  let end = -1;
  for (let i = 1; i < lines.length; i++) {
    if (lines[i].trim() === "---") {
      end = i;
      break;
    }
  }
  if (end === -1) return { frontmatter: null, body: text };
  const fmText = lines.slice(1, end).join("\n");
  const body = lines.slice(end + 1).join("\n");
  const frontmatter = fmText.trim() === "" ? {} : parseYaml(fmText);
  return { frontmatter, body };
}

export { YamlError };
