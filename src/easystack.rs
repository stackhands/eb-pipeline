//! Build list lines from an EasyBuild easystack: the file `eb --easystack`
//! builds from, which `eb-stack rebuild-set`, `eb-stack stack order
//! --easystack-out` and `eb-stack stack easystack` write.
//!
//! Each entry becomes the line of the build EasyBuild 5 makes of it: a name
//! without `.eb` gets it, and the entry's `options` become `eb` options as
//! EasyBuild's `opts_dict_to_eb_opts` writes them, in the order of their
//! names. Only `easyconfigs` is read; EasyBuild ignores the other top-level
//! keys as well.

use crate::util::{list_word, ListLine};
use serde_yaml::{Mapping, Value};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

struct Diagnostic {
    message: String,
    location: Option<(usize, usize)>,
}

/// Whether the build list `path` is an easystack: a `.yml` or `.yaml` file.
pub fn is_easystack(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("yml") || extension.eq_ignore_ascii_case("yaml")
        })
}

/// Read and parse an easystack, with source locations in its diagnostics.
pub fn read(path: &Path) -> Result<Vec<ListLine>, String> {
    let mut file = crate::util::open_regular_file(path)
        .map_err(|e| format!("cannot read list {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read list {}: {e}", path.display()))?;
    let text = std::str::from_utf8(&bytes).map_err(|e| {
        let (line, column) = byte_location(&bytes, e.valid_up_to());
        format!(
            "easystack {}:{line}:{column}: input is not valid UTF-8",
            path.display()
        )
    })?;
    parse_diagnostic(text).map_err(|error| match error.location {
        Some((line, column)) => format!(
            "easystack {}:{line}:{column}: {}",
            path.display(),
            error.message
        ),
        None => format!("easystack {}: {}", path.display(), error.message),
    })
}

/// The build list lines of the easystack `text`, in file order.
pub fn parse(text: &str) -> Result<Vec<ListLine>, String> {
    parse_diagnostic(text).map_err(|error| error.message)
}

fn parse_diagnostic(text: &str) -> Result<Vec<ListLine>, Diagnostic> {
    // Quote spellings PyYAML keeps as text before YAML 1.2 turns them into numbers.
    // A diagnostic names a column of this text. The quoted rewrite is only a parse.
    let quoted = quote_scalars_easybuild_keeps_as_text(text);
    let doc = match parsed_yaml(&quoted) {
        Ok(doc) => {
            // A walk that did not reach the end of the stream quotes only the
            // spans it saw. A later whole spelling would still parse as a
            // number. Report the parse error instead of those changed values.
            if plain_number_spellings_remain(&quoted) {
                if let Err(error) = parsed_yaml(text) {
                    return Err(yaml_diagnostic(text, error));
                }
            }
            doc
        }
        Err(quoted_error) => match parsed_yaml(text) {
            Err(error) => {
                return Err(prefer_spelling_from_file(
                    text,
                    &quoted,
                    error,
                    quoted_error,
                ))
            }
            Ok(doc) => {
                // Quoting made two text spellings one key. Report that repeat
                // from the rewrite, at the original byte of its mark.
                // Any other failure of the rewrite leaves the original parse.
                let (message, _) = diagnostic_message(&quoted_error);
                if duplicate_message(&message) {
                    if let Some(dup) = first_duplicate(&quoted) {
                        let message = named_message(&message, &dup);
                        let index = map_rewritten_index(text, &quoted, dup.index);
                        return Err(Diagnostic {
                            message,
                            location: Some(advance_mark(text, 0, 0, 0, index)),
                        });
                    }
                }
                doc
            }
        },
    };
    let entries = match doc.get("easyconfigs") {
        Some(Value::Sequence(entries)) => entries,
        Some(_) => {
            return Err(Diagnostic {
                message: "`easyconfigs` is not a list; start each entry with `- `".into(),
                location: None,
            })
        }
        None => {
            return Err(Diagnostic {
                message: "there is no top-level `easyconfigs` key".into(),
                location: None,
            })
        }
    };
    entries
        .iter()
        .enumerate()
        .map(|(i, item)| {
            entry(item).map_err(|message| Diagnostic {
                message: format!("entry {}: {message}", i + 1),
                location: None,
            })
        })
        .collect()
}

fn parsed_yaml(text: &str) -> Result<Value, serde_yaml::Error> {
    let mut doc: Value = serde_yaml::from_str(text)?;
    doc.apply_merge()?;
    Ok(doc)
}

/// Quote a plain scalar that EasyBuild's loader keeps as text.
///
/// This loader follows YAML 1.2. EasyBuild reads an easystack with PyYAML.
/// PyYAML leaves spellings such as `1e0`, `1.5e1`, `+.5` and `0o20` as text.
/// YAML 1.2 reads them as numbers, and a parsed one becomes a bare option.
/// A spelling is quoted only when it is the whole plain scalar.
///
/// The source span is the libyaml event mark. The mark starts at a leading
/// anchor or tag, and the scalar text is the suffix of that mark equal to the
/// event value. A folded line is not that suffix, so it stays as written.
///
/// libyaml stops at the first scan error, so one walk misses a spelling after
/// an aborted plain key such as `{1e0:}`. Each pass quotes the spans it saw
/// and walks the rewritten text. The passes stop when a walk reaches the end
/// of the stream, when a pass adds no span, or when the error does not advance.
fn quote_scalars_easybuild_keeps_as_text(text: &str) -> String {
    let mut current = text.to_string();
    let mut previous_error: Option<usize> = None;
    let passes = text.len().saturating_add(2).max(2);
    for _ in 0..passes {
        let walk = walk_plain_spellings(&current);
        let Some(error_at) = walk.error_at else {
            return splice_single_quotes(&current, &walk.spans);
        };
        if walk.spans.is_empty() || previous_error.is_some_and(|previous| error_at <= previous) {
            break;
        }
        let rewritten = splice_single_quotes(&current, &walk.spans);
        if rewritten == current {
            break;
        }
        previous_error = Some(index_after_quotes(&walk.spans, error_at));
        current = rewritten;
    }
    current
}

struct SpellingWalk {
    spans: Vec<(usize, usize)>,
    /// `None` when the walk read `STREAM_END`.
    error_at: Option<usize>,
}

fn walk_plain_spellings(text: &str) -> SpellingWalk {
    let Some(mut parser) = YamlParser::open(text) else {
        return SpellingWalk {
            spans: Vec::new(),
            error_at: None,
        };
    };
    let mut spans = Vec::new();
    loop {
        match parser.next_event() {
            Some(YamlEvent::StreamEnd) => {
                return SpellingWalk {
                    spans,
                    error_at: None,
                };
            }
            Some(YamlEvent::Scalar(scalar)) => {
                if let Some(span) = text_spelling_span(text, &scalar) {
                    push_span(&mut spans, span);
                }
            }
            Some(_) => {}
            None => {
                if let Some(span) = parser.aborted_plain_spelling(text) {
                    push_span(&mut spans, span);
                }
                return SpellingWalk {
                    spans,
                    error_at: Some(parser.error_index()),
                };
            }
        }
    }
}

/// Where `index` sits after each quoted span inserts its two quote marks.
fn index_after_quotes(spans: &[(usize, usize)], index: usize) -> usize {
    let mut shift = 0;
    for &(start, end) in spans {
        if start >= index {
            break;
        }
        if end <= index {
            shift += 2;
        } else {
            shift += 1;
            break;
        }
    }
    index + shift
}

fn plain_number_spellings_remain(text: &str) -> bool {
    !walk_plain_spellings(text).spans.is_empty()
}

/// The plain-scalar bytes to quote, when those bytes are the whole scalar.
///
/// `start_mark` includes one anchor and one tag. The event value is the scalar
/// text, so a one-line scalar is the suffix of the mark with that length.
fn text_spelling_span(text: &str, scalar: &YamlScalar) -> Option<(usize, usize)> {
    if !scalar.plain {
        return None;
    }
    let start = scalar.start as usize;
    let end = scalar.end as usize;
    let content = end.checked_sub(scalar.value.len())?;
    if content < start {
        return None;
    }
    let slice = text.get(content..end)?;
    if slice != scalar.value {
        return None;
    }
    text_spelling(slice).then_some((content, end))
}

fn text_spelling(s: &str) -> bool {
    yaml12_number_len(s) == Some(s.len()) && !pyyaml_number(s)
}

fn push_span(spans: &mut Vec<(usize, usize)>, span: (usize, usize)) {
    if spans.last().is_some_and(|(_, end)| span.0 < *end) {
        return;
    }
    spans.push(span);
}

fn splice_single_quotes(text: &str, spans: &[(usize, usize)]) -> String {
    if spans.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + spans.len() * 2);
    let mut cursor = 0;
    for &(start, end) in spans {
        if start < cursor || end > text.len() || start > end {
            return text.to_string();
        }
        out.push_str(&text[cursor..start]);
        out.push('\'');
        out.push_str(&text[start..end]);
        out.push('\'');
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

fn break_len(text: &str, i: usize) -> Option<usize> {
    let rest = &text[i..];
    if rest.starts_with("\r\n") {
        return Some(2);
    }
    let ch = rest.chars().next()?;
    match ch {
        '\n' | '\r' | '\u{0085}' | '\u{2028}' | '\u{2029}' => Some(ch.len_utf8()),
        _ => None,
    }
}

fn yaml12_number_len(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    if let Some(len) = prefixed_int_len(bytes, i) {
        return Some(len);
    }
    let start = i;
    if bytes[i] == b'.' {
        i += 1;
        let frac = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == frac {
            return None;
        }
    } else if bytes[i].is_ascii_digit() {
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
    } else {
        return None;
    }
    i = take_exponent(bytes, i);
    (i > start).then_some(i)
}

fn prefixed_int_len(bytes: &[u8], i: usize) -> Option<usize> {
    let rest = bytes.get(i..)?;
    let hex = rest.starts_with(b"0x") || rest.starts_with(b"0X");
    let oct = rest.starts_with(b"0o") || rest.starts_with(b"0O");
    let bin = rest.starts_with(b"0b") || rest.starts_with(b"0B");
    if !hex && !oct && !bin {
        return None;
    }
    let mut j = i + 2;
    let digits = j;
    while j < bytes.len()
        && if hex {
            bytes[j].is_ascii_hexdigit()
        } else if oct {
            matches!(bytes[j], b'0'..=b'7')
        } else {
            matches!(bytes[j], b'0' | b'1')
        }
    {
        j += 1;
    }
    (j > digits).then_some(j)
}

fn take_exponent(bytes: &[u8], i: usize) -> usize {
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        let mark = i;
        let mut j = i + 1;
        if j < bytes.len() && (bytes[j] == b'+' || bytes[j] == b'-') {
            j += 1;
        }
        let digits = j;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > digits {
            return j;
        }
        return mark;
    }
    i
}

fn pyyaml_number(s: &str) -> bool {
    pyyaml_int(s) || pyyaml_float(s)
}

fn pyyaml_int(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let Some(rest) = bytes.get(i..) else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    if rest.len() > 2 && rest[0] == b'0' && rest[1] == b'b' {
        return rest[2..].iter().all(|c| matches!(c, b'0' | b'1' | b'_'));
    }
    if rest.len() > 2 && rest[0] == b'0' && rest[1] == b'x' {
        return rest[2..]
            .iter()
            .all(|c| c.is_ascii_hexdigit() || *c == b'_');
    }
    if rest.len() >= 2
        && rest[0] == b'0'
        && rest[1..].iter().all(|c| matches!(c, b'0'..=b'7' | b'_'))
    {
        return true;
    }
    if rest == b"0" {
        return true;
    }
    rest[0].is_ascii_digit()
        && rest[0] != b'0'
        && rest[1..].iter().all(|c| c.is_ascii_digit() || *c == b'_')
}

fn pyyaml_float(s: &str) -> bool {
    let bytes = s.as_bytes();
    let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") || matches!(s, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    let mut i = s.len() - unsigned.len();
    if i >= bytes.len() {
        return false;
    }
    if bytes[0] == b'.' {
        return dot_lead_float(bytes);
    }
    if bytes[i] == b'.' {
        // A sign and then a dot, as in `+.5`, is text to PyYAML.
        return false;
    }
    if !bytes[i].is_ascii_digit() {
        return false;
    }
    i += 1;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'.' {
        return false;
    }
    i += 1;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    if i == bytes.len() {
        return true;
    }
    signed_exponent(&bytes[i..])
}

fn dot_lead_float(bytes: &[u8]) -> bool {
    if bytes.first() != Some(&b'.') || bytes.get(1).is_none_or(|c| !c.is_ascii_digit()) {
        return false;
    }
    let mut i = 2;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    i == bytes.len() || signed_exponent(&bytes[i..])
}

fn signed_exponent(bytes: &[u8]) -> bool {
    if bytes.len() < 3 || (bytes[0] != b'e' && bytes[0] != b'E') {
        return false;
    }
    if bytes[1] != b'+' && bytes[1] != b'-' {
        return false;
    }
    bytes[2..].iter().all(|c| c.is_ascii_digit())
}

fn diagnostic_message(error: &serde_yaml::Error) -> (String, Option<(usize, usize)>) {
    let parser_location = error
        .location()
        .map(|location| (location.line(), location.column()));
    let mut message = error.to_string();
    if let Some((line, column)) = parser_location {
        message = message
            .strip_suffix(&format!(" at line {line} column {column}"))
            .unwrap_or(&message)
            .to_string();
    }
    (message, parser_location)
}

fn yaml_diagnostic(text: &str, error: serde_yaml::Error) -> Diagnostic {
    let (mut message, mut location) = diagnostic_message(&error);
    if duplicate_message(&message) {
        if let Some(dup) = first_duplicate(text) {
            message = named_message(&message, &dup);
            location = Some(advance_mark(text, 0, 0, 0, dup.index));
        }
    }
    Diagnostic { message, location }
}

/// Both parses failed. A number spelling such as `1e0` is key 1.0 in the
/// original message and key "1e0" in the quoted one. Keep the file's spelling
/// when that repeat is earlier. A flow repeat and a block repeat of one key
/// keep the earlier column.
///
/// `{1e0:}` is a scan error here. The quoted rewrite reads past it. When that
/// rewrite fails further along, its error is the one to report. Every mark in
/// that message is a column of the original text.
///
/// When the two duplicates sit on the same column, the quoted spelling is the
/// one to report.
fn prefer_spelling_from_file(
    text: &str,
    quoted: &str,
    error: serde_yaml::Error,
    quoted_error: serde_yaml::Error,
) -> Diagnostic {
    let original = yaml_diagnostic(text, error);
    let rewritten = yaml_diagnostic(quoted, quoted_error);
    if let (Some(original_at), Some(rewritten_at)) = (original.location, rewritten.location) {
        let original_index = location_index(text, original_at.0, original_at.1);
        let rewritten_index = map_rewritten_index(
            text,
            quoted,
            location_index(quoted, rewritten_at.0, rewritten_at.1),
        );
        if rewritten_index > original_index {
            let at = advance_mark(text, 0, 0, 0, rewritten_index);
            return Diagnostic {
                message: message_on_original(text, quoted, &rewritten.message),
                location: Some(at),
            };
        }
    }
    if duplicate_message(&original.message) && duplicate_message(&rewritten.message) {
        if let Some(rewritten_at) = rewritten.location {
            let index = map_rewritten_index(
                text,
                quoted,
                location_index(quoted, rewritten_at.0, rewritten_at.1),
            );
            let quoted_at = advance_mark(text, 0, 0, 0, index);
            if original.location.is_none_or(|at| quoted_at <= at) {
                return Diagnostic {
                    message: message_on_original(text, quoted, &rewritten.message),
                    location: Some(quoted_at),
                };
            }
        }
    }
    original
}

/// The quoted file's problem and context, written back onto the original text.
///
/// libyaml's message names the problem mark and, when it differs, the context
/// mark. Both indexes go through `map_rewritten_index`, even when the problem
/// column did not move. A duplicate is serde's error: libyaml walks past it
/// and may stop on a later syntax error. That text stays, and the caller maps
/// its mark. A real libyaml error is rebuilt so the problem and the context
/// name that same event.
fn message_on_original(original: &str, quoted: &str, fallback: &str) -> String {
    if duplicate_message(fallback) {
        return fallback.to_string();
    }
    let Some(err) = libyaml_failure(quoted) else {
        return fallback.to_string();
    };
    render_libyaml_message(original, quoted, &err)
}

struct LibyamlFailure {
    problem: String,
    problem_offset: usize,
    problem_index: usize,
    problem_line: usize,
    problem_column: usize,
    context: Option<String>,
    context_index: usize,
    context_line: usize,
    context_column: usize,
}

fn libyaml_failure(text: &str) -> Option<LibyamlFailure> {
    let mut parser = YamlParser::open(text)?;
    loop {
        match parser.next_event() {
            Some(YamlEvent::StreamEnd) => return None,
            Some(_) => {}
            None => return parser.failure(),
        }
    }
}

fn render_libyaml_message(original: &str, quoted: &str, err: &LibyamlFailure) -> String {
    let problem_printed = err.problem_line != 0 || err.problem_column != 0;
    let problem_at = mapped_mark(original, quoted, err.problem_index);
    let mut message = err.problem.clone();
    if problem_printed {
        message.push_str(&format!(
            " at line {} column {}",
            problem_at.0, problem_at.1
        ));
    } else if err.problem_offset != 0 {
        let at = map_rewritten_index(original, quoted, err.problem_offset);
        message.push_str(&format!(" at position {at}"));
    }
    let context_printed = err.context.is_some()
        && (err.context_line != 0 || err.context_column != 0)
        && (err.context_line != err.problem_line || err.context_column != err.problem_column);
    if let Some(context) = &err.context {
        message.push_str(", ");
        message.push_str(context);
        if context_printed {
            let context_at = mapped_mark(original, quoted, err.context_index);
            message.push_str(&format!(
                " at line {} column {}",
                context_at.0, context_at.1
            ));
        }
    }
    if problem_printed && !context_printed {
        let suffix = format!(" at line {} column {}", problem_at.0, problem_at.1);
        if let Some(stripped) = message.strip_suffix(&suffix) {
            return stripped.to_string();
        }
    }
    message
}

fn mapped_mark(original: &str, quoted: &str, index: usize) -> (usize, usize) {
    advance_mark(
        original,
        0,
        0,
        0,
        map_rewritten_index(original, quoted, index),
    )
}

/// Byte in `original` for `rewritten_index` in the quoted rewrite.
///
/// Each pass inserts `'` around a spelling and leaves every other byte in order.
fn map_rewritten_index(original: &str, rewritten: &str, rewritten_index: usize) -> usize {
    let orig = original.as_bytes();
    let rew = rewritten.as_bytes();
    let target = rewritten_index.min(rew.len());
    let mut original_at = 0;
    let mut rewritten_at = 0;
    while rewritten_at < target && original_at < orig.len() {
        if orig[original_at] == rew[rewritten_at] {
            original_at += 1;
            rewritten_at += 1;
        } else if rew[rewritten_at] == b'\'' {
            rewritten_at += 1;
        } else {
            break;
        }
    }
    original_at
}

fn duplicate_message(message: &str) -> bool {
    let Some(tail) = message.split_once("duplicate entry ").map(|(_, tail)| tail) else {
        return false;
    };
    tail.starts_with("with key ")
        || tail.starts_with("with null key")
        || tail.starts_with("in YAML map")
}

fn duplicate_entry_key(message: &str) -> Option<String> {
    // The key is a JSON string. `rsplit(": ")` drops a key such as "a: b".
    let encoded = message.split_once("duplicate entry with key ")?.1;
    serde_json::from_str(encoded).ok()
}

fn message_naming_key(message: &str, key: &str) -> String {
    let encoded = serde_json::to_string(key).expect("a key is JSON text");
    match message.split_once("duplicate entry ") {
        Some((head, _)) => format!("{head}duplicate entry with key {encoded}"),
        None => format!("duplicate entry with key {encoded}"),
    }
}

/// The first key serde would reject, from the event walk.
///
/// Each mapping has its own keys. A scalar is the value serde reads from its
/// span. A collection is built from the children the walk has already
/// resolved, so a child alias of an outer anchor is part of that value. The
/// mark of a scalar is its content; the mark of a collection is its start.
/// An explicit key with no scalar token points at the `?`. When a value
/// cannot be built, the walk stops and serde's own duplicate message stays.
struct Dup {
    index: usize,
    spelling: Option<String>,
    value: Value,
}

fn named_message(message: &str, dup: &Dup) -> String {
    let Some(spelling) = dup.spelling.as_deref() else {
        return message.to_string();
    };
    // A collection alias stays "duplicate entry in YAML map".
    // A number alias keeps serde's `key 1.0` on the original parse. The quoted
    // rewrite reads that anchor as text, so the alias is a string and is named
    // `*name`. Those two marks share a column, and the tie keeps `*name`.
    // Every other alias is named `*name` on both parses.
    if let Some(alias) = spelling.strip_prefix('*') {
        if !alias.is_empty() {
            return match &dup.value {
                Value::Sequence(_) | Value::Mapping(_) | Value::Number(_) => message.to_string(),
                _ => message_naming_key(message, spelling),
            };
        }
    }
    if duplicate_entry_key(message).as_deref() == Some(spelling) {
        return message.to_string();
    }
    let rename = match &dup.value {
        Value::Number(number) => format!("{number}") != spelling,
        Value::Tagged(_) => true,
        _ => false,
    };
    if rename {
        message_naming_key(message, spelling)
    } else {
        message.to_string()
    }
}

enum Walk {
    Go,
    Dup(Dup),
    Stop,
}

fn first_duplicate(text: &str) -> Option<Dup> {
    let mut parser = YamlParser::open(text)?;
    let mut tokens = Tokens::scan(text);
    let mut frames = Vec::new();
    let mut anchors = HashMap::new();
    loop {
        let event = parser.next_event()?;
        match event {
            YamlEvent::StreamEnd => return None,
            other => {
                match on_duplicate_event(text, &mut frames, &mut anchors, &mut tokens, other) {
                    Walk::Go => {}
                    Walk::Dup(dup) => return Some(dup),
                    Walk::Stop => return None,
                }
            }
        }
    }
}

fn on_duplicate_event(
    text: &str,
    frames: &mut Vec<NodeFrame>,
    anchors: &mut HashMap<String, Value>,
    tokens: &mut Tokens,
    event: YamlEvent,
) -> Walk {
    match event {
        YamlEvent::Scalar(scalar) => scalar_key(text, frames, anchors, tokens, scalar),
        YamlEvent::Alias { name, start, end } => {
            alias_key(frames, anchors, tokens, name, start, end)
        }
        YamlEvent::MapStart { start, end, tag } => {
            let bits = tokens.take(end as usize);
            let is_key = take_key_slot(frames);
            frames.push(NodeFrame::Map(MapState {
                expect_key: true,
                keys: Vec::new(),
                pending: None,
                entries: Vec::new(),
                is_key,
                point: bits.collection_at.unwrap_or(start as usize),
                anchor: bits.anchor,
                tag: serde_collection_tag(tag),
            }));
            Walk::Go
        }
        YamlEvent::SeqStart { start, end, tag } => {
            let bits = tokens.take(end as usize);
            let is_key = take_key_slot(frames);
            frames.push(NodeFrame::Seq(SeqState {
                items: Vec::new(),
                is_key,
                point: bits.collection_at.unwrap_or(start as usize),
                anchor: bits.anchor,
                tag: serde_collection_tag(tag),
            }));
            Walk::Go
        }
        YamlEvent::MapEnd => finish_collection(frames, anchors, true),
        YamlEvent::SeqEnd => finish_collection(frames, anchors, false),
        YamlEvent::Other | YamlEvent::StreamEnd => Walk::Go,
    }
}

fn scalar_key(
    text: &str,
    frames: &mut [NodeFrame],
    anchors: &mut HashMap<String, Value>,
    tokens: &mut Tokens,
    scalar: YamlScalar,
) -> Walk {
    let is_key = take_key_slot(frames);
    let bits = tokens.take(scalar.end as usize);
    let Some(value) = resolve_span(text, scalar.start, scalar.end) else {
        return Walk::Stop;
    };
    if let Some(name) = bits.anchor.clone() {
        anchors.insert(name, value.clone());
    }
    let spelling = scalar_spelling(&scalar, &value, &bits);
    let index = scalar_index(text, &scalar, &value, &bits);
    place_node(frames, value, index, Some(spelling), is_key)
}

fn alias_key(
    frames: &mut [NodeFrame],
    anchors: &mut HashMap<String, Value>,
    tokens: &mut Tokens,
    name: String,
    start: u64,
    end: u64,
) -> Walk {
    let is_key = take_key_slot(frames);
    let bits = tokens.take(end as usize);
    let Some(value) = anchors.get(&name).cloned() else {
        return Walk::Stop;
    };
    if let Some(anchor) = bits.anchor.clone() {
        anchors.insert(anchor, value.clone());
    }
    let index = bits
        .alias
        .as_ref()
        .map(|tok| tok.start)
        .unwrap_or(start as usize);
    place_node(frames, value, index, Some(format!("*{name}")), is_key)
}

fn finish_collection(
    frames: &mut Vec<NodeFrame>,
    anchors: &mut HashMap<String, Value>,
    map: bool,
) -> Walk {
    let Some(frame) = frames.pop() else {
        return Walk::Stop;
    };
    let (is_key, anchor, tag, point, value) = match (frame, map) {
        (NodeFrame::Map(state), true) => (
            state.is_key,
            state.anchor,
            state.tag,
            state.point,
            Value::Mapping(state.entries.into_iter().collect()),
        ),
        (NodeFrame::Seq(state), false) => (
            state.is_key,
            state.anchor,
            state.tag,
            state.point,
            Value::Sequence(state.items),
        ),
        _ => return Walk::Stop,
    };
    let value = with_tag(value, tag);
    if let Some(name) = anchor {
        anchors.insert(name, value.clone());
    }
    place_node(frames, value, point, None, is_key)
}

/// serde keeps a tag when libyaml's resolved tag still starts with `!`.
///
/// `!foo` stays a local tag, and the one leading `!` is stripped. `!!map`,
/// `!!seq`, `!<tag:yaml.org,2002:map>` and `!<tag:example.com:foo>` resolve to
/// tags that do not start with `!`, so the value stays untagged.
fn serde_collection_tag(resolved: Option<String>) -> Option<String> {
    let tag = resolved?;
    let bytes = tag.as_bytes();
    let (&bang, rest) = bytes.split_first()?;
    if bang != b'!' {
        return None;
    }
    let kept = if rest.is_empty() { bytes } else { rest };
    std::str::from_utf8(kept).ok().map(str::to_string)
}

fn with_tag(value: Value, tag: Option<String>) -> Value {
    match tag {
        Some(tag) => Value::Tagged(Box::new(serde_yaml::value::TaggedValue {
            tag: serde_yaml::value::Tag::new(tag),
            value,
        })),
        None => value,
    }
}

/// Put `value` into the parent. A repeated key is the duplicate. A value that
/// cannot be attached stops the walk, so a later key is not named in its place.
fn place_node(
    frames: &mut [NodeFrame],
    value: Value,
    index: usize,
    spelling: Option<String>,
    is_key: bool,
) -> Walk {
    if is_key {
        let Some(NodeFrame::Map(map)) = frames.last_mut() else {
            return Walk::Stop;
        };
        if map.keys.iter().any(|key| key == &value) {
            return Walk::Dup(Dup {
                index,
                spelling,
                value,
            });
        }
        map.keys.push(value.clone());
        map.pending = Some(value);
        return Walk::Go;
    }
    match frames.last_mut() {
        Some(NodeFrame::Map(map)) => {
            let Some(key) = map.pending.take() else {
                return Walk::Stop;
            };
            map.entries.push((key, value));
            Walk::Go
        }
        Some(NodeFrame::Seq(seq)) => {
            seq.items.push(value);
            Walk::Go
        }
        None => Walk::Go,
    }
}

/// This node is a key of the current mapping. The next node is then the value.
fn take_key_slot(frames: &mut [NodeFrame]) -> bool {
    let Some(NodeFrame::Map(map)) = frames.last_mut() else {
        return false;
    };
    let is_key = map.expect_key;
    map.expect_key = !map.expect_key;
    is_key
}

struct MapState {
    expect_key: bool,
    keys: Vec<Value>,
    pending: Option<Value>,
    entries: Vec<(Value, Value)>,
    /// This mapping is itself a key of the parent mapping.
    is_key: bool,
    /// Where the duplicate points when this mapping is itself a key.
    point: usize,
    anchor: Option<String>,
    tag: Option<String>,
}

struct SeqState {
    items: Vec<Value>,
    is_key: bool,
    point: usize,
    anchor: Option<String>,
    tag: Option<String>,
}

enum NodeFrame {
    Map(MapState),
    Seq(SeqState),
}

fn resolve_span(text: &str, start: u64, end: u64) -> Option<Value> {
    let slice = text.get(start as usize..end as usize)?;
    if slice.is_empty() {
        return Some(Value::Null);
    }
    serde_yaml::from_str(slice).ok()
}

fn scalar_spelling(scalar: &YamlScalar, value: &Value, bits: &NodeBits) -> String {
    if scalar.value.is_empty() && matches!(value, Value::Tagged(_)) {
        bits.tag_text.clone().unwrap_or_default()
    } else {
        scalar.value.clone()
    }
}

/// Location of a scalar key from its own token.
///
/// A block token starts at `|` or `>`. The content is the line after that
/// header. A quoted token starts at the quote. When an anchor or a tag sits
/// on an earlier line and the raw body is the spelling, the spelling is the
/// point, one character past that quote. An explicit key with no scalar token
/// points at the `?`.
fn scalar_index(text: &str, scalar: &YamlScalar, value: &Value, bits: &NodeBits) -> usize {
    if scalar.value.is_empty() && matches!(value, Value::Tagged(_)) {
        if let Some(at) = bits.tag_at {
            return at;
        }
    }
    let Some(tok) = bits.scalar.as_ref() else {
        return bits.key_at.or(bits.tag_at).unwrap_or(scalar.start as usize);
    };
    match scalar.style {
        ScalarStyle::Plain => tok.start,
        ScalarStyle::Single | ScalarStyle::Double => {
            let open = tok.start;
            let end = tok.end;
            let inner = open + 1;
            let crossed = bits
                .prop_at
                .is_some_and(|prop| line_was_crossed(text, prop, open));
            if crossed && end > inner && text.get(inner..end - 1) == Some(scalar.value.as_str()) {
                inner
            } else {
                open
            }
        }
        ScalarStyle::Literal | ScalarStyle::Folded => block_content_at(text, tok.start, tok.end),
    }
}

/// Content of a block scalar whose indicator is already `start`.
/// A `#` comment is skipped only on the header line.
fn block_content_at(text: &str, start: usize, end: usize) -> usize {
    let bytes = text.as_bytes();
    let end = end.min(bytes.len());
    if start >= end || !matches!(bytes.get(start), Some(b'|' | b'>')) {
        return start;
    }
    let mut i = skip_block_header(bytes, start, end);
    while i < end && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    if i < end && bytes[i] == b'#' {
        while i < end && break_len(text, i).is_none() {
            let Some(ch) = text[i..end].chars().next() else {
                break;
            };
            i += ch.len_utf8();
        }
    }
    if let Some(n) = break_len(text, i) {
        if i + n <= end {
            i += n;
        }
    }
    while i < end && matches!(bytes.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    i.min(end)
}

fn location_index(text: &str, line: usize, column: usize) -> usize {
    let mut index = 0;
    let mut cur_line = 1usize;
    let mut cur_col = 1usize;
    while index < text.len() && (cur_line < line || (cur_line == line && cur_col < column)) {
        if let Some(n) = break_len(text, index) {
            index += n;
            cur_line += 1;
            cur_col = 1;
            continue;
        }
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        index += ch.len_utf8();
        cur_col += 1;
    }
    index
}

enum YamlEvent {
    Scalar(YamlScalar),
    Alias {
        name: String,
        start: u64,
        end: u64,
    },
    MapStart {
        start: u64,
        end: u64,
        tag: Option<String>,
    },
    MapEnd,
    SeqStart {
        start: u64,
        end: u64,
        tag: Option<String>,
    },
    SeqEnd,
    StreamEnd,
    Other,
}

#[derive(Clone, Copy)]
enum ScalarStyle {
    Plain,
    Single,
    Double,
    Literal,
    Folded,
}

struct YamlScalar {
    value: String,
    plain: bool,
    style: ScalarStyle,
    start: u64,
    end: u64,
}

fn line_was_crossed(text: &str, start: usize, end: usize) -> bool {
    text.get(start..end)
        .is_some_and(|gap| gap.contains(['\n', '\r', '\u{0085}', '\u{2028}', '\u{2029}']))
}

fn skip_block_header(bytes: &[u8], mut i: usize, end: usize) -> usize {
    i += 1;
    let mut chomp = false;
    let mut indent = false;
    while i < end {
        match bytes[i] {
            b'+' | b'-' if !chomp => {
                chomp = true;
                i += 1;
            }
            b'1'..=b'9' if !indent => {
                indent = true;
                i += 1;
            }
            _ => break,
        }
    }
    i
}

/// libyaml's column counts one per character and resets on a break.
fn advance_mark(
    text: &str,
    mut index: usize,
    mut line: u64,
    mut column: u64,
    at: usize,
) -> (usize, usize) {
    let at = at.min(text.len());
    while index < at {
        if let Some(n) = break_len(text, index) {
            index += n;
            line += 1;
            column = 0;
            continue;
        }
        let ch = text[index..].chars().next().unwrap_or('\u{FFFD}');
        index += ch.len_utf8();
        column += 1;
    }
    (line as usize + 1, column as usize + 1)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TokKind {
    Anchor,
    Tag,
    Alias,
    Scalar,
    Key,
    SeqStart,
    MapStart,
    Skip,
}

#[derive(Clone)]
struct Tok {
    kind: TokKind,
    start: usize,
    end: usize,
    text: String,
}

struct NodeBits {
    anchor: Option<String>,
    tag_text: Option<String>,
    tag_at: Option<usize>,
    /// Earliest anchor or tag. A break between this and a quote moves the point.
    prop_at: Option<usize>,
    scalar: Option<Tok>,
    alias: Option<Tok>,
    /// Start of a `?` key token. An empty explicit key has no scalar token.
    key_at: Option<usize>,
    collection_at: Option<usize>,
}

struct Tokens {
    toks: Vec<Tok>,
    i: usize,
}

impl Tokens {
    fn scan(text: &str) -> Self {
        let mut toks = Vec::new();
        let Some(parser) = YamlParser::open(text) else {
            return Self { toks, i: 0 };
        };
        unsafe {
            loop {
                let mut token = std::mem::MaybeUninit::<unsafe_libyaml::yaml_token_t>::uninit();
                let failed =
                    unsafe_libyaml::yaml_parser_scan(parser.parser, token.as_mut_ptr()).fail;
                if failed {
                    break;
                }
                let mut token = token.assume_init();
                let start = token.start_mark.index as usize;
                let end = token.end_mark.index as usize;
                let kind = tok_kind(token.type_);
                let text_of = match kind {
                    TokKind::Anchor => c_string(token.data.anchor.value),
                    TokKind::Alias => c_string(token.data.alias.value),
                    TokKind::Tag => text.get(start..end).unwrap_or("").to_string(),
                    TokKind::Scalar => c_bytes(token.data.scalar.value, token.data.scalar.length),
                    _ => String::new(),
                };
                let done = token.type_ == unsafe_libyaml::YAML_STREAM_END_TOKEN;
                unsafe_libyaml::yaml_token_delete(&mut token);
                toks.push(Tok {
                    kind,
                    start,
                    end,
                    text: text_of,
                });
                if done {
                    break;
                }
            }
        }
        Self { toks, i: 0 }
    }

    /// Tokens of the node whose event ends at `event_end`.
    ///
    /// An anchor is kept only so the following token can be the key. A tag on
    /// an empty node is the key. Structural tokens before the node are skipped.
    /// A `?` at or past `event_end` is the next key. An omitted value leaves it.
    fn take(&mut self, event_end: usize) -> NodeBits {
        let mut bits = NodeBits {
            anchor: None,
            tag_text: None,
            tag_at: None,
            prop_at: None,
            scalar: None,
            alias: None,
            key_at: None,
            collection_at: None,
        };
        while self.i < self.toks.len() {
            let tok = self.toks[self.i].clone();
            let next_key = tok.kind == TokKind::Key && tok.start >= event_end;
            let next_node = tok.start > event_end
                && matches!(
                    tok.kind,
                    TokKind::Anchor
                        | TokKind::Tag
                        | TokKind::Scalar
                        | TokKind::Alias
                        | TokKind::SeqStart
                        | TokKind::MapStart
                        | TokKind::Key
                );
            if next_key || next_node {
                break;
            }
            self.i += 1;
            match tok.kind {
                TokKind::Anchor => {
                    bits.prop_at = bits.prop_at.or(Some(tok.start));
                    bits.anchor = Some(tok.text);
                }
                TokKind::Tag => {
                    bits.prop_at = bits.prop_at.or(Some(tok.start));
                    bits.tag_at = Some(tok.start);
                    bits.tag_text = Some(tok.text);
                }
                TokKind::Scalar => {
                    bits.scalar = Some(tok);
                    break;
                }
                TokKind::Alias => {
                    bits.alias = Some(tok);
                    break;
                }
                TokKind::Key => {
                    bits.key_at = Some(tok.start);
                }
                TokKind::SeqStart | TokKind::MapStart => {
                    bits.collection_at = Some(tok.start);
                    break;
                }
                TokKind::Skip => {
                    if tok.start > event_end {
                        self.i -= 1;
                        break;
                    }
                }
            }
        }
        bits
    }
}

fn tok_kind(type_: unsafe_libyaml::yaml_token_type_t) -> TokKind {
    use unsafe_libyaml::{
        YAML_ALIAS_TOKEN, YAML_ANCHOR_TOKEN, YAML_BLOCK_MAPPING_START_TOKEN,
        YAML_BLOCK_SEQUENCE_START_TOKEN, YAML_FLOW_MAPPING_START_TOKEN,
        YAML_FLOW_SEQUENCE_START_TOKEN, YAML_KEY_TOKEN, YAML_SCALAR_TOKEN, YAML_TAG_TOKEN,
    };
    match type_ {
        YAML_ANCHOR_TOKEN => TokKind::Anchor,
        YAML_TAG_TOKEN => TokKind::Tag,
        YAML_ALIAS_TOKEN => TokKind::Alias,
        YAML_KEY_TOKEN => TokKind::Key,
        YAML_SCALAR_TOKEN => TokKind::Scalar,
        YAML_BLOCK_SEQUENCE_START_TOKEN | YAML_FLOW_SEQUENCE_START_TOKEN => TokKind::SeqStart,
        YAML_BLOCK_MAPPING_START_TOKEN | YAML_FLOW_MAPPING_START_TOKEN => TokKind::MapStart,
        _ => TokKind::Skip,
    }
}

fn c_str_owned(ptr: *const libc::c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned() })
    }
}

fn copied_tag(tag: *const u8) -> Option<String> {
    if tag.is_null() {
        None
    } else {
        let text = c_string(tag);
        (!text.is_empty()).then_some(text)
    }
}

fn c_string(ptr: *const u8) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        unsafe {
            std::ffi::CStr::from_ptr(ptr.cast())
                .to_string_lossy()
                .into_owned()
        }
    }
}

fn c_bytes(ptr: *const u8, len: u64) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        unsafe {
            let bytes = std::slice::from_raw_parts(ptr, len as usize);
            String::from_utf8_lossy(bytes).into_owned()
        }
    }
}

struct YamlParser {
    parser: *mut unsafe_libyaml::yaml_parser_t,
}

impl YamlParser {
    fn open(text: &str) -> Option<Self> {
        let parser = Box::into_raw(Box::new(std::mem::MaybeUninit::<
            unsafe_libyaml::yaml_parser_t,
        >::uninit()));
        // The parser struct stays in this allocation. libyaml stores heap
        // buffers in it and reads `text` until the parser is deleted.
        unsafe {
            if unsafe_libyaml::yaml_parser_initialize(parser.cast()).fail {
                drop(Box::from_raw(parser));
                return None;
            }
            let parser = parser.cast();
            unsafe_libyaml::yaml_parser_set_encoding(parser, unsafe_libyaml::YAML_UTF8_ENCODING);
            unsafe_libyaml::yaml_parser_set_input_string(parser, text.as_ptr(), text.len() as u64);
            Some(Self { parser })
        }
    }

    fn next_event(&mut self) -> Option<YamlEvent> {
        unsafe {
            let mut event = std::mem::MaybeUninit::<unsafe_libyaml::yaml_event_t>::uninit();
            let failed = unsafe_libyaml::yaml_parser_parse(self.parser, event.as_mut_ptr()).fail;
            let mut event = event.assume_init();
            let parsed = if failed { None } else { owned_event(&event) };
            unsafe_libyaml::yaml_event_delete(&mut event);
            parsed
        }
    }

    fn error_index(&self) -> usize {
        unsafe {
            let parser = &*self.parser;
            parser.problem_mark.index as usize
        }
    }

    fn failure(&self) -> Option<LibyamlFailure> {
        unsafe {
            let parser = &*self.parser;
            let problem = c_str_owned(parser.problem)?;
            Some(LibyamlFailure {
                problem,
                problem_offset: parser.problem_offset as usize,
                problem_index: parser.problem_mark.index as usize,
                problem_line: parser.problem_mark.line as usize,
                problem_column: parser.problem_mark.column as usize,
                context: c_str_owned(parser.context),
                context_index: parser.context_mark.index as usize,
                context_line: parser.context_mark.line as usize,
                context_column: parser.context_mark.column as usize,
            })
        }
    }

    /// Span of a plain scalar the scanner rejected before finishing the event.
    ///
    /// `{1e0:}` is one plain scalar up to the `:`, and libyaml stops there
    /// instead of emitting the event. The context mark is that start.
    fn aborted_plain_spelling(&self, text: &str) -> Option<(usize, usize)> {
        let (start, end) = unsafe {
            let parser = &*self.parser;
            let context = parser.context;
            if context.is_null()
                || std::ffi::CStr::from_ptr(context).to_bytes() != b"while scanning a plain scalar"
            {
                return None;
            }
            (
                parser.context_mark.index as usize,
                parser.problem_mark.index as usize,
            )
        };
        let slice = text.get(start..end)?;
        text_spelling(slice).then_some((start, end))
    }
}

impl Drop for YamlParser {
    fn drop(&mut self) {
        unsafe {
            unsafe_libyaml::yaml_parser_delete(self.parser);
            drop(Box::from_raw(self.parser));
        }
    }
}

unsafe fn owned_event(event: &unsafe_libyaml::yaml_event_t) -> Option<YamlEvent> {
    use unsafe_libyaml::{
        YAML_ALIAS_EVENT, YAML_DOUBLE_QUOTED_SCALAR_STYLE, YAML_FOLDED_SCALAR_STYLE,
        YAML_LITERAL_SCALAR_STYLE, YAML_MAPPING_END_EVENT, YAML_MAPPING_START_EVENT,
        YAML_PLAIN_SCALAR_STYLE, YAML_SCALAR_EVENT, YAML_SEQUENCE_END_EVENT,
        YAML_SEQUENCE_START_EVENT, YAML_SINGLE_QUOTED_SCALAR_STYLE, YAML_STREAM_END_EVENT,
    };
    Some(match event.type_ {
        YAML_STREAM_END_EVENT => YamlEvent::StreamEnd,
        YAML_SCALAR_EVENT => {
            let scalar = &event.data.scalar;
            let value = if scalar.value.is_null() {
                String::new()
            } else {
                let bytes = std::slice::from_raw_parts(scalar.value, scalar.length as usize);
                String::from_utf8(bytes.to_vec()).ok()?
            };
            let style = match scalar.style {
                YAML_SINGLE_QUOTED_SCALAR_STYLE => ScalarStyle::Single,
                YAML_DOUBLE_QUOTED_SCALAR_STYLE => ScalarStyle::Double,
                YAML_LITERAL_SCALAR_STYLE => ScalarStyle::Literal,
                YAML_FOLDED_SCALAR_STYLE => ScalarStyle::Folded,
                _ => ScalarStyle::Plain,
            };
            YamlEvent::Scalar(YamlScalar {
                value,
                plain: scalar.style == YAML_PLAIN_SCALAR_STYLE,
                style,
                start: event.start_mark.index,
                end: event.end_mark.index,
            })
        }
        YAML_ALIAS_EVENT => YamlEvent::Alias {
            name: c_string(event.data.alias.anchor),
            start: event.start_mark.index,
            end: event.end_mark.index,
        },
        YAML_MAPPING_START_EVENT => YamlEvent::MapStart {
            start: event.start_mark.index,
            end: event.end_mark.index,
            tag: copied_tag(event.data.mapping_start.tag),
        },
        YAML_MAPPING_END_EVENT => YamlEvent::MapEnd,
        YAML_SEQUENCE_START_EVENT => YamlEvent::SeqStart {
            start: event.start_mark.index,
            end: event.end_mark.index,
            tag: copied_tag(event.data.sequence_start.tag),
        },
        YAML_SEQUENCE_END_EVENT => YamlEvent::SeqEnd,
        _ => YamlEvent::Other,
    })
}

fn byte_location(bytes: &[u8], offset: usize) -> (usize, usize) {
    let before = &bytes[..offset];
    let line = before.iter().filter(|byte| **byte == b'\n').count() + 1;
    let column = before
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(offset + 1, |newline| offset - newline);
    (line, column)
}

fn entry(e: &Value) -> Result<ListLine, String> {
    let (name, options) = match e {
        Value::String(name) => (name, None),
        Value::Mapping(m) if m.len() == 1 => {
            let (k, v) = m.iter().next().expect("one key");
            let Value::String(name) = k else {
                return Err(format!("the easyconfig name `{}` is not a string", show(k)));
            };
            (name, options_of(name, v)?)
        }
        Value::Mapping(m) if m.len() > 1 => {
            let keys: Vec<String> = m.keys().map(show).collect();
            return Err(format!(
                "the entry has {} keys, {}: indent `options` under the easyconfig name",
                m.len(),
                keys.join(", ")
            ));
        }
        _ => {
            return Err(
                "an entry is an easyconfig name, or a mapping of one name to its options".into(),
            )
        }
    };
    if name.is_empty() {
        return Err("the easyconfig name is empty".into());
    }
    let easyconfig = if name.ends_with(".eb") {
        name.clone()
    } else {
        format!("{name}.eb")
    };
    let args = match options {
        Some(o) => eb_args(o).map_err(|m| format!("{easyconfig}: {m}"))?,
        None => Vec::new(),
    };
    let mut raw = Vec::with_capacity(1 + args.len());
    for w in std::iter::once(&easyconfig).chain(&args) {
        raw.push(list_word(w).map_err(|e| format!("{easyconfig}: {e}"))?);
    }
    Ok(ListLine {
        raw: raw.join(" "),
        easyconfig,
        args,
    })
}

/// The `options` of the entry `name`, the only key EasyBuild takes there.
fn options_of<'a>(name: &str, v: &'a Value) -> Result<Option<&'a Mapping>, String> {
    let spec = match v {
        Value::Null => return Ok(None),
        Value::Mapping(spec) => spec,
        _ => return Err(format!("{name}: put its options under `options:`")),
    };
    let mut other: Vec<String> = spec
        .keys()
        .filter(|k| k.as_str() != Some("options"))
        .map(show)
        .collect();
    if !other.is_empty() {
        other.sort();
        return Err(format!(
            "{name}: only `options` can follow the name, not {}",
            other.join(", ")
        ));
    }
    match spec.get("options") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Mapping(o)) => Ok(Some(o)),
        Some(_) => Err(format!(
            "{name}: `options` is not a mapping of option names to values"
        )),
    }
}

/// The `eb` options of an entry, as `opts_dict_to_eb_opts` writes them:
/// sorted by name, `-x` for a name of one letter and `--name` for a longer
/// one; true, no value or the number 1 gives the bare option, false
/// `--disable-name`, a list its items joined with `,`, and anything else
/// `--name=value`, except that `amend` and `try-amend` take one option per
/// item.
fn eb_args(options: &Mapping) -> Result<Vec<String>, String> {
    let mut named = Vec::new();
    for (k, v) in options {
        let Value::String(k) = k else {
            return Err(format!("the option name `{}` is not a string", show(k)));
        };
        if k.is_empty() || k.starts_with('-') {
            return Err(format!(
                "the option `{k}`: write an option's name without its dashes"
            ));
        }
        named.push((k.as_str(), v));
    }
    named.sort_by_key(|(k, _)| *k);
    let mut args = Vec::new();
    for (k, v) in named {
        let short = k.chars().count() == 1;
        let opt = if short {
            format!("-{k}")
        } else {
            format!("--{k}")
        };
        if k == "amend" || k == "try-amend" {
            let items: Vec<&Value> = match v {
                Value::Sequence(s) => s.iter().collect(),
                v => vec![v],
            };
            for x in items {
                args.push(format!("{opt}={}", text(k, x)?));
            }
            continue;
        }
        match flag(v) {
            Some(true) => args.push(opt),
            Some(false) if short => {
                return Err(format!(
                    "the option `{k}`: a one-letter option has no --disable- form"
                ))
            }
            Some(false) => args.push(format!("--disable-{k}")),
            None if short => {
                return Err(format!(
                    "the option `{k}`: give a one-letter option that takes a value by its long name"
                ))
            }
            None => {
                let value = match v {
                    Value::Sequence(s) => s
                        .iter()
                        .map(|x| text(k, x))
                        .collect::<Result<Vec<_>, _>>()?
                        .join(","),
                    v => text(k, v)?,
                };
                args.push(format!("{opt}={value}"));
            }
        }
    }
    Ok(args)
}

/// `Some(true)` for a value that gives the bare option, and `Some(false)`
/// for false. EasyBuild reads YAML 1.1, where yes, on, no and off are
/// booleans too, and gives the bare option for each value in `[True,
/// None]`, which in Python holds the numbers 1 and 1.0 as well.
fn flag(v: &Value) -> Option<bool> {
    match v {
        Value::Null => Some(true),
        Value::Bool(b) => Some(*b),
        Value::String(s) => yaml11_bool(s),
        Value::Number(n) if n.as_f64() == Some(1.0) => Some(true),
        _ => None,
    }
}

fn yaml11_bool(s: &str) -> Option<bool> {
    match s {
        "yes" | "Yes" | "YES" | "on" | "On" | "ON" => Some(true),
        "no" | "No" | "NO" | "off" | "Off" | "OFF" => Some(false),
        _ => None,
    }
}

/// A value of the option `k` as EasyBuild writes it, with Python's `str()`.
fn text(k: &str, v: &Value) -> Result<String, String> {
    match v {
        Value::String(s) => Ok(match yaml11_bool(s) {
            Some(b) => python_bool(b).into(),
            None => s.clone(),
        }),
        Value::Bool(b) => Ok(python_bool(*b).into()),
        Value::Null => Ok("None".into()),
        Value::Number(n) if n.is_i64() || n.is_u64() => Ok(n.to_string()),
        Value::Number(n) => {
            Ok(python_float(n.as_f64().ok_or_else(|| {
                format!("the option `{k}`: `{n}` is not a finite number")
            })?))
        }
        _ => Err(format!(
            "the option `{k}`: a value is text, a number, a boolean or a list of them"
        )),
    }
}

/// Format a binary float as Python's `str(float)` does.
fn python_float(value: f64) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value == f64::INFINITY {
        return "inf".into();
    }
    if value == f64::NEG_INFINITY {
        return "-inf".into();
    }

    // Ryu follows round-to-nearest, ties-to-even when two shortest decimal
    // representations round back to the same binary float. This is the rule
    // used by CPython's dtoa mode 0; Rust's Display implementation may choose
    // the other decimal in that case.
    let mut buffer = ryu::Buffer::new();
    let lexical = buffer.format_finite(value);
    let (negative, unsigned) = lexical
        .strip_prefix('-')
        .map_or((false, lexical), |s| (true, s));
    let (mantissa, explicit_exponent) = unsigned
        .split_once(['e', 'E'])
        .map_or((unsigned, 0), |(m, e)| (m, e.parse::<i32>().unwrap()));
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let raw_digits = format!("{integer}{fraction}");

    if raw_digits.bytes().all(|b| b == b'0') {
        return if negative { "-0.0" } else { "0.0" }.into();
    }

    let leading = raw_digits.bytes().take_while(|b| *b == b'0').count();
    let mut digits = raw_digits[leading..].trim_end_matches('0').to_string();
    let exponent = if integer.bytes().any(|b| b != b'0') {
        integer.len() as i32 - integer.bytes().take_while(|b| *b == b'0').count() as i32 - 1
    } else {
        -(fraction.bytes().take_while(|b| *b == b'0').count() as i32) - 1
    } + explicit_exponent;

    let mut result = String::new();
    if negative {
        result.push('-');
    }
    if !(-4..16).contains(&exponent) {
        result.push(digits.remove(0));
        if !digits.is_empty() {
            result.push('.');
            result.push_str(&digits);
        }
        result.push('e');
        result.push(if exponent < 0 { '-' } else { '+' });
        result.push_str(&format!("{:02}", exponent.unsigned_abs()));
    } else if exponent < 0 {
        result.push_str("0.");
        result.extend(std::iter::repeat_n('0', (-exponent - 1) as usize));
        result.push_str(&digits);
    } else {
        let decimal = exponent as usize + 1;
        if digits.len() <= decimal {
            result.push_str(&digits);
            result.extend(std::iter::repeat_n('0', decimal - digits.len()));
            result.push_str(".0");
        } else {
            result.push_str(&digits[..decimal]);
            result.push('.');
            result.push_str(&digits[decimal..]);
        }
    }
    result
}

fn python_bool(b: bool) -> &'static str {
    if b {
        "True"
    } else {
        "False"
    }
}

fn show(v: &Value) -> String {
    serde_yaml::to_string(v)
        .map(|s| s.trim_end().to_string())
        .unwrap_or_else(|_| "?".into())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        parse(text).unwrap().into_iter().map(|l| l.raw).collect()
    }

    fn refused(text: &str) -> String {
        parse(text).unwrap_err()
    }

    #[test]
    fn names_get_eb_and_keep_their_order() {
        assert_eq!(
            lines("easyconfigs:\n  - b-1.0\n  - a-1.0.eb\n  - c-1.0.eb:\n"),
            ["b-1.0.eb", "a-1.0.eb", "c-1.0.eb"]
        );
    }

    #[test]
    fn ordinary_easyconfig_bytes_stay_unchanged() {
        let line = &parse("easyconfigs:\n  - Foo-1.0-foss-2025a.eb\n").unwrap()[0];
        assert_eq!(line.raw.as_bytes(), b"Foo-1.0-foss-2025a.eb");
        assert_eq!(line.easyconfig.as_bytes(), b"Foo-1.0-foss-2025a.eb");
    }

    #[test]
    fn options_become_eb_options_sorted_by_name() {
        let text = "easyconfigs:
  - x-1.0.eb:
      options:
        rebuild: true
        from-pr: 12345
        robot: false
        include-easyblocks: [a.py, b.py]
        amend: [versionsuffix=-a, toolchain=b]
        force:
        f: yes
        accept-eula-for: 'A B'
";
        let l = &parse(text).unwrap()[0];
        assert_eq!(
            l.args,
            [
                "--accept-eula-for=A B",
                "--amend=versionsuffix=-a",
                "--amend=toolchain=b",
                "-f",
                "--force",
                "--from-pr=12345",
                "--include-easyblocks=a.py,b.py",
                "--rebuild",
                "--disable-robot",
            ]
        );
        assert_eq!(
            l.raw,
            "x-1.0.eb '--accept-eula-for=A B' --amend=versionsuffix=-a --amend=toolchain=b -f --force --from-pr=12345 --include-easyblocks=a.py,b.py --rebuild --disable-robot"
        );
        assert_eq!(crate::util::parse_list(&l.raw).unwrap()[0], *l);
    }

    #[test]
    fn yaml_merge_keys_supply_easyconfig_options_as_in_easybuild() {
        let text = r#"
defaults: &defaults
  options:
    rebuild: true
    robot: false
easyconfigs:
  - x-1.0.eb:
      <<: *defaults
"#;
        assert_eq!(lines(text), ["x-1.0.eb --rebuild --disable-robot"],);
    }

    #[test]
    fn ordinary_easystack_line_bytes_remain_unchanged() {
        let parsed =
            parse("easyconfigs:\n  - x-1.0.eb: {options: {rebuild: true, robot: false}}\n")
                .unwrap();
        assert_eq!(
            parsed[0].raw.as_bytes(),
            b"x-1.0.eb --rebuild --disable-robot"
        );
    }

    #[test]
    fn yaml_1_1_booleans_are_booleans_as_in_easybuild() {
        let l =
            &parse("easyconfigs:\n  - x.eb: {options: {rebuild: on, robot: off, a-b: [yes, 1]}}\n")
                .unwrap()[0];
        assert_eq!(l.args, ["--a-b=True,1", "--rebuild", "--disable-robot"]);
    }

    #[test]
    fn an_apostrophe_and_a_shell_character_stay_one_argument() {
        let l = &parse(
            "easyconfigs:\n  - x.eb: {options: {amend: \"it's\", hooks: \"/tmp/a(b).py\"}}\n",
        )
        .unwrap()[0];
        assert_eq!(l.args, ["--amend=it's", "--hooks=/tmp/a(b).py"]);
        assert_eq!(l.raw, "x.eb \"--amend=it's\" --hooks=/tmp/a(b).py");
        assert_eq!(crate::util::parse_list(&l.raw).unwrap()[0], *l);
    }

    #[test]
    fn a_plain_one_gives_the_bare_option_as_in_easybuild() {
        let l = &parse(
            "easyconfigs:\n  - x.eb: {options: {rebuild: 1, parallel: '1', a-b: [1], amend: 1, c-d: 0}}\n",
        )
        .unwrap()[0];
        assert_eq!(
            l.args,
            [
                "--a-b=1",
                "--amend=1",
                "--c-d=0",
                "--parallel=1",
                "--rebuild"
            ]
        );
    }

    #[test]
    fn a_floating_one_gives_the_bare_option_as_in_easybuild() {
        let l = &parse("easyconfigs:\n  - x.eb: {options: {rebuild: 1.0, parallel: '1.0'}}\n")
            .unwrap()[0];
        assert_eq!(l.args, ["--parallel=1.0", "--rebuild"]);
    }

    #[test]
    fn decimal_option_values_are_written_as_in_easybuild() {
        let l = &parse(
            "easyconfigs:\n  - x.eb: {options: {amend: 2.5, large: 1.0e+20, nan: .nan, negative: -.inf, parallel: 1.5, positive: .inf, ratios: [2.5, 3.25], small: 0.000001}}\n",
        )
        .unwrap()[0];
        assert_eq!(
            l.args,
            [
                "--amend=2.5",
                "--large=1e+20",
                "--nan=nan",
                "--negative=-inf",
                "--parallel=1.5",
                "--positive=inf",
                "--ratios=2.5,3.25",
                "--small=1e-06"
            ]
        );
    }

    #[test]
    fn decimal_tie_is_written_as_in_python() {
        let l = &parse("easyconfigs:\n  - x.eb: {options: {parallel: 1136789840277548.3}}\n")
            .unwrap()[0];
        assert_eq!(l.args, ["--parallel=1136789840277548.2"]);
    }

    #[test]
    fn finite_floats_are_written_as_in_python() {
        for (line_number, line) in include_str!("../tests/data/python-float-repr.tsv")
            .lines()
            .enumerate()
        {
            if line.starts_with('#') {
                continue;
            }
            let (bits, expected) = line
                .split_once('\t')
                .unwrap_or_else(|| panic!("fixture line {} has no tab", line_number + 1));
            let bits = u64::from_str_radix(bits, 16)
                .unwrap_or_else(|_| panic!("fixture line {} has invalid bits", line_number + 1));
            assert_eq!(
                python_float(f64::from_bits(bits)),
                expected,
                "fixture line {} for bits {bits:016x}",
                line_number + 1
            );
        }
    }

    #[test]
    fn ordinary_option_output_keeps_its_exact_bytes() {
        let l = &parse(
            "easyconfigs:\n  - x.eb: {options: {rebuild: true, parallel: 4, robot: false}}\n",
        )
        .unwrap()[0];
        assert_eq!(
            l.raw.as_bytes(),
            b"x.eb --parallel=4 --rebuild --disable-robot"
        );
    }

    #[test]
    fn other_top_level_keys_are_ignored_as_eb_ignores_them() {
        assert_eq!(
            lines("robot: true\neasybuild_version: 5.4.2\neasyconfigs: [x-1.0.eb]\n"),
            ["x-1.0.eb"]
        );
    }

    #[test]
    fn what_eb_would_not_build_as_written_is_refused() {
        for (text, needed) in [
            ("easyconfig: [x.eb]\n", "no top-level `easyconfigs` key"),
            ("easyconfigs: x.eb\n", "`easyconfigs` is not a list"),
            ("easyconfigs:\n  x.eb: {}\n", "`easyconfigs` is not a list"),
            (
                "easyconfigs:\n  - [x.eb]\n",
                "entry 1: an entry is an easyconfig name",
            ),
            (
                "easyconfigs:\n  - a.eb:\n    options:\n      rebuild: true\n",
                "entry 1: the entry has 2 keys, a.eb, options: indent `options` under the easyconfig name",
            ),
            (
                "easyconfigs:\n  - x.eb\n  - y.eb: {opts: {}}\n",
                "entry 2: y.eb: only `options` can follow the name, not opts",
            ),
            (
                "easyconfigs:\n  - x.eb: {options: [rebuild]}\n",
                "`options` is not a mapping",
            ),
            (
                "easyconfigs:\n  - x.eb: {options: {--rebuild: true}}\n",
                "without its dashes",
            ),
            (
                "easyconfigs:\n  - x.eb: {options: {hooks: {a: b}}}\n",
                "a value is text",
            ),
            (
                "easyconfigs:\n  - x.eb: {options: {r: false}}\n",
                "no --disable- form",
            ),
            (
                "easyconfigs:\n  - x.eb: {options: {r: /x}}\n",
                "by its long name",
            ),
            (
                "easyconfigs:\n  - x.eb: {options: {a-b: \"a\\nb\"}}\n",
                "line break",
            ),
            ("easyconfigs:\n  - ''\n", "the easyconfig name is empty"),
            ("easyconfigs: [\n", "while parsing"),
        ] {
            let e = refused(text);
            assert!(e.contains(needed), "{text:?}: {e}");
        }
    }

    #[test]
    fn only_yml_and_yaml_files_are_easystacks() {
        for (p, want) in [
            ("rebuilds/x.yml", true),
            ("x.yaml", true),
            ("rebuilds/x.YML", true),
            ("x.YAML", true),
            ("x.list", false),
            ("x.yml.list", false),
            ("yml", false),
        ] {
            assert_eq!(is_easystack(Path::new(p)), want, "{p}");
        }
    }

    #[test]
    #[allow(
        clippy::disallowed_methods,
        reason = "this seeds a temporary easystack, not a user-visible output path"
    )]
    fn a_duplicate_key_after_a_unicode_line_break_points_at_the_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        // An empty YAML line sits between the two keys when the gap is two breaks.
        for (gap, line) in [
            ("\u{2028}", 5),
            ("\u{2029}", 5),
            ("\u{0085}", 5),
            ("\r", 5),
            ("\u{2028}\u{2028}", 6),
        ] {
            let text = format!(
                "easyconfigs:\n  - x.eb:\n      options:\n        rebuild: true{gap}        rebuild: false\n"
            );
            std::fs::write(&path, &text).unwrap();
            let err = read(&path).unwrap_err();
            assert_eq!(
                err,
                format!(
                    "easystack {}:{}:9: easyconfigs[0].x.eb.options: duplicate entry with key \"rebuild\"",
                    path.display(),
                    line
                ),
                "{gap:?}"
            );
        }
    }

    #[test]
    fn plain_spellings_easybuild_keeps_as_text_stay_text() {
        let text = "easyconfigs:
  - x.eb:
      options:
        exp: 1.5e+1
        kept: 1.0
        large: 1.0e+20
        mask: 0o20
        neg: -.5
        bare: foo,1e0
        flow: [1e0, x]
        note: \"see 1e0\"
        parallel: 1.5e1
        parts: [10e-1, 1.0]
        plus: +1.0
        port: 1e0:80
        prose: see 1e0 later
        ratio: +.5
        rebuild: 1e0
        signed: 1.
        whole: 10e-1
";
        let l = &parse(text).unwrap()[0];
        assert_eq!(
            l.args,
            [
                "--bare=foo,1e0",
                "--exp=15.0",
                "--flow=1e0,x",
                "--kept",
                "--large=1e+20",
                "--mask=0o20",
                "--neg=-.5",
                "--note=see 1e0",
                "--parallel=1.5e1",
                "--parts=10e-1,1.0",
                "--plus",
                "--port=1e0:80",
                "--prose=see 1e0 later",
                "--ratio=+.5",
                "--rebuild=1e0",
                "--signed",
                "--whole=10e-1",
            ]
        );
    }

    #[test]
    fn quoting_leaves_block_scalars_comments_and_quoted_text() {
        let text = "\
# 1e0 stays a comment
easyconfigs:
  - x.eb:
      options:
        amend: |
          1e0
        note: \"see 1e0\"
        plain: foo:1e0
        rebuild: 1e0
";
        let quoted = quote_scalars_easybuild_keeps_as_text(text);
        assert!(quoted.contains("# 1e0 stays a comment\n"), "{quoted}");
        assert!(quoted.contains("          1e0\n"), "{quoted}");
        assert!(quoted.contains("\"see 1e0\""), "{quoted}");
        assert!(quoted.contains("plain: foo:1e0\n"), "{quoted}");
        assert!(quoted.contains("rebuild: '1e0'\n"), "{quoted}");
        let embedded = "\
easyconfigs:
  - x.eb:
      options:
        bare: foo,1e0
        flow: [1e0, x]
        port: 1e0:80
        prose: see 1e0 later
";
        let embedded = quote_scalars_easybuild_keeps_as_text(embedded);
        assert!(embedded.contains("bare: foo,1e0\n"), "{embedded}");
        assert!(embedded.contains("flow: ['1e0', x]\n"), "{embedded}");
        assert!(embedded.contains("port: 1e0:80\n"), "{embedded}");
        assert!(embedded.contains("prose: see 1e0 later\n"), "{embedded}");
        for (input, want) in [
            ("note: see\n  1e0 later\n", "note: see\n  1e0 later\n"),
            ("note:\n  1e0\n", "note:\n  '1e0'\n"),
            ("note: &a 1e0\n", "note: &a '1e0'\n"),
            ("note: [see 1e0 later, x]\n", "note: [see 1e0 later, x]\n"),
            ("note: see [1e0] later\n", "note: see [1e0] later\n"),
            ("note: {k: 1e0}\n", "note: {k: '1e0'}\n"),
            ("note: {1e0:}\n", "note: {'1e0':}\n"),
            ("note: [1e0:x]\n", "note: [1e0:x]\n"),
            ("note: 1e0 # c\n", "note: '1e0' # c\n"),
            ("note: 1e0#c\n", "note: 1e0#c\n"),
            ("- 1e0\n", "- '1e0'\n"),
            ("- -.5\n", "- '-.5'\n"),
        ] {
            let quoted = quote_scalars_easybuild_keeps_as_text(input);
            assert_eq!(quoted, want, "{input:?}");
            serde_yaml::from_str::<Value>(&quoted).unwrap_or_else(|e| panic!("{quoted}: {e}"));
        }
        let err = refused("easyconfigs:\n  - x.eb:\n      options:\n        parallel: 1e0\n        rebuild: true\n        rebuild: false\n");
        assert!(
            err.contains("duplicate entry with key \"rebuild\""),
            "{err}"
        );
    }

    #[test]
    fn a_duplicate_inside_a_flow_map_uses_the_original_column() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        std::fs::write(&path, "opts: {rebuild: 1e0, flags: {1e0: a, '1e0': b}}\n").unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:1:38: opts.flags: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
        std::fs::write(
            &path,
            "easyconfigs:\n  - x.eb: {note: 1e0, options: {1e0: 1, '1e0': 2}}\n",
        )
        .unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:2:41: easyconfigs[0].x.eb.options: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
    }

    #[test]
    fn a_quoted_flow_duplicate_points_at_the_second_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        // `{` is column 15. The repeated key `'1e0'` starts at column 26.
        let line = "     options: {'1e0': a, '1e0': b}";
        let column = line.rfind("'1e0'").expect("repeated key") + 1;
        assert_eq!(column, 26);
        std::fs::write(&path, format!("{line}\n")).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:1:{column}: options: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
    }

    #[test]
    fn a_repeated_text_spelling_keeps_the_users_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for key in ["1e0", "0o20", "+.5", "1.5e1"] {
            std::fs::write(
                &path,
                format!(
                    "easyconfigs:\n  - x.eb:\n      options:\n        {key}: 1\n        {key}: 2\n"
                ),
            )
            .unwrap();
            assert_eq!(
                read(&path).unwrap_err(),
                format!(
                    "easystack {}:5:9: easyconfigs[0].x.eb.options: duplicate entry with key \"{key}\"",
                    path.display()
                ),
                "{key}"
            );
        }
    }

    #[test]
    fn a_nested_flow_map_duplicate_keeps_the_users_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for key in ["1e0", "0o20", "+.5"] {
            let line = format!("  - x.eb: {{options: {{{key}: 1, {key}: 2}}}}");
            let column = line.rfind(key).expect("repeated key") + 1;
            std::fs::write(&path, format!("easyconfigs:\n{line}\n")).unwrap();
            assert_eq!(
                read(&path).unwrap_err(),
                format!(
                    "easystack {}:2:{column}: easyconfigs[0].x.eb.options: duplicate entry with key \"{key}\"",
                    path.display()
                ),
                "{key}"
            );
        }
    }

    fn position(text: &str, byte: usize) -> (usize, usize) {
        byte_location(text.as_bytes(), byte)
    }

    #[test]
    fn a_block_scalar_body_is_not_scanned_for_duplicate_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let flow = "note: |\n  {rebuild: 1, rebuild: 2}\nflags: {rebuild: 1, rebuild: 2}\n";
        let flow_at = flow.rfind("rebuild").expect("repeated rebuild");
        assert_eq!(position(flow, flow_at), (3, 21));
        std::fs::write(&path, flow).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:3:21: flags: duplicate entry with key \"rebuild\"",
                path.display()
            )
        );
        let literal = "note: |\n  1e0: 1\n  1e0: 2\nflags: {1e0: 1, 1e0: 2}\n";
        let literal_at = literal.rfind("1e0").expect("repeated 1e0");
        assert_eq!(position(literal, literal_at), (4, 17));
        std::fs::write(&path, literal).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:4:17: flags: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
    }

    #[test]
    fn an_earlier_flow_duplicate_wins_over_a_later_block_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "\
easyconfigs:
  - x.eb:
      options: {1e0: 1, 1e0: 2}
  - y.eb:
      options:
        1e0: 1
        1e0: 2
";
        let flow = "{1e0: 1, 1e0: 2}";
        let at = text.find(flow).expect("flow map") + flow.rfind("1e0").expect("repeated 1e0");
        assert_eq!(position(text, at), (3, 25));
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:3:25: easyconfigs[0].x.eb.options: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
        let quoted = "opts: {1e0: a, '1e0': b}\n1e0: 1\n1e0: 2\n";
        let at = quoted.find("'1e0'").expect("quoted key");
        assert_eq!(position(quoted, at), (1, 16));
        std::fs::write(&path, quoted).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:1:16: opts: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
    }

    #[test]
    fn a_quoted_flow_key_with_no_blank_after_the_colon_points_at_the_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for (text, column, detail) in [
            (
                "{\"1e0\":1,\"1e0\":2}\n",
                10,
                "duplicate entry with key \"1e0\"",
            ),
            (
                "{'1e0':1,'1e0':2}\n",
                10,
                "duplicate entry with key \"1e0\"",
            ),
            (
                "{\"a\":{\"1e0\":1,\"1e0\":2}}\n",
                15,
                "a: duplicate entry with key \"1e0\"",
            ),
        ] {
            let needle = if text.contains("'1e0'") {
                "'1e0'"
            } else {
                "\"1e0\""
            };
            assert_eq!(
                position(text, text.rfind(needle).expect(needle)),
                (1, column)
            );
            std::fs::write(&path, text).unwrap();
            assert_eq!(
                read(&path).unwrap_err(),
                format!("easystack {}:1:{column}: {detail}", path.display()),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_repeated_explicit_key_points_at_the_users_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "\
easyconfigs:
  - x.eb:
      options:
        ? 1e0
        : 1
        ? 1e0
        : 2
";
        let at = text.rfind("1e0").expect("repeated key");
        assert_eq!(position(text, at), (6, 11));
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:6:11: easyconfigs[0].x.eb.options: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
    }

    #[test]
    fn an_anchor_or_a_tag_before_a_block_indicator_leaves_the_body() {
        for header in ["&a", "!!str"] {
            let text = format!(
                "easyconfigs:\n  - x.eb:\n      options:\n        amend: {header} >-\n          # c\n          1e0\n"
            );
            let quoted = quote_scalars_easybuild_keeps_as_text(&text);
            assert!(
                quoted.contains(&format!("amend: {header} >-\n")),
                "{quoted}"
            );
            assert!(quoted.contains("          # c\n"), "{quoted}");
            assert!(quoted.contains("          1e0\n"), "{quoted}");
            assert!(!quoted.contains("'1e0'"), "{quoted}");
            assert_eq!(parse(&text).unwrap()[0].args, ["--amend=# c 1e0"]);
        }
    }

    #[test]
    fn a_repeated_escaped_quote_points_at_the_second_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let single = "k:\n  'it''s': 1\n  'it''s': 2\n";
        let at = single.rfind("'it''s'").expect("repeated key");
        assert_eq!(position(single, at), (3, 3));
        std::fs::write(&path, single).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:3:3: k: duplicate entry with key \"it's\"",
                path.display()
            )
        );
        let double = "k:\n  \"a\\\"b\": 1\n  \"a\\\"b\": 2\n";
        let at = double.rfind("\"a\\\"b\"").expect("repeated key");
        assert_eq!(position(double, at), (3, 3));
        std::fs::write(&path, double).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:3:3: k: duplicate entry with key \"a\\\"b\"",
                path.display()
            )
        );
        let flow = "opts: {\"a\\\"b\": 1, \"a\\\"b\": 2}\n";
        let at = flow.rfind("\"a\\\"b\"").expect("repeated key");
        assert_eq!(position(flow, at), (1, 19));
        std::fs::write(&path, flow).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:1:19: opts: duplicate entry with key \"a\\\"b\"",
                path.display()
            )
        );
    }

    fn duplicate_at(text: &str, needle: &str) -> (usize, usize) {
        let at = text
            .rfind(needle)
            .unwrap_or_else(|| panic!("missing {needle}"));
        position(text, at)
    }

    #[test]
    fn a_same_line_value_stops_at_the_key_column() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for key in ["1e0", "0o20", "+.5"] {
            let text = format!(
                "easyconfigs:\n  - x.eb:\n      options:\n        - {key}: 1\n          {key}: 2\n"
            );
            let (line, column) = duplicate_at(&text, key);
            assert_eq!((line, column), (5, 11), "{key}");
            std::fs::write(&path, &text).unwrap();
            assert_eq!(
                read(&path).unwrap_err(),
                format!(
                    "easystack {}:5:11: easyconfigs[0].x.eb.options[0]: duplicate entry with key \"{key}\"",
                    path.display()
                ),
                "{key}"
            );
        }
    }

    #[test]
    fn a_key_after_a_sequence_marker_points_at_the_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "k:\n  - '1e0': 1\n    '1e0': 2\n";
        assert_eq!(duplicate_at(text, "'1e0'"), (3, 5));
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:3:5: k[0]: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
    }

    #[test]
    fn a_quoted_block_key_keeps_an_internal_colon() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for key in ["\"a:b\"", "'a:b'"] {
            let text = format!("k:\n  {key}: 1\n  {key}: 2\n");
            assert_eq!(duplicate_at(&text, key), (3, 3), "{key}");
            std::fs::write(&path, &text).unwrap();
            assert_eq!(
                read(&path).unwrap_err(),
                format!(
                    "easystack {}:3:3: k: duplicate entry with key \"a:b\"",
                    path.display()
                ),
                "{key}"
            );
        }
    }

    #[test]
    fn a_quoted_multiline_scalar_is_not_scanned_for_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for quote in ['\'', '"'] {
            let text = format!("n: {quote}\n  k: 1\n  k: 2\n{quote}\nflags:\n  k: 1\n  k: 2\n");
            assert_eq!(duplicate_at(&text, "k:"), (7, 3), "{quote}");
            std::fs::write(&path, &text).unwrap();
            assert_eq!(
                read(&path).unwrap_err(),
                format!(
                    "easystack {}:7:3: flags: duplicate entry with key \"k\"",
                    path.display()
                ),
                "{quote}"
            );
        }
    }

    #[test]
    fn a_block_scalar_stops_at_the_owning_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "\
easyconfigs:
  - x.eb: |
      kept
    y.eb:
      options:
        rebuild: 1
        rebuild: 2
";
        assert_eq!(duplicate_at(text, "rebuild"), (7, 9));
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:7:9: easyconfigs[0].y.eb.options: duplicate entry with key \"rebuild\"",
                path.display()
            )
        );
    }

    #[test]
    fn an_ordinary_flow_repeat_wins_over_a_later_block_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "opts: {rebuild: a, rebuild: b}\nrebuild: 1\nrebuild: 2\n";
        let line_end = text.find('\n').expect("line");
        let at = text[..line_end].rfind("rebuild").expect("flow key");
        assert_eq!(position(text, at), (1, 20));
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:1:20: opts: duplicate entry with key \"rebuild\"",
                path.display()
            )
        );
    }

    #[test]
    fn an_anchor_or_a_tag_before_a_flow_map_points_at_the_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for (prop, column) in [("&o", 28), ("!!str", 31)] {
            let text =
                format!("easyconfigs:\n  - x.eb:\n      options: {prop} {{1e0: 1, 1e0: 2}}\n");
            assert_eq!(duplicate_at(&text, "1e0"), (3, column), "{prop}");
            std::fs::write(&path, &text).unwrap();
            assert_eq!(
                read(&path).unwrap_err(),
                format!(
                    "easystack {}:3:{column}: easyconfigs[0].x.eb.options: duplicate entry with key \"1e0\"",
                    path.display()
                ),
                "{prop}"
            );
        }
    }

    #[test]
    fn a_sequence_marker_or_an_explicit_key_points_at_the_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let plain = "k:\n  - 1e0: 1\n    1e0: 2\n";
        assert_eq!(duplicate_at(plain, "1e0"), (3, 5));
        std::fs::write(&path, plain).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:3:5: k[0]: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
        let explicit = "k:\n  - ? 1e0\n    : 1\n    ? 1e0\n    : 2\n";
        assert_eq!(duplicate_at(explicit, "1e0"), (4, 7));
        std::fs::write(&path, explicit).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:4:7: k[0]: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
    }

    #[test]
    fn an_explicit_flow_key_keeps_the_users_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "{? 1e0, ? 1e0}\n";
        assert_eq!(duplicate_at(text, "1e0"), (1, 11));
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:1:11: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
    }

    #[test]
    fn an_apostrophe_in_a_plain_scalar_does_not_hide_a_later_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let note = "\
easyconfigs:
  - x.eb:
      options:
        note: it's
        rebuild: 1
        rebuild: 2
";
        assert_eq!(duplicate_at(note, "rebuild"), (6, 9));
        std::fs::write(&path, note).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:6:9: easyconfigs[0].x.eb.options: duplicate entry with key \"rebuild\"",
                path.display()
            )
        );
        let name = "k:\n  O'Brien: 1\n  O'Brien: 2\n";
        assert_eq!(duplicate_at(name, "O'Brien"), (3, 3));
        std::fs::write(&path, name).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:3:3: k: duplicate entry with key \"O'Brien\"",
                path.display()
            )
        );
        let number = "note: it's\n1e0: 1\n1e0: 2\n";
        assert_eq!(duplicate_at(number, "1e0"), (3, 1));
        std::fs::write(&path, number).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:3:1: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
    }

    #[test]
    fn sequence_items_keep_separate_key_sets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let rebuild = "k:\n  - rebuild: 1\n  - other: 1\n    rebuild: 1\n    rebuild: 2\n";
        assert_eq!(duplicate_at(rebuild, "rebuild"), (5, 5));
        std::fs::write(&path, rebuild).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:5:5: k[1]: duplicate entry with key \"rebuild\"",
                path.display()
            )
        );
        let number = "k:\n  - 1e0: 1\n  - 1e0: 2\n    1e0: 3\n";
        assert_eq!(duplicate_at(number, "1e0"), (4, 5));
        std::fs::write(&path, number).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:4:5: k[1]: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
    }

    #[test]
    fn a_plain_scalar_with_an_internal_quote_does_not_swallow_the_next_map() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "note: say \"hi\nopts: {rebuild: 1, rebuild: 2}\n";
        assert_eq!(duplicate_at(text, "rebuild"), (2, 20));
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:2:20: opts: duplicate entry with key \"rebuild\"",
                path.display()
            )
        );
    }

    #[test]
    fn a_quoted_key_containing_colon_space_points_at_the_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "\"a: b\": 1\n\"a: b\": 2\n";
        assert_eq!(duplicate_at(text, "\"a: b\""), (2, 1));
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:2:1: duplicate entry with key \"a: b\"",
                path.display()
            )
        );
    }

    #[test]
    fn an_anchor_or_a_tag_before_a_key_points_at_the_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for prop in ["&a", "!!str"] {
            let text = format!("{prop} 1e0: 1\n{prop} 1e0: 2\n");
            let (line, column) = duplicate_at(&text, "1e0");
            assert_eq!(line, 2, "{prop}");
            std::fs::write(&path, &text).unwrap();
            assert_eq!(
                read(&path).unwrap_err(),
                format!(
                    "easystack {}:2:{column}: duplicate entry with key \"1e0\"",
                    path.display()
                ),
                "{prop}"
            );
        }
    }

    #[test]
    fn an_explicit_flow_key_with_an_internal_colon_points_at_the_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let quoted = "{? \"a:b\", ? \"a:b\"}\n";
        assert_eq!(duplicate_at(quoted, "\"a:b\""), (1, 13));
        std::fs::write(&path, quoted).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:1:13: duplicate entry with key \"a:b\"",
                path.display()
            )
        );
        let plain = "{? 1e0:80, ? 1e0:80}\n";
        assert_eq!(duplicate_at(plain, "1e0:80"), (1, 14));
        std::fs::write(&path, plain).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:1:14: duplicate entry with key \"1e0:80\"",
                path.display()
            )
        );
    }

    #[test]
    fn a_question_mark_inside_a_flow_plain_scalar_stays_in_the_value() {
        for (body, arg) in [
            ("{amend: http://host/q?1e0}", "--amend=http://host/q?1e0"),
            ("{note: foo?1e0}", "--note=foo?1e0"),
            ("{note: 1e0?x}", "--note=1e0?x"),
        ] {
            let text = format!("easyconfigs:\n  - x.eb:\n      options: {body}\n");
            let parsed = parse(&text).unwrap_or_else(|error| panic!("{body}: {error}"));
            assert_eq!(parsed[0].args, [arg], "{body}");
        }
    }

    #[test]
    fn a_multiline_plain_value_of_an_explicit_key_is_one_scalar() {
        let text = "\
easyconfigs:
  - x.eb:
      options:
        ? note
        : 1e0
          more
";
        assert_eq!(parse(text).unwrap()[0].args, ["--note=1e0 more"]);
    }

    #[test]
    fn an_aborted_flow_key_does_not_change_a_later_number() {
        let options = "\
easyconfigs:
  - x.eb:
      options:
        rebuild: 1e0
        parallel: 1.5e1
        mask: 0o20
        ratio: +.5
";
        let want = [
            "--mask=0o20",
            "--parallel=1.5e1",
            "--ratio=+.5",
            "--rebuild=1e0",
        ];
        let changed = ["--rebuild", "--parallel=15.0", "--mask=16", "--ratio=0.5"];
        for head in ["notes: {1e0:}\n", "notes: {1e0:, rebuild: 1e0}\n"] {
            let text = format!("{head}{options}");
            assert_spellings_or_error(&text, &want, &changed);
        }
        assert_spellings_or_error(
            "easyconfigs:\n  - x.eb:\n      options: {1e0:, rebuild: 1e0, parallel: 1.5e1, mask: 0o20, ratio: +.5}\n",
            &[
                "--1e0",
                "--mask=0o20",
                "--parallel=1.5e1",
                "--ratio=+.5",
                "--rebuild=1e0",
            ],
            &changed,
        );
        assert_spellings_or_error(
            "easyconfigs:\n  - x.eb:\n      options: {1e0:, mask: 0o20}\n",
            &["--1e0", "--mask=0o20"],
            &["--mask=16"],
        );
    }

    fn assert_spellings_or_error(text: &str, want: &[&str], changed: &[&str]) {
        match parse(text) {
            Ok(lines) => {
                assert_eq!(lines[0].args, want, "{text}");
                for arg in changed {
                    assert!(!lines[0].args.iter().any(|got| got == arg), "{text}");
                }
            }
            Err(error) => {
                for arg in changed {
                    assert!(!error.contains(arg), "{error}");
                }
            }
        }
    }

    #[test]
    fn a_property_on_the_previous_line_points_at_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for (text, needle) in [
            (
                "k:\n  ? &b\n    1e0\n  : 1\n  ? &b\n    1e0\n  : 2\n",
                "1e0",
            ),
            (
                "k:\n  ? !!str\n    1e0\n  : 1\n  ? !!str\n    1e0\n  : 2\n",
                "1e0",
            ),
            (
                "k:\n  ? !<tag:yaml.org,2002:str>\n    1e0\n  : 1\n  ? !<tag:yaml.org,2002:str>\n    1e0\n  : 2\n",
                "1e0",
            ),
            (
                "k:\n  ? &b # c\n    1e0\n  : 1\n  ? &b # c\n    1e0\n  : 2\n",
                "1e0",
            ),
            (
                "k:\n  ? &b\r\n    1e0\r\n  : 1\r\n  ? &b\r\n    1e0\r\n  : 2\r\n",
                "1e0",
            ),
            (
                "k:\n  ? &c\n    \"1e0\"\n  : 1\n  ? &c\n    \"1e0\"\n  : 2\n",
                "1e0",
            ),
            (
                "k:\n  ? &c \"1e0\"\n  : 1\n  ? &c \"1e0\"\n  : 2\n",
                "\"1e0\"",
            ),
            (
                "k:\n  ? &b # café\n    1e0\n  : 1\n  ? &b # café\n    1e0\n  : 2\n",
                "1e0",
            ),
            (
                "k:\n  ? !!str # café\n    1e0\n  : 1\n  ? !!str # café\n    1e0\n  : 2\n",
                "1e0",
            ),
            (
                "k:\n  ? &b # c👍\n    1e0\n  : 1\n  ? &b # c👍\n    1e0\n  : 2\n",
                "1e0",
            ),
            (
                "k:\n  ? &b\t# café\n    1e0\n  : 1\n  ? &b\t# café\n    1e0\n  : 2\n",
                "1e0",
            ),
            (
                "k:\n  ? &b # café\n    rebuild\n  : 1\n  ? &b # café\n    rebuild\n  : 2\n",
                "rebuild",
            ),
            (
                "k:\n  ? !!str # café\n    rebuild\n  : 1\n  ? !!str # café\n    rebuild\n  : 2\n",
                "rebuild",
            ),
            (
                "k:\n  ? &b # c👍\n    rebuild\n  : 1\n  ? &b # c👍\n    rebuild\n  : 2\n",
                "rebuild",
            ),
            (
                "k:\n  ? &b\t# c👍\n    rebuild\n  : 1\n  ? &b\t# c👍\n    rebuild\n  : 2\n",
                "rebuild",
            ),
        ] {
            let (line, column) = duplicate_at(text, needle);
            let key = if needle == "rebuild" { needle } else { "1e0" };
            std::fs::write(&path, text).unwrap();
            assert_eq!(
                read(&path).unwrap_err(),
                format!(
                    "easystack {}:{}:{}: k: duplicate entry with key \"{key}\"",
                    path.display(),
                    line,
                    column
                ),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_duplicate_after_an_aborted_plain_key_is_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for (text, key) in [
            ("notes: {1e0:}\n1e0: 1\n1e0: 2\n", "1e0"),
            ("notes: {1e0:}\nrebuild: 1\nrebuild: 2\n", "rebuild"),
        ] {
            let (line, column) = duplicate_at(text, key);
            assert_eq!((line, column), (3, 1), "{key}");
            std::fs::write(&path, text).unwrap();
            assert_eq!(
                read(&path).unwrap_err(),
                format!(
                    "easystack {}:3:1: duplicate entry with key \"{key}\"",
                    path.display()
                ),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_comment_or_an_escape_is_not_the_duplicate_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let block = "k:\n  ? &b |- # 1e0\n    1e0\n  : 1\n  ? &b |- # 1e0\n    1e0\n  : 2\n";
        assert_eq!(duplicate_at(block, "1e0"), (6, 5));
        std::fs::write(&path, block).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:6:5: k: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
        let escaped = "k:\n  ? &c\n    \"\\x31e0\"\n  : 1\n  ? &c\n    \"\\x31e0\"\n  : 2\n";
        let quote = escaped.rfind("\"\\x31e0\"").expect("opening quote");
        assert_eq!(position(escaped, quote), (6, 5));
        std::fs::write(&path, escaped).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {}:6:5: k: duplicate entry with key \"1e0\"",
                path.display()
            )
        );
    }

    #[test]
    fn a_non_string_tag_keeps_the_key_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for tag in ["!", "!!float"] {
            let text = format!("k:\n  ? {tag}\n    1e0\n  : 1\n  ? {tag}\n    1e0\n  : 2\n");
            assert_eq!(duplicate_at(&text, "1e0"), (6, 5), "{tag}");
            std::fs::write(&path, &text).unwrap();
            assert_eq!(
                read(&path).unwrap_err(),
                format!(
                    "easystack {}:6:5: k: duplicate entry with key \"1e0\"",
                    path.display()
                ),
                "{tag}"
            );
        }
    }

    #[test]
    fn a_unicode_break_after_a_property_points_at_the_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for gap in ['\u{0085}', '\u{2028}', '\u{2029}'] {
            let text =
                format!("k:\n  ? &c{gap}    \"1e0\"\n  : 1\n  ? &c{gap}    \"1e0\"\n  : 2\n");
            let quote = text.rfind("\"1e0\"").expect("quoted key");
            let (quote_line, quote_column) = advance_mark(&text, 0, 0, 0, quote);
            assert_eq!((quote_line, quote_column), (6, 5), "{gap:?}");
            std::fs::write(&path, &text).unwrap();
            assert_eq!(
                read(&path).unwrap_err(),
                format!(
                    "easystack {}:6:6: k: duplicate entry with key \"1e0\"",
                    path.display()
                ),
                "{gap:?}"
            );
        }
    }

    #[test]
    fn an_anchor_before_an_explicit_key_keeps_the_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "\
easyconfigs:
  - x.eb:
      options:
        ? &ref 1e0
        : true
        ? &b 1e0
        : false
";
        let (line, column) = duplicate_at(text, "1e0");
        assert_eq!((line, column), (6, 14));
        std::fs::write(&path, text).unwrap();
        assert_eq!(
            read(&path).unwrap_err(),
            format!(
                "easystack {path}:{line}:{column}: easyconfigs[0].x.eb.options: duplicate entry with key \"1e0\"",
                path = path.display()
            )
        );
    }

    fn expect_read(path: &std::path::Path, text: &str, line: usize, column: usize, detail: &str) {
        std::fs::write(path, text).unwrap();
        assert_eq!(
            read(path).unwrap_err(),
            format!("easystack {}:{}:{}: {detail}", path.display(), line, column),
            "{text:?}"
        );
    }

    /// A break after an anchor or a tag is not part of the name, and the `#`
    /// that follows it is a comment. The repeated key is the next line.
    #[test]
    fn a_unicode_break_after_an_anchor_or_a_tag_is_not_a_comment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for (prop, gap, comment, key) in [
            ("&b", '\u{0085}', "# 1e0", "1e0"),
            ("!!str", '\u{2028}', "# café", "rebuild"),
            ("!!float", '\u{2028}', "# café", "1e0"),
            ("!", '\u{2028}', "# café", "1e0"),
        ] {
            let text = format!(
                "k:\n  ? {prop}{gap}{comment}\n    {key}\n  : 1\n  ? {prop}{gap}{comment}\n    {key}\n  : 2\n"
            );
            let at = text.rfind(key).expect(key);
            assert_eq!(advance_mark(&text, 0, 0, 0, at), (8, 5), "{text:?}");
            expect_read(
                &path,
                &text,
                8,
                5,
                &format!("k: duplicate entry with key \"{key}\""),
            );
        }
        let flow = "{? &c\u{2028}1e0: 1, ? &c\u{2028}1e0: 2}";
        let at = flow.rfind("1e0").expect("second key");
        assert_eq!(advance_mark(flow, 0, 0, 0, at), (3, 1));
        expect_read(&path, flow, 3, 1, "duplicate entry with key \"1e0\"");
    }

    /// `{1e0:}` stops the original scan. The quoted rewrite reads further, and
    /// that later error is the one to report.
    #[test]
    fn a_later_parse_error_is_not_hidden_by_an_aborted_plain_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let float_key = "notes: {1e0:}\n? !!float\n  1e0\n: 1\n? !!float\n  1e0\n: 2\n";
        let at = float_key.rfind("1e0").expect("second key");
        assert_eq!(position(float_key, at), (6, 3));
        expect_read(&path, float_key, 6, 3, "duplicate entry with key \"1e0\"");
        let bang = "notes: {1e0:}\n? !\n  1e0\n: 1\n? !\n  1e0\n: 2\n";
        let at = bang.rfind("1e0").expect("second key");
        assert_eq!(position(bang, at), (6, 3));
        expect_read(&path, bang, 6, 3, "duplicate entry with key \"1e0\"");
        expect_read(
            &path,
            "notes: {1e0:}\neasyconfigs: [\n",
            3,
            1,
            "did not find expected node content at line 3 column 1, while parsing a flow node",
        );
        expect_read(
            &path,
            "notes: {1e0:}\nfoo: bar\n  options: true\n",
            3,
            10,
            "mapping values are not allowed in this context",
        );
    }

    /// An earlier repeated collection is the duplicate. A later `!` scalar is
    /// named only when that scalar is the repeated node.
    #[test]
    fn an_earlier_collection_key_is_the_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for key in ["[a]", "{}", "{a: 1}"] {
            let text =
                format!("k:\n  ? {key}\n  : 1\n  ? {key}\n  : 2\n  ? ! z\n  : 3\n  ? ! z\n  : 4\n");
            let at = text.rfind(key).unwrap_or_else(|| panic!("missing {key}"));
            let (line, column) = position(&text, at);
            assert_eq!((line, column), (4, 5), "{key}");
            let wrong = text.rfind('z').expect("later tag");
            assert_eq!(position(&text, wrong), (8, 7), "{key}");
            expect_read(&path, &text, 4, 5, "k: duplicate entry in YAML map");
        }
    }

    /// A tagged key is named from the bytes libyaml gives, at that key's mark.
    #[test]
    fn a_tagged_or_non_finite_key_keeps_its_source_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        for (node, spelling, detail) in [
            (
                "!!float .Inf",
                ".Inf",
                "k: duplicate entry with key \".Inf\"",
            ),
            (
                "!!float +.inf",
                "+.inf",
                "k: duplicate entry with key \"+.inf\"",
            ),
            (
                "!!float -.inf",
                "-.inf",
                "k: duplicate entry with key -.inf",
            ),
            ("!!float .nan", ".nan", "k: duplicate entry with key .nan"),
            ("!foo", "!foo", "k: duplicate entry with key \"!foo\""),
            ("!!int 0x10", "0x10", "k: duplicate entry with key \"0x10\""),
        ] {
            let text = format!("k:\n  ? {node}\n  : 1\n  ? {node}\n  : 2\n");
            let at = text.rfind(spelling).expect(spelling);
            let (line, column) = position(&text, at);
            assert_eq!(line, 4, "{node}");
            expect_read(&path, &text, line, column, detail);
        }
    }

    /// A `#` line under a block header is body text. Only the header line holds
    /// a comment.
    #[test]
    fn a_block_body_line_is_not_a_comment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text =
            "k:\n  ? &b |-\n    # not-a-comment\n  : 1\n  ? &b |-\n    # not-a-comment\n  : 2\n";
        let at = text.rfind("# not-a-comment").expect("body");
        assert_eq!(position(text, at), (6, 5));
        expect_read(
            &path,
            text,
            6,
            5,
            "k: duplicate entry with key \"# not-a-comment\"",
        );
    }

    /// Every duplicate fixture from the quoting and location fixes, checked
    /// again through `read`.
    #[test]
    fn round_fixtures_keep_their_message_and_location() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let rows: &[(&str, usize, usize, &str)] = &[
            (
                "{? &c\u{2028}1e0: 1, ? &c\u{2028}1e0: 2}",
                3,
                1,
                "duplicate entry with key \"1e0\"",
            ),
            (
                "k:\n  ? [a]\n  : 1\n  ? [a]\n  : 2\n  ? ! z\n  : 3\n  ? ! z\n  : 4\n",
                4,
                5,
                "k: duplicate entry in YAML map",
            ),
            (
                "k:\n  ? {}\n  : 1\n  ? {}\n  : 2\n  ? ! z\n  : 3\n  ? ! z\n  : 4\n",
                4,
                5,
                "k: duplicate entry in YAML map",
            ),
            (
                "k:\n  ? {a: 1}\n  : 1\n  ? {a: 1}\n  : 2\n  ? ! z\n  : 3\n  ? ! z\n  : 4\n",
                4,
                5,
                "k: duplicate entry in YAML map",
            ),
            (
                "k:\n  ? !!float .Inf\n  : 1\n  ? !!float .Inf\n  : 2\n",
                4,
                13,
                "k: duplicate entry with key \".Inf\"",
            ),
            (
                "k:\n  ? !!float +.inf\n  : 1\n  ? !!float +.inf\n  : 2\n",
                4,
                13,
                "k: duplicate entry with key \"+.inf\"",
            ),
            (
                "k:\n  ? !!float -.inf\n  : 1\n  ? !!float -.inf\n  : 2\n",
                4,
                13,
                "k: duplicate entry with key -.inf",
            ),
            (
                "k:\n  ? !!float .nan\n  : 1\n  ? !!float .nan\n  : 2\n",
                4,
                13,
                "k: duplicate entry with key .nan",
            ),
            (
                "k:\n  ? !foo\n  : 1\n  ? !foo\n  : 2\n",
                4,
                5,
                "k: duplicate entry with key \"!foo\"",
            ),
            (
                "k:\n  ? !!int 0x10\n  : 1\n  ? !!int 0x10\n  : 2\n",
                4,
                11,
                "k: duplicate entry with key \"0x10\"",
            ),
            (
                "k:\n  ? &b |-\n    # not-a-comment\n  : 1\n  ? &b |-\n    # not-a-comment\n  : 2\n",
                6,
                5,
                "k: duplicate entry with key \"# not-a-comment\"",
            ),
            (
                "notes: {1e0:}\n? !!float\n  1e0\n: 1\n? !!float\n  1e0\n: 2\n",
                6,
                3,
                "duplicate entry with key \"1e0\"",
            ),
            (
                "notes: {1e0:}\n? !\n  1e0\n: 1\n? !\n  1e0\n: 2\n",
                6,
                3,
                "duplicate entry with key \"1e0\"",
            ),
            (
                "notes: {1e0:}\neasyconfigs: [\n",
                3,
                1,
                "did not find expected node content at line 3 column 1, while parsing a flow node",
            ),
            (
                "notes: {1e0:}\nfoo: bar\n  options: true\n",
                3,
                10,
                "mapping values are not allowed in this context",
            ),
            (
                "notes: {1e0:}\n1e0: 1\n1e0: 2\n",
                3,
                1,
                "duplicate entry with key \"1e0\"",
            ),
            (
                "notes: {1e0:}\nrebuild: 1\nrebuild: 2\n",
                3,
                1,
                "duplicate entry with key \"rebuild\"",
            ),
            (
                "k:\n  ? &b |- # 1e0\n    1e0\n  : 1\n  ? &b |- # 1e0\n    1e0\n  : 2\n",
                6,
                5,
                "k: duplicate entry with key \"1e0\"",
            ),
            (
                "k:\n  'it''s': 1\n  'it''s': 2\n",
                3,
                3,
                "k: duplicate entry with key \"it's\"",
            ),
            (
                "k:\n  \"a\\\"b\": 1\n  \"a\\\"b\": 2\n",
                3,
                3,
                "k: duplicate entry with key \"a\\\"b\"",
            ),
            (
                "opts: {\"a\\\"b\": 1, \"a\\\"b\": 2}\n",
                1,
                19,
                "opts: duplicate entry with key \"a\\\"b\"",
            ),
            (
                "{\"1e0\":1,\"1e0\":2}\n",
                1,
                10,
                "duplicate entry with key \"1e0\"",
            ),
            (
                "{'1e0':1,'1e0':2}\n",
                1,
                10,
                "duplicate entry with key \"1e0\"",
            ),
            (
                "{\"a\":{\"1e0\":1,\"1e0\":2}}\n",
                1,
                15,
                "a: duplicate entry with key \"1e0\"",
            ),
            (
                "{? 1e0, ? 1e0}\n",
                1,
                11,
                "duplicate entry with key \"1e0\"",
            ),
            (
                "{? \"a:b\", ? \"a:b\"}\n",
                1,
                13,
                "duplicate entry with key \"a:b\"",
            ),
            (
                "{? 1e0:80, ? 1e0:80}\n",
                1,
                14,
                "duplicate entry with key \"1e0:80\"",
            ),
            (
                "\"a: b\": 1\n\"a: b\": 2\n",
                2,
                1,
                "duplicate entry with key \"a: b\"",
            ),
            (
                "opts: {rebuild: a, rebuild: b}\nrebuild: 1\nrebuild: 2\n",
                1,
                20,
                "opts: duplicate entry with key \"rebuild\"",
            ),
            (
                "opts: {rebuild: 1e0, flags: {1e0: a, '1e0': b}}\n",
                1,
                38,
                "opts.flags: duplicate entry with key \"1e0\"",
            ),
            (
                "opts: {1e0: a, '1e0': b}\n1e0: 1\n1e0: 2\n",
                1,
                16,
                "opts: duplicate entry with key \"1e0\"",
            ),
            (
                "note: say \"hi\nopts: {rebuild: 1, rebuild: 2}\n",
                2,
                20,
                "opts: duplicate entry with key \"rebuild\"",
            ),
            (
                "k:\n  - rebuild: 1\n  - other: 1\n    rebuild: 1\n    rebuild: 2\n",
                5,
                5,
                "k[1]: duplicate entry with key \"rebuild\"",
            ),
            (
                "k:\n  - 1e0: 1\n  - 1e0: 2\n    1e0: 3\n",
                4,
                5,
                "k[1]: duplicate entry with key \"1e0\"",
            ),
            (
                "k:\n  - '1e0': 1\n    '1e0': 2\n",
                3,
                5,
                "k[0]: duplicate entry with key \"1e0\"",
            ),
            (
                "k:\n  - 1e0: 1\n    1e0: 2\n",
                3,
                5,
                "k[0]: duplicate entry with key \"1e0\"",
            ),
            (
                "k:\n  - ? 1e0\n    : 1\n    ? 1e0\n    : 2\n",
                4,
                7,
                "k[0]: duplicate entry with key \"1e0\"",
            ),
            (
                "k:\n  O'Brien: 1\n  O'Brien: 2\n",
                3,
                3,
                "k: duplicate entry with key \"O'Brien\"",
            ),
            (
                "note: it's\n1e0: 1\n1e0: 2\n",
                3,
                1,
                "duplicate entry with key \"1e0\"",
            ),
            (
                "k:\n  \"a:b\": 1\n  \"a:b\": 2\n",
                3,
                3,
                "k: duplicate entry with key \"a:b\"",
            ),
            (
                "k:\n  'a:b': 1\n  'a:b': 2\n",
                3,
                3,
                "k: duplicate entry with key \"a:b\"",
            ),
            (
                "k:\n  ? &b # \"decoy\"\n    \"rebuild\"\n  : 1\n  ? &b # \"decoy\"\n    \"rebuild\"\n  : 2\n",
                6,
                6,
                "k: duplicate entry with key \"rebuild\"",
            ),
            (
                "k:\n  ? &b # it's\n    'it''s'\n  : 1\n  ? &b # it's\n    'it''s'\n  : 2\n",
                6,
                5,
                "k: duplicate entry with key \"it's\"",
            ),
            (
                "k:\n  ? &b # > decoy\n    |-\n      key\n  : 1\n  ? &b # > decoy\n    |-\n      key\n  : 2\n",
                8,
                7,
                "k: duplicate entry with key \"key\"",
            ),
            (
                "k:\n  ? &b # owner's \"primary\"\n    \"rebuild\"\n  : 1\n  ? &b # owner's \"primary\"\n    \"rebuild\"\n  : 2\n",
                6,
                6,
                "k: duplicate entry with key \"rebuild\"",
            ),
            (
                "k:\n  ? &b # use | literal\n    |-\n      key\n  : 1\n  ? &b # use | literal\n    |-\n      key\n  : 2\n",
                8,
                7,
                "k: duplicate entry with key \"key\"",
            ),
            (
                "k:\n  ? !<tag:example> |-\n    key\n  : 1\n  ? !<tag:example> |-\n    key\n  : 2\n",
                6,
                5,
                "k: duplicate entry with key \"key\"",
            ),
            (
                "? &b # c\n  !foo\n: 1\n? &b # c\n  !foo\n: 2\n",
                5,
                3,
                "duplicate entry with key \"!foo\"",
            ),
            (
                "? &b !foo\n: 1\n? &b !foo\n: 2\n",
                3,
                6,
                "duplicate entry with key \"!foo\"",
            ),
            (
                "k:\n  ? &b\n    1e0\n    more\n  : 1\n  ? &b\n    1e0\n    more\n  : 2\n",
                7,
                5,
                "k: duplicate entry with key \"1e0 more\"",
            ),
            (
                "k:\n  ? &a 1e0\n  : 1\n  ? *a\n  : 2\n  ? z\n  : 3\n  ? z\n  : 4\n",
                4,
                5,
                "k: duplicate entry with key \"*a\"",
            ),
            (
                "a: &A [1]\n? *A\n? *A\n: 1\n# pad\n? ! z\n: 1\n? ! z\n",
                3,
                3,
                "duplicate entry in YAML map",
            ),
            (
                "a: 1\na: 2\nbad: [\n",
                2,
                1,
                "duplicate entry with key \"a\"",
            ),
            (
                "easyconfigs:\n  - x.eb:\n      options:\n        1e0: 1\n        1e0: 2\nbad: [\n",
                5,
                9,
                "easyconfigs[0].x.eb.options: duplicate entry with key \"1e0\"",
            ),
            (
                "{!!map {a: 1}: 1, {a: 1}: 2, z: 1, z: 2}\n",
                1,
                19,
                "duplicate entry in YAML map",
            ),
            (
                "k:\n  ? {a: 1}\n  : 1\n  ? !!map {a: 1}\n  : 2\n  z: 1\n  z: 2\n",
                4,
                11,
                "k: duplicate entry in YAML map",
            ),
            (
                "k:\n  ? [1]\n  : 1\n  ? !!seq [1]\n  : 2\n  z: 1\n  z: 2\n",
                4,
                11,
                "k: duplicate entry in YAML map",
            ),
            (
                "k:\n  ? {a: 1}\n  : 1\n  ? !!str {a: 1}\n  : 2\n  z: 1\n  z: 2\n",
                4,
                11,
                "k: duplicate entry in YAML map",
            ),
            (
                "k:\n  ? !!omap\n    - a: 1\n  : 1\n  ? !!omap\n    - a: 1\n  : 2\n  z: 1\n  z: 2\n",
                6,
                5,
                "k: duplicate entry in YAML map",
            ),
            (
                "k:\n  ? {a: 1}\n  : 1\n  ? !<tag:yaml.org,2002:map> {a: 1}\n  : 2\n  z: 1\n  z: 2\n",
                4,
                30,
                "k: duplicate entry in YAML map",
            ),
            (
                "k: {? !<tag:example.com:foo> {a: 1}: 1, ? {a: 1}: 2, z: 1, z: 2}\n",
                1,
                43,
                "k: duplicate entry in YAML map",
            ),
            (
                "a: &A !!map {a: 1}\nk: {? {a: 1}: 1, ? *A: 2, z: 1, z: 2}\n",
                2,
                20,
                "k: duplicate entry in YAML map",
            ),
            (
                "k:\n  ?\n  ?\n  : 1\n",
                3,
                3,
                "k: duplicate entry with null key",
            ),
            (
                "k:\n  1e0: 1\n  ?\n  ?\n  : 1\n",
                4,
                3,
                "k: duplicate entry with null key",
            ),
            (
                "x: {1e0:, café: [}\n",
                1,
                18,
                "did not find expected node content at line 1 column 18, while parsing a flow node",
            ),
        ];
        for (text, line, column, detail) in rows {
            expect_read(&path, text, *line, *column, detail);
        }
        for (prop, comment, key) in [
            ("&b", "# 1e0", "1e0"),
            ("!!str", "# café", "rebuild"),
            ("!!float", "# café", "1e0"),
            ("!", "# café", "1e0"),
        ] {
            for gap in ['\u{0085}', '\u{2028}', '\u{2029}'] {
                let text = format!(
                    "k:\n  ? {prop}{gap}{comment}\n    {key}\n  : 1\n  ? {prop}{gap}{comment}\n    {key}\n  : 2\n"
                );
                expect_read(
                    &path,
                    &text,
                    8,
                    5,
                    &format!("k: duplicate entry with key \"{key}\""),
                );
            }
        }
        for key in ["1e0", "0o20", "+.5", "1.5e1"] {
            let text = format!(
                "easyconfigs:\n  - x.eb:\n      options:\n        {key}: 1\n        {key}: 2\n"
            );
            expect_read(
                &path,
                &text,
                5,
                9,
                &format!("easyconfigs[0].x.eb.options: duplicate entry with key \"{key}\""),
            );
        }
        let explicit = "\
easyconfigs:
  - x.eb:
      options:
        ? 1e0
        : 1
        ? 1e0
        : 2
";
        expect_read(
            &path,
            explicit,
            6,
            11,
            "easyconfigs[0].x.eb.options: duplicate entry with key \"1e0\"",
        );
        let anchored = "\
easyconfigs:
  - x.eb:
      options:
        ? &ref 1e0
        : true
        ? &b 1e0
        : false
";
        expect_read(
            &path,
            anchored,
            6,
            14,
            "easyconfigs[0].x.eb.options: duplicate entry with key \"1e0\"",
        );
    }

    /// A quote, apostrophe, `|` or `>` inside a `#` comment is not the key.
    #[test]
    fn a_comment_is_not_the_key_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        // The opening quote of "rebuild" is at 6:5. The spelling starts at 6:6,
        // the same column as a next-line "1e0" whose anchor is on the previous line.
        let decoy = "\
k:
  ? &b # \"decoy\"
    \"rebuild\"
  : 1
  ? &b # \"decoy\"
    \"rebuild\"
  : 2
";
        let quote = decoy.rfind("\"rebuild\"").expect("quote");
        assert_eq!(advance_mark(decoy, 0, 0, 0, quote), (6, 5));
        let comment_quote = decoy.rfind("# \"decoy\"").expect("comment") + 2;
        assert_eq!(advance_mark(decoy, 0, 0, 0, comment_quote), (5, 10));
        expect_read(
            &path,
            decoy,
            6,
            6,
            "k: duplicate entry with key \"rebuild\"",
        );

        // The raw body it''s is not the value it's, so the key is the quote at 6:5.
        let apostrophe = "\
k:
  ? &b # it's
    'it''s'
  : 1
  ? &b # it's
    'it''s'
  : 2
";
        let tick = apostrophe.rfind("'it''s'").expect("tick");
        assert_eq!(advance_mark(apostrophe, 0, 0, 0, tick), (6, 5));
        expect_read(
            &path,
            apostrophe,
            6,
            5,
            "k: duplicate entry with key \"it's\"",
        );

        let angle = "\
k:
  ? &b # > decoy
    |-
      key
  : 1
  ? &b # > decoy
    |-
      key
  : 2
";
        let content = angle.rfind("key").expect("content");
        assert_eq!(advance_mark(angle, 0, 0, 0, content), (8, 7));
        expect_read(&path, angle, 8, 7, "k: duplicate entry with key \"key\"");

        // The comment quote sits at 5:18 and the p of primary at 5:19.
        let owner = "\
k:
  ? &b # owner's \"primary\"
    \"rebuild\"
  : 1
  ? &b # owner's \"primary\"
    \"rebuild\"
  : 2
";
        let primary = owner.rfind("\"primary\"").expect("primary");
        assert_eq!(advance_mark(owner, 0, 0, 0, primary), (5, 18));
        assert_eq!(advance_mark(owner, 0, 0, 0, primary + 1), (5, 19));
        let owner_key = owner.rfind("\"rebuild\"").expect("key");
        assert_eq!(advance_mark(owner, 0, 0, 0, owner_key), (6, 5));
        expect_read(
            &path,
            owner,
            6,
            6,
            "k: duplicate entry with key \"rebuild\"",
        );

        let literal = "\
k:
  ? &b # use | literal
    |-
      key
  : 1
  ? &b # use | literal
    |-
      key
  : 2
";
        let bar = literal.rfind("| literal").expect("bar");
        assert_eq!(advance_mark(literal, 0, 0, 0, bar), (6, 14));
        let ell = literal.rfind("literal").expect("ell");
        assert_eq!(advance_mark(literal, 0, 0, 0, ell), (6, 16));
        let body = literal.rfind("key").expect("body");
        assert_eq!(advance_mark(literal, 0, 0, 0, body), (8, 7));
        expect_read(&path, literal, 8, 7, "k: duplicate entry with key \"key\"");
    }

    /// `!<tag:example>` contains `>`. The key is the block content, not that `>`.
    #[test]
    fn a_verbatim_tag_is_not_the_block_indicator() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "\
k:
  ? !<tag:example> |-
    key
  : 1
  ? !<tag:example> |-
    key
  : 2
";
        let indicator = text.rfind("|-").expect("indicator");
        assert_eq!(advance_mark(text, 0, 0, 0, indicator), (5, 20));
        expect_read(&path, text, 6, 5, "k: duplicate entry with key \"key\"");
    }

    /// An empty tagged key is the tag token, not the anchor and the comment.
    #[test]
    fn an_empty_tagged_key_is_named_from_its_tag() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let commented = "\
? &b # c
  !foo
: 1
? &b # c
  !foo
: 2
";
        let anchor = commented.rfind("&b").expect("anchor");
        assert_eq!(advance_mark(commented, 0, 0, 0, anchor), (4, 3));
        expect_read(&path, commented, 5, 3, "duplicate entry with key \"!foo\"");
        let inline = "\
? &b !foo
: 1
? &b !foo
: 2
";
        let bang = inline.rfind("&b").expect("anchor");
        assert_eq!(advance_mark(inline, 0, 0, 0, bang), (3, 3));
        expect_read(&path, inline, 3, 6, "duplicate entry with key \"!foo\"");
    }

    /// A folded plain key starts at its scalar, not at the anchor on the line above.
    #[test]
    fn a_folded_plain_key_points_at_its_scalar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "\
k:
  ? &b
    1e0
    more
  : 1
  ? &b
    1e0
    more
  : 2
";
        let anchor = text.rfind("&b").expect("anchor");
        assert_eq!(advance_mark(text, 0, 0, 0, anchor), (6, 5));
        expect_read(
            &path,
            text,
            7,
            5,
            "k: duplicate entry with key \"1e0 more\"",
        );
    }

    /// An alias is the first repeat serde would find, at the alias token.
    #[test]
    fn an_alias_key_is_the_earliest_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let number = "\
k:
  ? &a 1e0
  : 1
  ? *a
  : 2
  ? z
  : 3
  ? z
  : 4
";
        let zed = number.rfind('z').expect("later key");
        assert_eq!(advance_mark(number, 0, 0, 0, zed), (8, 5));
        expect_read(&path, number, 4, 5, "k: duplicate entry with key \"*a\"");

        // The second *A is line 3. The later ! z is line 8.
        let collection = "\
a: &A [1]
? *A
? *A
: 1
# pad
? ! z
: 1
? ! z
";
        let star = collection.rfind("*A").expect("alias");
        assert_eq!(advance_mark(collection, 0, 0, 0, star), (3, 3));
        let later = collection.rfind('z').expect("later");
        assert_eq!(advance_mark(collection, 0, 0, 0, later), (8, 5));
        expect_read(&path, collection, 3, 3, "duplicate entry in YAML map");
    }

    /// The original parse says `key 1.0` and the quoted parse says `key "*a"`.
    /// Both marks are the alias. The tie keeps the quoted spelling.
    #[test]
    fn a_column_tie_keeps_the_quoted_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "\
k:
  ? &a 1e0
  : 1
  ? *a
  : 2
  ? z
  : 3
  ? z
  : 4
";
        let quoted = quote_scalars_easybuild_keeps_as_text(text);
        let original = yaml_diagnostic(text, serde_yaml::from_str::<Value>(text).unwrap_err());
        let rewritten =
            yaml_diagnostic(&quoted, serde_yaml::from_str::<Value>(&quoted).unwrap_err());
        let original_at = original.location.expect("original column");
        let rewritten_at = rewritten.location.expect("quoted column");
        let mapped = advance_mark(
            text,
            0,
            0,
            0,
            map_rewritten_index(
                text,
                &quoted,
                location_index(&quoted, rewritten_at.0, rewritten_at.1),
            ),
        );
        assert_eq!(original_at, (4, 5));
        assert_eq!(mapped, original_at);
        assert_eq!(original.message, "k: duplicate entry with key 1.0");
        assert_eq!(rewritten.message, "k: duplicate entry with key \"*a\"");
        expect_read(&path, text, 4, 5, "k: duplicate entry with key \"*a\"");
    }

    /// The mapped column is written into the message, not only into the location.
    #[test]
    fn a_rewritten_column_is_written_into_the_message() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "x: {1e0:, café: [}\n";
        expect_read(
            &path,
            text,
            1,
            18,
            "did not find expected node content at line 1 column 18, while parsing a flow node",
        );
    }

    /// Both the problem mark and the context mark are columns of the original text.
    #[test]
    fn a_flow_sequence_maps_both_marks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "x: {1e0:, more: [1, 2}";
        let bracket = text.rfind('[').expect("sequence");
        let end = text.rfind('}').expect("closer");
        assert_eq!(advance_mark(text, 0, 0, 0, bracket), (1, 17));
        assert_eq!(advance_mark(text, 0, 0, 0, end), (1, 22));
        expect_read(
            &path,
            text,
            1,
            22,
            "did not find expected ',' or ']' at line 1 column 22, while parsing a flow sequence at line 1 column 17",
        );
    }

    /// The context is the plain scalar, two columns before the quoted scan.
    #[test]
    fn a_plain_scalar_context_mark_is_mapped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "{1e0:, café:}";
        let cafe = text.find('c').expect("scalar");
        let colon = text.rfind(':').expect("colon");
        assert_eq!(advance_mark(text, 0, 0, 0, cafe), (1, 8));
        assert_eq!(advance_mark(text, 0, 0, 0, colon), (1, 12));
        expect_read(
            &path,
            text,
            1,
            12,
            "found unexpected ':' at line 1 column 12, while scanning a plain scalar at line 1 column 8",
        );
    }

    /// `{1e0:}` and a later `1e0` are both quoted. The alias mark moves back by both.
    #[test]
    fn an_alias_context_mark_is_mapped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "notes: {1e0:}\nm: {1e0: 1, *}\n";
        let star = text.rfind('*').expect("alias");
        assert_eq!(advance_mark(text, 0, 0, 0, star), (2, 13));
        assert_eq!(advance_mark(text, 0, 0, 0, star + 1), (2, 14));
        expect_read(
            &path,
            text,
            2,
            14,
            "did not find expected alphabetic or numeric character at line 2 column 14, while scanning an alias at line 2 column 13",
        );
    }

    /// The problem sits on the next line, so its column does not move. The context does.
    #[test]
    fn an_unchanged_problem_column_still_maps_the_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "x: {1e0:, \"unterminated\n";
        let quote = text.find('"').expect("quote");
        assert_eq!(advance_mark(text, 0, 0, 0, quote), (1, 11));
        expect_read(
            &path,
            text,
            2,
            1,
            "found unexpected end of stream at line 2 column 1, while scanning a quoted scalar at line 1 column 11",
        );
    }

    /// `B` is a collection whose value aliases `A`. The second `*B` is the duplicate, not `z`.
    #[test]
    fn an_anchored_collection_with_an_outer_alias_is_the_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let map = "\
a: &A [1]
b: &B {x: *A}
k: {? *B: 1, ? *B: 2, z: 1, z: 2}
";
        let star = map.rfind("*B").expect("second alias");
        let zed = map.rfind('z').expect("later key");
        assert_eq!(advance_mark(map, 0, 0, 0, star), (3, 16));
        assert_eq!(advance_mark(map, 0, 0, 0, zed), (3, 29));
        expect_read(&path, map, 3, 16, "k: duplicate entry in YAML map");

        let seq = "\
a: &A [1]
b: &B [*A]
k: {? *B: 1, ? *B: 2, z: 1, z: 2}
";
        let star = seq.rfind("*B").expect("second alias");
        assert_eq!(advance_mark(seq, 0, 0, 0, star), (3, 16));
        expect_read(&path, seq, 3, 16, "k: duplicate entry in YAML map");
    }

    /// A block collection that aliases an outer anchor is stored. The later `rebuild` is not the error.
    #[test]
    fn a_block_anchor_with_an_outer_alias_is_the_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "\
a: &A [1]
b: &B
  x: *A
k:
  ? *B
  : 1
  ? *B
  : 2
  rebuild: 1
  rebuild: 2
";
        let star = text.rfind("*B").expect("second alias");
        let later = text.rfind("rebuild").expect("later key");
        assert_eq!(advance_mark(text, 0, 0, 0, star), (7, 5));
        assert_eq!(advance_mark(text, 0, 0, 0, later), (10, 3));
        expect_read(&path, text, 7, 5, "k: duplicate entry in YAML map");
    }

    /// `{?,?}` has no scalar token. The second empty key is the `?` before `}`.
    #[test]
    fn an_empty_flow_key_points_at_the_question_mark() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "{?,?}";
        let mark = text.rfind('?').expect("second key");
        let end = text.rfind('}').expect("closer");
        assert_eq!(advance_mark(text, 0, 0, 0, mark), (1, 4));
        assert_eq!(advance_mark(text, 0, 0, 0, end), (1, 5));
        expect_read(&path, text, 1, 4, "duplicate entry with null key");
    }

    /// A block `?` with no content points at the indicator, not the column after it.
    #[test]
    fn an_empty_block_key_points_at_the_question_mark() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "k:\n  ?\n  : 1\n  ?\n  : 2\n";
        let mark = text.rfind('?').expect("second key");
        assert_eq!(advance_mark(text, 0, 0, 0, mark), (4, 3));
        assert_eq!(advance_mark(text, 0, 0, 0, mark + 1), (4, 4));
        expect_read(&path, text, 4, 3, "k: duplicate entry with null key");
    }

    /// A duplicate and a later syntax error are different events. The message
    /// stays the duplicate, at the repeated key.
    #[test]
    fn a_duplicate_before_a_syntax_error_keeps_the_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "a: 1\na: 2\nbad: [\n";
        let repeated = text.rfind("a:").expect("repeated key");
        let syntax = text.rfind('[').expect("syntax error");
        assert_eq!(advance_mark(text, 0, 0, 0, repeated), (2, 1));
        assert_eq!(advance_mark(text, 0, 0, 0, syntax), (3, 6));
        expect_read(&path, text, 2, 1, "duplicate entry with key \"a\"");
    }

    /// The repeated `1e0` is line 5 column 9. The scan error names line 7.
    #[test]
    fn a_repeated_spelling_before_a_syntax_error_keeps_its_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "\
easyconfigs:
  - x.eb:
      options:
        1e0: 1
        1e0: 2
bad: [
";
        let repeated = text.rfind("1e0").expect("repeated key");
        let syntax = text.rfind('[').expect("syntax error");
        assert_eq!(advance_mark(text, 0, 0, 0, repeated), (5, 9));
        assert_eq!(advance_mark(text, 0, 0, 0, syntax), (6, 6));
        expect_read(
            &path,
            text,
            5,
            9,
            "easyconfigs[0].x.eb.options: duplicate entry with key \"1e0\"",
        );
    }

    /// `!!map` resolves away. The second plain map is the duplicate, not `z`.
    #[test]
    fn a_resolved_map_tag_matches_a_plain_map() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let flow = "{!!map {a: 1}: 1, {a: 1}: 2, z: 1, z: 2}\n";
        let repeated = flow.rfind("{a: 1}").expect("plain map");
        let later = flow.rfind('z').expect("later key");
        assert_eq!(advance_mark(flow, 0, 0, 0, repeated), (1, 19));
        assert_eq!(advance_mark(flow, 0, 0, 0, later), (1, 36));
        expect_read(&path, flow, 1, 19, "duplicate entry in YAML map");

        let block = "\
k:
  ? {a: 1}
  : 1
  ? !!map {a: 1}
  : 2
  z: 1
  z: 2
";
        let repeated = block.rfind("{a: 1}").expect("tagged map");
        let later = block.rfind('z').expect("later key");
        let first = block.find('{').expect("first key");
        assert_eq!(advance_mark(block, 0, 0, 0, first), (2, 5));
        assert_eq!(advance_mark(block, 0, 0, 0, repeated), (4, 11));
        assert_eq!(advance_mark(block, 0, 0, 0, later), (7, 3));
        expect_read(&path, block, 4, 11, "k: duplicate entry in YAML map");
    }

    /// `!!seq` and `!!str` resolve the same way as `!!map`.
    #[test]
    fn a_resolved_seq_or_str_tag_matches_the_plain_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let seq = "\
k:
  ? [1]
  : 1
  ? !!seq [1]
  : 2
  z: 1
  z: 2
";
        let repeated = seq.rfind('[').expect("tagged sequence");
        let later = seq.rfind('z').expect("later key");
        assert_eq!(advance_mark(seq, 0, 0, 0, repeated), (4, 11));
        assert_eq!(advance_mark(seq, 0, 0, 0, later), (7, 3));
        expect_read(&path, seq, 4, 11, "k: duplicate entry in YAML map");

        let text = "\
k:
  ? {a: 1}
  : 1
  ? !!str {a: 1}
  : 2
  z: 1
  z: 2
";
        let repeated = text.rfind("{a: 1}").expect("tagged map");
        assert_eq!(advance_mark(text, 0, 0, 0, repeated), (4, 11));
        expect_read(&path, text, 4, 11, "k: duplicate entry in YAML map");

        let omap = "\
k:
  ? !!omap
    - a: 1
  : 1
  ? !!omap
    - a: 1
  : 2
  z: 1
  z: 2
";
        let repeated = omap.rfind('-').expect("second omap");
        let later = omap.rfind('z').expect("later key");
        assert_eq!(advance_mark(omap, 0, 0, 0, repeated), (6, 5));
        assert_eq!(advance_mark(omap, 0, 0, 0, later), (9, 3));
        expect_read(&path, omap, 6, 5, "k: duplicate entry in YAML map");
    }

    /// A verbatim `tag:yaml.org,2002:map` is the same key as a plain map.
    #[test]
    fn a_verbatim_core_tag_matches_a_plain_map() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "\
k:
  ? {a: 1}
  : 1
  ? !<tag:yaml.org,2002:map> {a: 1}
  : 2
  z: 1
  z: 2
";
        let repeated = text.rfind("{a: 1}").expect("tagged map");
        let later = text.rfind('z').expect("later key");
        assert_eq!(advance_mark(text, 0, 0, 0, repeated), (4, 30));
        assert_eq!(advance_mark(text, 0, 0, 0, later), (7, 3));
        expect_read(&path, text, 4, 30, "k: duplicate entry in YAML map");
    }

    /// `!<tag:example.com:foo>` resolves to a global tag and stays untagged.
    #[test]
    fn a_verbatim_global_tag_matches_a_plain_map() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let flow = "k: {? !<tag:example.com:foo> {a: 1}: 1, ? {a: 1}: 2, z: 1, z: 2}\n";
        let repeated = flow.rfind("{a: 1}").expect("plain map");
        let later = flow.rfind('z').expect("later key");
        assert_eq!(advance_mark(flow, 0, 0, 0, repeated), (1, 43));
        assert_eq!(advance_mark(flow, 0, 0, 0, later), (1, 60));
        expect_read(&path, flow, 1, 43, "k: duplicate entry in YAML map");

        let block = "\
k:
  ? !<tag:example.com:foo> {a: 1}
  : 1
  ? {a: 1}
  : 2
  z: 1
  z: 2
";
        let repeated = block.rfind("{a: 1}").expect("plain map");
        assert_eq!(advance_mark(block, 0, 0, 0, repeated), (4, 5));
        expect_read(&path, block, 4, 5, "k: duplicate entry in YAML map");
    }

    /// `&A !!map {a: 1}` stored for `*A` compares equal to a plain `{a: 1}`.
    #[test]
    fn an_anchored_map_tag_alias_matches_a_plain_map() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let text = "\
a: &A !!map {a: 1}
k: {? {a: 1}: 1, ? *A: 2, z: 1, z: 2}
";
        let alias = text.rfind("*A").expect("alias key");
        let later = text.rfind('z').expect("later key");
        assert_eq!(advance_mark(text, 0, 0, 0, alias), (2, 20));
        assert_eq!(advance_mark(text, 0, 0, 0, later), (2, 33));
        expect_read(&path, text, 2, 20, "k: duplicate entry in YAML map");

        let beside = "\
a: &A !!map {a: 1}
k: {? *A: 1, ? {a: 1}: 2, z: 1, z: 2}
";
        let plain = beside.rfind("{a: 1}").expect("plain map");
        assert_eq!(advance_mark(beside, 0, 0, 0, plain), (2, 16));
        expect_read(&path, beside, 2, 16, "k: duplicate entry in YAML map");
    }

    /// A local `!foo` tag stays on the key. Two of them are the duplicate.
    /// One `!foo` map and a plain map are different keys, so the repeated `z` is the error.
    #[test]
    fn a_local_tag_stays_on_the_collection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let same = "\
k:
  ? !foo {a: 1}
  : 1
  ? !foo {a: 1}
  : 2
  z: 1
  z: 2
";
        let repeated = same.rfind("{a: 1}").expect("second local map");
        assert_eq!(advance_mark(same, 0, 0, 0, repeated), (4, 10));
        expect_read(&path, same, 4, 10, "k: duplicate entry in YAML map");

        let other = "\
k:
  ? !foo {a: 1}
  : 1
  ? {a: 1}
  : 2
  z: 1
  z: 2
";
        let later = other.rfind('z').expect("later key");
        assert_eq!(advance_mark(other, 0, 0, 0, later), (7, 3));
        expect_read(&path, other, 7, 3, "k: duplicate entry with key \"z\"");
    }

    /// An omitted value leaves the next `?` for that key.
    #[test]
    fn an_omitted_value_leaves_the_next_question_mark() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.yml");
        let cases = [
            (
                "k:\n  ?\n  ?\n  : 1\n",
                3,
                3,
                "k: duplicate entry with null key",
            ),
            (
                "k:\r\n  ?\r\n  ?\r\n  : 1\r\n",
                3,
                3,
                "k: duplicate entry with null key",
            ),
            (
                "k:\n  ?\u{2028}  ?\n  : 1\n",
                3,
                3,
                "k: duplicate entry with null key",
            ),
            (
                "k:\n  ?\u{0085}  ?\n  : 1\n",
                3,
                3,
                "k: duplicate entry with null key",
            ),
            (
                "k:\n  nested:\n    ?\n    ?\n    : 1\n",
                4,
                5,
                "k.nested: duplicate entry with null key",
            ),
            (
                "k:\n  ? # c\n  ? # c\n  : 1\n",
                3,
                3,
                "k: duplicate entry with null key",
            ),
            (
                "k:\n  1e0: 1\n  ?\n  ?\n  : 1\n",
                4,
                3,
                "k: duplicate entry with null key",
            ),
        ];
        for (text, line, column, detail) in cases {
            let mark = text.rfind('?').expect("repeated key");
            assert_eq!(
                advance_mark(text, 0, 0, 0, mark),
                (line, column),
                "{text:?}"
            );
            assert_eq!(
                advance_mark(text, 0, 0, 0, mark + 1),
                (line, column + 1),
                "{text:?}"
            );
            expect_read(&path, text, line, column, detail);
        }
    }
}
