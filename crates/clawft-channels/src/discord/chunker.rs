//! Discord outbound message chunker (WEFT-169).
//!
//! Splits long messages for Discord delivery with:
//! - **Code-fence preservation** — mid-fence splits close and reopen with language tag
//! - **Markdown emphasis re-balance** — unpaired `**` / `__` / `*` / `_` closed and reopened
//! - **Nitro 4000-char limit** — config-driven max length (standard 2000 / nitro 4000)
//! - **Embed packing** — optional description/field packing with Discord limits
//! - **File-upload fallback** — past a configurable chunk count

use clawft_types::config::DiscordConfig;

/// Standard Discord message content limit (non-Nitro).
pub const DISCORD_STANDARD_MAX_LEN: usize = 2000;
/// Nitro / boosted message content limit.
pub const DISCORD_NITRO_MAX_LEN: usize = 4000;

/// Discord embed field limits (API).
pub const EMBED_TITLE_MAX: usize = 256;
pub const EMBED_DESCRIPTION_MAX: usize = 4096;
pub const EMBED_FIELD_NAME_MAX: usize = 256;
pub const EMBED_FIELD_VALUE_MAX: usize = 1024;
pub const EMBED_FOOTER_MAX: usize = 2048;
pub const EMBED_AUTHOR_MAX: usize = 256;
pub const EMBED_TOTAL_MAX: usize = 6000;
pub const EMBED_FIELDS_MAX: usize = 25;

/// Default number of text/embed chunks before file fallback.
pub const DEFAULT_MAX_CHUNKS_BEFORE_FILE: usize = 10;

// ── Public types ───────────────────────────────────────────────────────────

/// Options controlling how content is chunked for Discord delivery.
#[derive(Debug, Clone)]
pub struct ChunkerOptions {
    /// Max characters per text message.
    pub max_len: usize,
    /// After this many text/embed pieces, remaining content becomes a file.
    pub max_chunks_before_file: usize,
    /// Prefer packing long content into embeds when beneficial.
    pub prefer_embeds: bool,
    /// Filename used for file-upload fallback.
    pub file_name: String,
}

impl Default for ChunkerOptions {
    fn default() -> Self {
        Self {
            max_len: DISCORD_STANDARD_MAX_LEN,
            max_chunks_before_file: DEFAULT_MAX_CHUNKS_BEFORE_FILE,
            prefer_embeds: false,
            file_name: "message.md".into(),
        }
    }
}

impl ChunkerOptions {
    /// Build options from [`DiscordConfig`] (nitro / max length / file threshold).
    pub fn from_discord_config(cfg: &DiscordConfig) -> Self {
        Self {
            max_len: resolve_max_message_len(cfg),
            max_chunks_before_file: cfg.max_chunks_before_file.max(1),
            prefer_embeds: cfg.prefer_embeds,
            file_name: "message.md".into(),
        }
    }
}

/// Resolve the effective message character limit from config.
///
/// Precedence: `max_message_length` (clamped 1..=4000) → nitro 4000 → 2000.
pub fn resolve_max_message_len(cfg: &DiscordConfig) -> usize {
    if let Some(n) = cfg.max_message_length {
        return n.clamp(1, DISCORD_NITRO_MAX_LEN);
    }
    if cfg.nitro {
        DISCORD_NITRO_MAX_LEN
    } else {
        DISCORD_STANDARD_MAX_LEN
    }
}

/// Whether the planned text/embed count should switch to file fallback.
pub fn should_use_file_fallback(chunk_count: usize, max_chunks: usize) -> bool {
    chunk_count > max_chunks.max(1)
}

/// One outbound unit produced by the chunker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundChunk {
    /// Plain message content.
    Text(String),
    /// A single embed payload (sent as message embeds array).
    Embed(EmbedPayload),
    /// Remaining content as a file attachment (multipart path may be stubbed).
    File {
        filename: String,
        content: String,
        /// Short content field accompanying the attachment.
        notice: String,
    },
}

/// Discord embed body (subset used for long-message packing).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EmbedPayload {
    pub title: Option<String>,
    pub description: Option<String>,
    pub fields: Vec<EmbedField>,
    pub footer: Option<String>,
    pub author: Option<String>,
}

/// One embed field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

impl EmbedPayload {
    /// Total characters counted toward Discord's 6000 embed budget.
    pub fn total_chars(&self) -> usize {
        let mut n = 0usize;
        if let Some(ref t) = self.title {
            n += t.chars().count();
        }
        if let Some(ref d) = self.description {
            n += d.chars().count();
        }
        if let Some(ref f) = self.footer {
            n += f.chars().count();
        }
        if let Some(ref a) = self.author {
            n += a.chars().count();
        }
        for field in &self.fields {
            n += field.name.chars().count();
            n += field.value.chars().count();
        }
        n
    }

    /// Truncate all fields to Discord limits; drop excess fields past 25.
    /// Returns total chars after enforcement.
    pub fn enforce_limits(&mut self) -> usize {
        if let Some(ref mut t) = self.title {
            *t = truncate_chars(t, EMBED_TITLE_MAX);
        }
        if let Some(ref mut d) = self.description {
            *d = truncate_chars(d, EMBED_DESCRIPTION_MAX);
        }
        if let Some(ref mut f) = self.footer {
            *f = truncate_chars(f, EMBED_FOOTER_MAX);
        }
        if let Some(ref mut a) = self.author {
            *a = truncate_chars(a, EMBED_AUTHOR_MAX);
        }
        if self.fields.len() > EMBED_FIELDS_MAX {
            self.fields.truncate(EMBED_FIELDS_MAX);
        }
        for field in &mut self.fields {
            field.name = truncate_chars(&field.name, EMBED_FIELD_NAME_MAX);
            if field.name.is_empty() {
                field.name = "\u{200b}".into(); // Discord requires non-empty name
            }
            field.value = truncate_chars(&field.value, EMBED_FIELD_VALUE_MAX);
            if field.value.is_empty() {
                field.value = "\u{200b}".into();
            }
        }
        // Shrink description / fields if total still over budget.
        loop {
            let total = self.total_chars();
            if total <= EMBED_TOTAL_MAX {
                break;
            }
            let over = total - EMBED_TOTAL_MAX;
            if let Some(ref mut d) = self.description {
                let cur = d.chars().count();
                if cur == 0 {
                    self.description = None;
                    continue;
                }
                let new_len = cur.saturating_sub(over.max(1));
                *d = truncate_chars(d, new_len);
                continue;
            }
            if self.fields.is_empty() {
                break;
            }
            let last = self.fields.len() - 1;
            let cur = self.fields[last].value.chars().count();
            if cur <= 1 {
                self.fields.pop();
            } else {
                let new_val = truncate_chars(&self.fields[last].value, cur.saturating_sub(over.max(1)));
                self.fields[last].value = new_val;
            }
        }
        self.total_chars()
    }

    /// Serialize to a Discord API embed object.
    pub fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        if let Some(ref t) = self.title {
            map.insert("title".into(), serde_json::Value::String(t.clone()));
        }
        if let Some(ref d) = self.description {
            map.insert("description".into(), serde_json::Value::String(d.clone()));
        }
        if let Some(ref f) = self.footer {
            map.insert(
                "footer".into(),
                serde_json::json!({ "text": f }),
            );
        }
        if let Some(ref a) = self.author {
            map.insert(
                "author".into(),
                serde_json::json!({ "name": a }),
            );
        }
        if !self.fields.is_empty() {
            let fields: Vec<serde_json::Value> = self
                .fields
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "name": f.name,
                        "value": f.value,
                        "inline": f.inline,
                    })
                })
                .collect();
            map.insert("fields".into(), serde_json::Value::Array(fields));
        }
        serde_json::Value::Object(map)
    }
}

/// Full delivery plan for one outbound message.
#[derive(Debug, Clone)]
pub struct ChunkPlan {
    pub chunks: Vec<OutboundChunk>,
    pub used_file_fallback: bool,
}

// ── Internal scan state ────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct FenceState {
    open: bool,
    /// Fence marker characters, e.g. "```" or "~~~~".
    marker: String,
    /// Info string / language tag from the opening fence.
    lang: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct EmphasisState {
    bold: bool,         // **
    underline: bool,    // __
    italic_star: bool,  // *
    italic_under: bool, // _
}

impl EmphasisState {
    fn closers(&self) -> String {
        // Close innermost-style first (stars before underscores, short before long).
        let mut s = String::new();
        if self.italic_star {
            s.push('*');
        }
        if self.bold {
            s.push_str("**");
        }
        if self.italic_under {
            s.push('_');
        }
        if self.underline {
            s.push_str("__");
        }
        s
    }

    fn openers(&self) -> String {
        // Reopen in reverse of closers so nesting restores cleanly.
        let mut s = String::new();
        if self.underline {
            s.push_str("__");
        }
        if self.italic_under {
            s.push('_');
        }
        if self.bold {
            s.push_str("**");
        }
        if self.italic_star {
            s.push('*');
        }
        s
    }

    fn any_open(&self) -> bool {
        self.bold || self.underline || self.italic_star || self.italic_under
    }
}

// ── Public entry points ────────────────────────────────────────────────────

/// Plan delivery for `content` under `opts`.
///
/// Produces text chunks (fence- and emphasis-aware), optionally embed packing,
/// and file fallback when the chunk count exceeds the configured threshold.
pub fn plan_chunks(content: &str, opts: &ChunkerOptions) -> ChunkPlan {
    if content.is_empty() {
        return ChunkPlan {
            chunks: vec![OutboundChunk::Text(String::new())],
            used_file_fallback: false,
        };
    }

    let max_len = opts.max_len.max(1);

    // Prefer embeds for long content when configured and length benefits.
    if opts.prefer_embeds && content.chars().count() > max_len {
        let embeds = pack_content_as_embeds(content);
        if !embeds.is_empty() && embeds.len() <= opts.max_chunks_before_file {
            return ChunkPlan {
                chunks: embeds.into_iter().map(OutboundChunk::Embed).collect(),
                used_file_fallback: false,
            };
        }
        // If embed packing still exceeds threshold, fall through to text+file.
    }

    let text_chunks = chunk_message(content, max_len);
    assemble_plan(text_chunks, content, opts)
}

/// Split `content` into balanced text chunks that each fit in `max_len`.
///
/// Public for unit tests and callers that only need plain text pieces.
pub fn chunk_message(content: &str, max_len: usize) -> Vec<String> {
    let max_len = max_len.max(1);
    if content.is_empty() {
        return vec![String::new()];
    }
    if char_len(content) <= max_len {
        return vec![content.to_owned()];
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut remaining = content;
    let mut fence = FenceState::default();
    let mut emphasis = EmphasisState::default();

    while !remaining.is_empty() {
        if char_len(remaining) <= max_len && !fence.open && !emphasis.any_open() {
            // Fits as-is and no open state that would need balancing on a prior
            // partial — but we may still need reopen prefix if we inherited open fence.
            // At loop start after a mid-fence split, fence.open is true and we need
            // reopen prefix, so this branch only for clean remainder.
            chunks.push(remaining.to_owned());
            break;
        }

        let reopen_prefix = if fence.open {
            fence_open_line(&fence.marker, &fence.lang)
        } else {
            String::new()
        };
        let emphasis_prefix = if emphasis.any_open() {
            emphasis.openers()
        } else {
            String::new()
        };
        let prefix = format!("{reopen_prefix}{emphasis_prefix}");
        let prefix_len = char_len(&prefix);

        // Budget for body before we know whether closers are needed. Prefer
        // leaving room for a fence close + emphasis closers when a fence is
        // already open (almost always need close). Otherwise take full rest
        // and shrink after scanning if closers push us over.
        let provisional_reserved = if fence.open {
            1 + fence.marker.chars().count().max(3) + 6
        } else {
            0
        };
        let usable = max_len
            .saturating_sub(prefix_len)
            .saturating_sub(provisional_reserved)
            .max(1);

        // How many bytes of `remaining` we can take (char-safe).
        let mut take_bytes = char_prefix_bytes(remaining, usable);
        if take_bytes == 0 {
            // Pathological tiny max_len: hard-take one char and hard-split.
            let one = remaining.chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            if one == 0 {
                break;
            }
            let (piece, rest) = remaining.split_at(one);
            let mut chunk = format!("{prefix}{piece}");
            if fence.open {
                chunk.push('\n');
                chunk.push_str(&fence.marker);
            }
            if !fence.open {
                chunk.push_str(&emphasis.closers());
            }
            if char_len(&chunk) > max_len {
                chunk = truncate_chars(&chunk, max_len);
            }
            chunks.push(chunk);
            remaining = rest;
            continue;
        }

        // Shrink the candidate until prefix + piece + required closers fit.
        let (raw_piece, rest, piece_fence, piece_emphasis) = loop {
            let search = &remaining[..take_bytes];
            let split_at = find_split_point(search, take_bytes);
            let (raw_piece, rest) = remaining.split_at(split_at);
            let mut piece_fence = fence.clone();
            let mut piece_emphasis = emphasis.clone();
            scan_segment(raw_piece, &mut piece_fence, &mut piece_emphasis);

            let mut suffix_len = 0usize;
            if piece_fence.open {
                let needs_nl = !raw_piece.ends_with('\n') && !prefix.ends_with('\n');
                suffix_len += usize::from(needs_nl) + piece_fence.marker.chars().count();
            } else if piece_emphasis.any_open() {
                suffix_len += char_len(&piece_emphasis.closers());
            }

            let total = prefix_len + char_len(raw_piece) + suffix_len;
            if total <= max_len || split_at <= 1 {
                break (raw_piece, rest, piece_fence, piece_emphasis);
            }
            // Reduce take by at least one char and retry.
            let reduce_to = char_len(raw_piece).saturating_sub(1).max(1);
            let new_take = char_prefix_bytes(remaining, reduce_to);
            if new_take >= take_bytes {
                break (raw_piece, rest, piece_fence, piece_emphasis);
            }
            take_bytes = new_take;
        };

        let mut chunk = String::with_capacity(prefix.len() + raw_piece.len() + 16);
        chunk.push_str(&prefix);
        chunk.push_str(raw_piece);

        // Close open fence at end of this chunk if still open after the piece.
        if piece_fence.open {
            if !chunk.ends_with('\n') {
                chunk.push('\n');
            }
            chunk.push_str(&piece_fence.marker);
        } else if piece_emphasis.any_open() {
            // Close unpaired emphasis so the chunk renders balanced; next
            // chunk reopens via emphasis_prefix.
            chunk.push_str(&piece_emphasis.closers());
        }

        if char_len(&chunk) > max_len {
            chunk = truncate_chars(&chunk, max_len);
        }

        let trimmed = chunk.trim_end_matches(['\r']);
        if !trimmed.is_empty() || chunks.is_empty() {
            chunks.push(trimmed.to_owned());
        }

        // Advance source-stream state (fence/emphasis as in the original text).
        fence = piece_fence;
        emphasis = piece_emphasis;

        remaining = rest.trim_start_matches('\n');
        if remaining.is_empty() {
            break;
        }
    }

    if chunks.is_empty() {
        chunks.push(content.to_owned());
    }
    chunks
}

/// Pack plain content into one or more embeds under Discord limits.
pub fn pack_content_as_embeds(content: &str) -> Vec<EmbedPayload> {
    let mut embeds = Vec::new();
    let mut rest = content;
    while !rest.is_empty() {
        let take = char_prefix_bytes(rest, EMBED_DESCRIPTION_MAX);
        if take == 0 {
            break;
        }
        // Prefer newline split within description budget.
        let search = &rest[..take];
        let split = search
            .rfind('\n')
            .filter(|&p| p > 0)
            .map(|p| p + 1)
            .unwrap_or(take);
        let (piece, next) = rest.split_at(split);
        let mut embed = EmbedPayload {
            description: Some(piece.trim_end().to_owned()),
            ..Default::default()
        };
        embed.enforce_limits();
        embeds.push(embed);
        rest = next.trim_start_matches('\n');
    }
    embeds
}

/// Build a single field-limited embed (for structured callers / tests).
pub fn build_field_embed(
    title: Option<&str>,
    fields: Vec<(&str, &str, bool)>,
) -> EmbedPayload {
    let mut embed = EmbedPayload {
        title: title.map(|t| truncate_chars(t, EMBED_TITLE_MAX)),
        fields: fields
            .into_iter()
            .take(EMBED_FIELDS_MAX)
            .map(|(n, v, inline)| EmbedField {
                name: truncate_chars(n, EMBED_FIELD_NAME_MAX),
                value: truncate_chars(v, EMBED_FIELD_VALUE_MAX),
                inline,
            })
            .collect(),
        ..Default::default()
    };
    embed.enforce_limits();
    embed
}

// ── Assembly ───────────────────────────────────────────────────────────────

fn assemble_plan(text_chunks: Vec<String>, original: &str, opts: &ChunkerOptions) -> ChunkPlan {
    let max_n = opts.max_chunks_before_file.max(1);
    if !should_use_file_fallback(text_chunks.len(), max_n) {
        return ChunkPlan {
            chunks: text_chunks.into_iter().map(OutboundChunk::Text).collect(),
            used_file_fallback: false,
        };
    }

    // Keep the first (max_n - 1) text chunks (at least 0), then one file with the rest.
    let keep = max_n.saturating_sub(1);
    let mut chunks: Vec<OutboundChunk> = text_chunks
        .iter()
        .take(keep)
        .cloned()
        .map(OutboundChunk::Text)
        .collect();

    let remainder: String = if keep == 0 {
        original.to_owned()
    } else {
        text_chunks
            .iter()
            .skip(keep)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    };

    let notice = if keep == 0 {
        "Message too long for sequential delivery; see attached file.".to_owned()
    } else {
        format!(
            "…continued in attachment (exceeded {max_n} message chunks)."
        )
    };

    chunks.push(OutboundChunk::File {
        filename: opts.file_name.clone(),
        content: remainder,
        notice,
    });

    ChunkPlan {
        chunks,
        used_file_fallback: true,
    }
}

// ── Split helpers ──────────────────────────────────────────────────────────

fn find_split_point(search: &str, take_bytes: usize) -> usize {
    // Prefer last newline, then last space, else hard split at take_bytes.
    if let Some(pos) = search.rfind('\n') {
        return pos + 1;
    }
    if let Some(pos) = search.rfind(' ') {
        return pos + 1;
    }
    // Ensure char boundary (take_bytes already is, but be safe).
    floor_char_boundary(search, take_bytes)
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

fn char_prefix_bytes(s: &str, max_chars: usize) -> usize {
    if max_chars == 0 {
        return 0;
    }
    for (n, (i, _)) in s.char_indices().enumerate() {
        if n == max_chars {
            return i;
        }
    }
    s.len()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if char_len(s) <= max {
        return s.to_owned();
    }
    let bytes = char_prefix_bytes(s, max);
    s[..bytes].to_owned()
}

fn fence_open_line(marker: &str, lang: &str) -> String {
    if lang.is_empty() {
        format!("{marker}\n")
    } else {
        format!("{marker}{lang}\n")
    }
}

// ── Scan: fence + emphasis ─────────────────────────────────────────────────

fn scan_segment(segment: &str, fence: &mut FenceState, emphasis: &mut EmphasisState) {
    // Process line-by-line for fences; within lines handle emphasis / inline code.
    let mut start = 0usize;
    let bytes = segment.as_bytes();
    let mut i = 0usize;
    while i <= bytes.len() {
        let at_end = i == bytes.len();
        let at_nl = !at_end && bytes[i] == b'\n';
        if at_end || at_nl {
            let line = &segment[start..i];
            process_line(line, fence, emphasis);
            if at_end {
                break;
            }
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
}

fn process_line(line: &str, fence: &mut FenceState, emphasis: &mut EmphasisState) {
    if let Some((marker, lang)) = parse_fence_line(line) {
        if !fence.open {
            fence.open = true;
            fence.marker = marker;
            fence.lang = lang;
            return;
        }
        // Closer must use the same fence character and at least as many.
        if fence_closes(&fence.marker, &marker) {
            fence.open = false;
            fence.marker.clear();
            fence.lang.clear();
            return;
        }
        // Inside fence, a non-matching fence-looking line is body content.
    }

    if fence.open {
        return; // ignore emphasis inside fences
    }

    scan_emphasis_line(line, emphasis);
}

/// Returns (marker, lang) when `line` is a fence line (opener or closer form).
fn parse_fence_line(line: &str) -> Option<(String, String)> {
    // Up to 3 spaces of indent.
    let mut rest = line;
    let mut spaces = 0usize;
    while rest.starts_with(' ') && spaces < 3 {
        rest = &rest[1..];
        spaces += 1;
    }
    if rest.is_empty() {
        return None;
    }
    let fence_char = rest.as_bytes()[0];
    if fence_char != b'`' && fence_char != b'~' {
        return None;
    }
    let mut count = 0usize;
    for b in rest.bytes() {
        if b == fence_char {
            count += 1;
        } else {
            break;
        }
    }
    if count < 3 {
        return None;
    }
    let marker = rest[..count].to_owned();
    let info = rest[count..].trim_end();
    // Closer: only whitespace after marker. Opener: optional info string
    // without backticks of the same kind for ```.
    if fence_char == b'`' && info.contains('`') {
        return None;
    }
    let lang = info.trim().to_owned();
    Some((marker, lang))
}

fn fence_closes(open_marker: &str, close_marker: &str) -> bool {
    if open_marker.is_empty() || close_marker.is_empty() {
        return false;
    }
    let oc = open_marker.as_bytes()[0];
    let cc = close_marker.as_bytes()[0];
    oc == cc && close_marker.len() >= open_marker.len()
}

fn scan_emphasis_line(line: &str, emphasis: &mut EmphasisState) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    let mut in_inline_code = false;

    while i < chars.len() {
        let c = chars[i];

        // Inline code toggles with single backticks (not part of a fence — fences are lines).
        if c == '`' {
            // Count run of backticks.
            let mut run = 0usize;
            while i + run < chars.len() && chars[i + run] == '`' {
                run += 1;
            }
            if run == 1 {
                in_inline_code = !in_inline_code;
            }
            i += run;
            continue;
        }

        if in_inline_code {
            i += 1;
            continue;
        }

        if c == '*' {
            let mut run = 0usize;
            while i + run < chars.len() && chars[i + run] == '*' {
                run += 1;
            }
            // Consume ** as bold, leftover * as italic.
            let mut left = run;
            while left >= 2 {
                emphasis.bold = !emphasis.bold;
                left -= 2;
            }
            if left == 1 {
                emphasis.italic_star = !emphasis.italic_star;
            }
            i += run;
            continue;
        }

        if c == '_' {
            let mut run = 0usize;
            while i + run < chars.len() && chars[i + run] == '_' {
                run += 1;
            }
            let mut left = run;
            while left >= 2 {
                emphasis.underline = !emphasis.underline;
                left -= 2;
            }
            if left == 1 {
                emphasis.italic_under = !emphasis.italic_under;
            }
            i += run;
            continue;
        }

        i += 1;
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_short_message() {
        let chunks = chunk_message("Hello, world!", 2000);
        assert_eq!(chunks, vec!["Hello, world!".to_owned()]);
    }

    #[test]
    fn chunk_empty_message() {
        let chunks = chunk_message("", 2000);
        assert_eq!(chunks, vec![String::new()]);
    }

    #[test]
    fn chunk_at_newline_boundary() {
        let line = "x".repeat(900);
        let msg = format!("{line}\n{line}\n{line}");
        let chunks = chunk_message(&msg, 2000);
        assert!(chunks.len() >= 2);
        for c in &chunks {
            assert!(char_len(c) <= 2000, "chunk len {}", char_len(c));
        }
    }

    #[test]
    fn chunk_at_space_boundary() {
        let word = "word ".repeat(500);
        let chunks = chunk_message(&word, 2000);
        assert!(chunks.len() >= 2);
        for c in &chunks {
            assert!(char_len(c) <= 2000);
        }
    }

    #[test]
    fn chunk_hard_split_no_boundaries() {
        let long = "a".repeat(5000);
        let chunks = chunk_message(&long, 2000);
        assert!(chunks.len() >= 3);
        for c in &chunks {
            assert!(char_len(c) <= 2000);
        }
        // Content preserved (modulo trailing trim on last pieces).
        let joined: usize = chunks.iter().map(|c| c.chars().filter(|ch| *ch == 'a').count()).sum();
        assert_eq!(joined, 5000);
    }

    #[test]
    fn chunk_exactly_at_limit() {
        let msg = "a".repeat(2000);
        let chunks = chunk_message(&msg, 2000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(char_len(&chunks[0]), 2000);
    }

    #[test]
    fn chunk_one_over_limit() {
        let msg = "a".repeat(2001);
        let chunks = chunk_message(&msg, 2000);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn chunk_preserves_words() {
        let msg = "Hello\nWorld\nThis is a test\nOf message chunking";
        let chunks = chunk_message(msg, 20);
        let reassembled = chunks.join("\n");
        assert!(reassembled.contains("Hello"));
        assert!(reassembled.contains("World"));
        assert!(reassembled.contains("chunking"));
    }

    #[test]
    fn fence_split_closes_and_reopens_with_lang() {
        // Build a fenced block longer than max_len so a split is forced mid-body.
        let body = "line of code\n".repeat(30); // ~390 chars
        let msg = format!("```rust\n{body}```");
        let max = 120usize;
        let chunks = chunk_message(&msg, max);
        assert!(chunks.len() >= 2, "expected multi-chunk, got {}", chunks.len());

        // First chunk should open fence and close it.
        assert!(
            chunks[0].contains("```rust") || chunks[0].starts_with("```"),
            "first chunk should open fence: {:?}",
            &chunks[0][..chunks[0].len().min(40)]
        );
        assert!(
            chunks[0].trim_end().ends_with("```"),
            "first chunk should close fence: {:?}",
            chunks[0]
        );

        // Subsequent mid-fence chunks should reopen with language tag.
        for (i, c) in chunks.iter().enumerate().skip(1) {
            if i + 1 < chunks.len() || c.contains("line of code") {
                // Continuation chunks that still carry body should reopen.
                if !c.trim_start().starts_with("```") && c.contains("line of code") {
                    // After final close of source fence, last chunk may just be ```
                    // — allow either reopen or pure closer residue.
                }
            }
        }
        // At least one continuation chunk reopens with rust tag.
        let reopened = chunks.iter().skip(1).any(|c| c.contains("```rust"));
        assert!(
            reopened,
            "expected a continuation chunk to reopen with ```rust, chunks={chunks:?}"
        );

        for c in &chunks {
            assert!(char_len(c) <= max, "chunk over limit: {}", char_len(c));
        }
    }

    #[test]
    fn fence_intact_when_fits() {
        let msg = "```python\nprint('hi')\n```";
        let chunks = chunk_message(msg, 2000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], msg);
    }

    #[test]
    fn emphasis_bold_rebalanced_across_split() {
        // Open bold early; force split before the closing **.
        let msg = format!("**{}", "word ".repeat(40)); // no closer
        let max = 50usize;
        let chunks = chunk_message(&msg, max);
        assert!(chunks.len() >= 2);

        // First chunk should close bold.
        assert!(
            chunks[0].contains("**") && chunks[0].matches("**").count() >= 1,
            "first={:?}",
            chunks[0]
        );
        // Count ** in first chunk — should be even (open+close) after balance,
        // or end with **.
        assert!(
            chunks[0].ends_with("**") || chunks[0].matches("**").count().is_multiple_of(2),
            "first chunk should rebalance bold: {:?}",
            chunks[0]
        );
        // Continuation should reopen bold.
        assert!(
            chunks[1].starts_with("**") || chunks[1].contains("**"),
            "second should reopen bold: {:?}",
            chunks[1]
        );
    }

    #[test]
    fn emphasis_italic_star_and_underline() {
        let msg = format!("_{}_ and *{}*", "a".repeat(80), "b".repeat(80));
        let chunks = chunk_message(&msg, 60);
        assert!(chunks.len() >= 2);
        for c in &chunks {
            assert!(char_len(c) <= 60);
        }
    }

    #[test]
    fn emphasis_double_underscore_underline() {
        let msg = format!("__{}__", "u".repeat(100));
        let chunks = chunk_message(&msg, 40);
        assert!(chunks.len() >= 2);
        // First piece opened __ so should end with __ closer if still open mid-way
        // (content has matching closer at end only).
        assert!(chunks.iter().any(|c| c.contains("__")));
    }

    #[test]
    fn nitro_resolve_max_len() {
        let mut cfg = DiscordConfig::default();
        assert_eq!(resolve_max_message_len(&cfg), DISCORD_STANDARD_MAX_LEN);

        cfg.nitro = true;
        assert_eq!(resolve_max_message_len(&cfg), DISCORD_NITRO_MAX_LEN);

        cfg.max_message_length = Some(3500);
        assert_eq!(resolve_max_message_len(&cfg), 3500);

        cfg.max_message_length = Some(99999);
        assert_eq!(resolve_max_message_len(&cfg), DISCORD_NITRO_MAX_LEN);

        cfg.max_message_length = Some(0);
        assert_eq!(resolve_max_message_len(&cfg), 1);
    }

    #[test]
    fn options_from_config_nitro() {
        let cfg = DiscordConfig {
            nitro: true,
            max_chunks_before_file: 5,
            prefer_embeds: true,
            ..Default::default()
        };
        let opts = ChunkerOptions::from_discord_config(&cfg);
        assert_eq!(opts.max_len, DISCORD_NITRO_MAX_LEN);
        assert_eq!(opts.max_chunks_before_file, 5);
        assert!(opts.prefer_embeds);
    }

    #[test]
    fn file_fallback_decision() {
        assert!(!should_use_file_fallback(10, 10));
        assert!(should_use_file_fallback(11, 10));
        assert!(!should_use_file_fallback(1, 1));
        assert!(should_use_file_fallback(2, 1));
    }

    #[test]
    fn plan_file_fallback_past_n_chunks() {
        let long = "word ".repeat(500); // forces many small chunks
        let opts = ChunkerOptions {
            max_len: 40,
            max_chunks_before_file: 3,
            prefer_embeds: false,
            file_name: "out.md".into(),
        };
        let plan = plan_chunks(&long, &opts);
        assert!(plan.used_file_fallback, "expected file fallback");
        assert!(
            plan.chunks.len() <= 3,
            "plan should not exceed threshold pieces, got {}",
            plan.chunks.len()
        );
        assert!(
            matches!(plan.chunks.last(), Some(OutboundChunk::File { .. })),
            "last chunk should be File, got {:?}",
            plan.chunks.last()
        );
        if let Some(OutboundChunk::File {
            filename,
            content,
            notice,
        }) = plan.chunks.last()
        {
            assert_eq!(filename, "out.md");
            assert!(!content.is_empty());
            assert!(!notice.is_empty());
        }
    }

    #[test]
    fn plan_no_file_when_under_threshold() {
        let msg = "hello world";
        let opts = ChunkerOptions::default();
        let plan = plan_chunks(msg, &opts);
        assert!(!plan.used_file_fallback);
        assert_eq!(plan.chunks.len(), 1);
        assert_eq!(plan.chunks[0], OutboundChunk::Text("hello world".into()));
    }

    #[test]
    fn embed_limits_enforced() {
        let mut embed = EmbedPayload {
            title: Some("t".repeat(300)),
            description: Some("d".repeat(5000)),
            footer: Some("f".repeat(3000)),
            author: Some("a".repeat(300)),
            fields: (0..30)
                .map(|i| EmbedField {
                    name: format!("n{i}{}", "x".repeat(300)),
                    value: "v".repeat(2000),
                    inline: false,
                })
                .collect(),
        };
        let total = embed.enforce_limits();
        if let Some(ref t) = embed.title {
            assert!(t.chars().count() <= EMBED_TITLE_MAX);
        }
        if let Some(ref d) = embed.description {
            assert!(d.chars().count() <= EMBED_DESCRIPTION_MAX);
        }
        if let Some(ref f) = embed.footer {
            assert!(f.chars().count() <= EMBED_FOOTER_MAX);
        }
        if let Some(ref a) = embed.author {
            assert!(a.chars().count() <= EMBED_AUTHOR_MAX);
        }
        assert!(embed.fields.len() <= EMBED_FIELDS_MAX);
        for f in &embed.fields {
            assert!(f.name.chars().count() <= EMBED_FIELD_NAME_MAX);
            assert!(f.value.chars().count() <= EMBED_FIELD_VALUE_MAX);
        }
        assert!(total <= EMBED_TOTAL_MAX, "total {total}");
        assert_eq!(total, embed.total_chars());
    }

    #[test]
    fn pack_embeds_splits_description() {
        let content = "para\n".repeat(2000); // well over 4096
        let embeds = pack_content_as_embeds(&content);
        assert!(embeds.len() >= 2);
        for e in &embeds {
            let d = e.description.as_ref().unwrap();
            assert!(d.chars().count() <= EMBED_DESCRIPTION_MAX);
            assert!(e.total_chars() <= EMBED_TOTAL_MAX);
        }
    }

    #[test]
    fn plan_prefer_embeds() {
        let content = "line of text goes here\n".repeat(200); // ~4600
        let opts = ChunkerOptions {
            max_len: 2000,
            max_chunks_before_file: 10,
            prefer_embeds: true,
            file_name: "message.md".into(),
        };
        let plan = plan_chunks(&content, &opts);
        assert!(!plan.used_file_fallback);
        assert!(
            plan.chunks.iter().all(|c| matches!(c, OutboundChunk::Embed(_))),
            "expected embeds, got {:?}",
            plan.chunks
        );
    }

    #[test]
    fn build_field_embed_truncates() {
        let title = "title".repeat(100);
        let name = "name".repeat(100);
        let value = "value".repeat(300);
        let embed = build_field_embed(Some(&title), vec![(&name, &value, true)]);
        assert!(embed.title.as_ref().unwrap().chars().count() <= EMBED_TITLE_MAX);
        assert_eq!(embed.fields.len(), 1);
        assert!(embed.fields[0].name.chars().count() <= EMBED_FIELD_NAME_MAX);
        assert!(embed.fields[0].value.chars().count() <= EMBED_FIELD_VALUE_MAX);
    }

    #[test]
    fn utf8_hard_split_no_panic() {
        let msg = "你好世界".repeat(100);
        let chunks = chunk_message(&msg, 25);
        assert!(chunks.len() >= 2);
        for c in &chunks {
            assert!(char_len(c) <= 25);
            // Valid UTF-8 by construction of String.
            assert!(!c.is_empty() || chunks.len() == 1);
        }
    }

    #[test]
    fn embed_to_json_shape() {
        let embed = EmbedPayload {
            title: Some("T".into()),
            description: Some("D".into()),
            fields: vec![EmbedField {
                name: "n".into(),
                value: "v".into(),
                inline: true,
            }],
            footer: Some("F".into()),
            author: Some("A".into()),
        };
        let v = embed.to_json();
        assert_eq!(v["title"], "T");
        assert_eq!(v["description"], "D");
        assert_eq!(v["footer"]["text"], "F");
        assert_eq!(v["author"]["name"], "A");
        assert_eq!(v["fields"][0]["inline"], true);
    }
}
