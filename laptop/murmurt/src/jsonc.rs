//! Position-aware JSONC editing. Line-based heuristics break on
//! real-world configs (single-line arrays, compact braces, empty
//! objects), so instead we scan the text once — tracking strings and
//! comments — and insert at exact byte positions with correct comma
//! placement. Every merge validates the result before writing; a
//! config we can't parse is a config we don't touch.

/// Scanner state over jsonc text: positions of structural characters,
/// skipping `"strings"`, `// line` and `/* block */` comments.
pub struct Scan<'a> {
    pub text: &'a str,
    /// byte offsets of every structural char outside strings/comments
    pub structural: Vec<(usize, u8)>,
}

pub fn scan(text: &str) -> Scan<'_> {
    let bytes = text.as_bytes();
    let mut structural = Vec::new();
    let mut i = 0;
    let mut in_str = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if c == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if in_str {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                in_line_comment = true;
                i += 1;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                in_block_comment = true;
                i += 1;
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' => structural.push((i, c)),
            _ => {}
        }
        i += 1;
    }
    Scan { text, structural }
}

impl<'a> Scan<'a> {
    /// Byte position right after the root object's opening brace.
    pub fn root_open(&self) -> Option<usize> {
        self.structural
            .first()
            .filter(|(_, c)| *c == b'{')
            .map(|(pos, _)| pos + 1)
    }

    /// Does the root object have any keys? i.e. is the next structural
    /// char after the opening brace something other than `}`.
    pub fn root_nonempty(&self) -> bool {
        match self.structural.first() {
            Some((_, b'{')) => self.structural.get(1).map(|(_, c)| *c != b'}').unwrap_or(false),
            _ => false,
        }
    }

    /// Byte position right after the `[` of the named top-level array key.
    pub fn array_open(&self, key: &str) -> Option<(usize, bool)> {
        // find `"key"` in the raw text (outside comments — good enough:
        // keys inside strings would need the scan, but a commented-out
        // key is the realistic hazard and this skips those regions)
        let keypos = find_key_outside_comments(self.text, key)?;
        // the next structural chars after the key should be ':' then '['
        let after = &self.text[keypos..];
        let colon = after.find(':')?;
        let bracket_rel = after[colon..].find('[')?;
        let bracket_abs = keypos + colon + bracket_rel;
        // non-empty? scan forward from bracket skipping ws/comments
        let nonempty = next_meaningful_byte(self.text, bracket_abs + 1).map(|(_, c)| c != b']').unwrap_or(false);
        Some((bracket_abs + 1, nonempty))
    }
}

/// Find `"key"` as a top-level-ish key: a quoted string in the text that
/// is not inside a comment. We don't verify object depth — waybar
/// configs are flat enough that the first hit is the real one, and the
/// caller validates the final result anyway.
fn find_key_outside_comments(text: &str, key: &str) -> Option<usize> {
    let needle = format!("\"{key}\"");
    let bytes = text.as_bytes();
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_line_comment {
            if c == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if c == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            in_line_comment = true;
            i += 2;
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            in_block_comment = true;
            i += 2;
            continue;
        }
        if text[i..].starts_with(&needle) {
            return Some(i + needle.len());
        }
        i += 1;
    }
    None
}

/// Next non-whitespace, non-comment byte after `pos`.
fn next_meaningful_byte(text: &str, pos: usize) -> Option<(usize, u8)> {
    let bytes = text.as_bytes();
    let mut i = pos;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            c => return Some((i, c)),
        }
    }
    None
}

/// Validate: strip comments and trailing commas, then parse as JSON.
/// We accept trailing commas in input (jsonc dialects often do).
pub fn valid_jsonc(text: &str) -> bool {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut in_str = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            out.push(c as char);
            if c == b'\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1] as char);
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            in_str = true;
            out.push('"');
            i += 1;
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        out.push(c as char);
        i += 1;
    }
    // drop trailing commas: `,` followed only by ws before } or ]
    let stripped = out;
    let mut finalout = String::with_capacity(stripped.len());
    let sb = stripped.as_bytes();
    let mut j = 0;
    while j < sb.len() {
        if sb[j] == b',' {
            let mut k = j + 1;
            while k < sb.len() && (sb[k] as char).is_whitespace() {
                k += 1;
            }
            if k < sb.len() && (sb[k] == b'}' || sb[k] == b']') {
                j += 1; // skip the comma
                continue;
            }
        }
        finalout.push(sb[j] as char);
        j += 1;
    }
    serde_json::from_str::<serde_json::Value>(&finalout).is_ok()
}

/// Insert `fragment` at byte position `pos`.
pub fn insert_at(text: &str, pos: usize, fragment: &str) -> String {
    let mut out = String::with_capacity(text.len() + fragment.len());
    out.push_str(&text[..pos]);
    out.push_str(fragment);
    out.push_str(&text[pos..]);
    out
}
