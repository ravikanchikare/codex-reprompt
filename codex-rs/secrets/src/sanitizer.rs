use regex::Regex;
use std::ops::Range;
use std::sync::LazyLock;

static OPENAI_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bsk-[A-Za-z0-9_-]{20,}\b"));
static ANTHROPIC_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bsk-ant-[A-Za-z0-9_-]{20,}\b"));
static AWS_ACCESS_KEY_ID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bAKIA[0-9A-Z]{16}\b"));
static GITHUB_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bgh[ps]_[A-Za-z0-9_]{36,}\b"));
static SLACK_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bxox[bpras]-[A-Za-z0-9-]{10,}\b"));
static JWT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]+\b")
});
static STRIPE_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\b[rs]k_(?:test|live)_[A-Za-z0-9]{20,}\b"));
static SENDGRID_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\bSG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}\b"));
static CONNECTION_STRING_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(r#"(?i)\b(?:postgres(?:ql)?|mysql|mongodb(?:\+srv)?):\/\/[^\s'"`]+"#)
});
static PRIVATE_KEY_BLOCK_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(
        r"(?s)-----BEGIN(?: [A-Z0-9]+)? PRIVATE KEY-----.*?-----END(?: [A-Z0-9]+)? PRIVATE KEY-----",
    )
});
static BEARER_TOKEN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"(?i)\bBearer\s+(?P<token>[A-Za-z0-9._\-]{20,})\b"));
static SECRET_ASSIGNMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(
        r#"(?ix)
\b(?P<name>[A-Z0-9_]*(?:API[_-]?KEY|TOKEN|SECRET|PASSWORD|PASSWD|PRIVATE[_-]?KEY)[A-Z0-9_]*)
(?P<sep>\s*[:=]\s*)
(?P<open_quote>["']?)
(?P<value>[^\s"'`]{8,})
(?P<close_quote>["']?)
"#,
    )
});
static ENTROPY_CANDIDATE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\b[A-Za-z0-9+/=_-]{24,}\b"));
static UUID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    OpenAiKey,
    AnthropicKey,
    AwsAccessKeyId,
    GithubToken,
    SlackToken,
    Jwt,
    StripeKey,
    SendGridKey,
    BearerToken,
    ConnectionString,
    PrivateKey,
    SecretAssignment,
    HighEntropy,
}

impl SecretKind {
    fn placeholder_tag(self) -> &'static str {
        match self {
            Self::OpenAiKey => "OPENAI_KEY",
            Self::AnthropicKey => "ANTHROPIC_KEY",
            Self::AwsAccessKeyId => "AWS_ACCESS_KEY_ID",
            Self::GithubToken => "GITHUB_TOKEN",
            Self::SlackToken => "SLACK_TOKEN",
            Self::Jwt => "JWT",
            Self::StripeKey => "STRIPE_KEY",
            Self::SendGridKey => "SENDGRID_KEY",
            Self::BearerToken => "BEARER_TOKEN",
            Self::ConnectionString => "CONNECTION_STRING",
            Self::PrivateKey => "PRIVATE_KEY",
            Self::SecretAssignment => "ENV_VALUE",
            Self::HighEntropy => "HIGH_ENTROPY",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionMapping {
    pub kind: SecretKind,
    pub placeholder: String,
    pub original: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionResult {
    pub redacted_text: String,
    pub redaction_count: usize,
    pub mappings: Vec<RedactionMapping>,
}

impl RedactionResult {
    pub fn rehydrate_text(&self, input: &str) -> String {
        rehydrate_redacted_text(input, &self.mappings)
    }
}

#[derive(Debug, Clone)]
pub struct SecretRedactionOptions {
    pub redact_high_entropy: bool,
    pub entropy_threshold: f64,
    pub min_entropy_length: usize,
}

impl Default for SecretRedactionOptions {
    fn default() -> Self {
        Self {
            redact_high_entropy: true,
            entropy_threshold: 4.5,
            min_entropy_length: 24,
        }
    }
}

/// Best-effort legacy secret redaction API retained for compatibility.
///
/// For stable output compatibility this returns a generic placeholder instead of
/// typed placeholders used by `redact_secrets_structured`.
pub fn redact_secrets(input: String) -> String {
    let result = redact_secrets_structured(&input, &SecretRedactionOptions::default());
    let placeholder_regex = compile_regex(r"\[REDACTED:[A-Z0-9_]+:\d+\]");
    placeholder_regex
        .replace_all(&result.redacted_text, "[REDACTED_SECRET]")
        .to_string()
}

/// Structured redaction API that exposes placeholder mappings so callers can
/// safely rehydrate selected fields (for example, refined prompt text).
pub fn redact_secrets_structured(input: &str, options: &SecretRedactionOptions) -> RedactionResult {
    let mut result = RedactionResult {
        redacted_text: input.to_string(),
        redaction_count: 0,
        mappings: Vec::new(),
    };

    apply_full_regex_pass(&mut result, &ANTHROPIC_KEY_REGEX, SecretKind::AnthropicKey);
    apply_full_regex_pass(&mut result, &OPENAI_KEY_REGEX, SecretKind::OpenAiKey);
    apply_full_regex_pass(
        &mut result,
        &AWS_ACCESS_KEY_ID_REGEX,
        SecretKind::AwsAccessKeyId,
    );
    apply_full_regex_pass(&mut result, &GITHUB_TOKEN_REGEX, SecretKind::GithubToken);
    apply_full_regex_pass(&mut result, &SLACK_TOKEN_REGEX, SecretKind::SlackToken);
    apply_full_regex_pass(&mut result, &JWT_REGEX, SecretKind::Jwt);
    apply_full_regex_pass(&mut result, &STRIPE_KEY_REGEX, SecretKind::StripeKey);
    apply_full_regex_pass(&mut result, &SENDGRID_KEY_REGEX, SecretKind::SendGridKey);
    apply_full_regex_pass(
        &mut result,
        &CONNECTION_STRING_REGEX,
        SecretKind::ConnectionString,
    );
    apply_full_regex_pass(
        &mut result,
        &PRIVATE_KEY_BLOCK_REGEX,
        SecretKind::PrivateKey,
    );
    apply_captured_value_pass(
        &mut result,
        &BEARER_TOKEN_REGEX,
        SecretKind::BearerToken,
        "token",
    );
    apply_captured_value_pass(
        &mut result,
        &SECRET_ASSIGNMENT_REGEX,
        SecretKind::SecretAssignment,
        "value",
    );

    if options.redact_high_entropy {
        apply_entropy_pass(&mut result, options);
    }

    result
}

pub fn rehydrate_redacted_text(input: &str, mappings: &[RedactionMapping]) -> String {
    let mut out = input.to_string();
    for mapping in mappings.iter().rev() {
        out = out.replace(&mapping.placeholder, &mapping.original);
    }
    out
}

fn apply_full_regex_pass(result: &mut RedactionResult, regex: &Regex, kind: SecretKind) {
    apply_redaction_pass(result, regex, kind, |cap| cap.get(0).map(capture_to_range));
}

fn apply_captured_value_pass(
    result: &mut RedactionResult,
    regex: &Regex,
    kind: SecretKind,
    capture_name: &str,
) {
    apply_redaction_pass(result, regex, kind, |cap| {
        cap.name(capture_name).map(capture_to_range)
    });
}

fn capture_to_range(cap: regex::Match<'_>) -> Range<usize> {
    cap.start()..cap.end()
}

fn apply_redaction_pass<F>(result: &mut RedactionResult, regex: &Regex, kind: SecretKind, select: F)
where
    F: Fn(&regex::Captures<'_>) -> Option<Range<usize>>,
{
    let input = result.redacted_text.clone();
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    let mut any = false;

    for cap in regex.captures_iter(&input) {
        let Some(range) = select(&cap) else {
            continue;
        };
        if range.start < cursor {
            continue;
        }
        output.push_str(&input[cursor..range.start]);
        let original = input[range.clone()].to_string();
        let placeholder = next_placeholder(kind, result.redaction_count + 1);
        output.push_str(&placeholder);
        result.mappings.push(RedactionMapping {
            kind,
            placeholder,
            original,
        });
        result.redaction_count += 1;
        cursor = range.end;
        any = true;
    }

    if any {
        output.push_str(&input[cursor..]);
        result.redacted_text = output;
    }
}

fn next_placeholder(kind: SecretKind, index: usize) -> String {
    format!("[REDACTED:{}:{index}]", kind.placeholder_tag())
}

fn apply_entropy_pass(result: &mut RedactionResult, options: &SecretRedactionOptions) {
    let input = result.redacted_text.clone();
    let mut matches: Vec<Range<usize>> = Vec::new();

    for mat in ENTROPY_CANDIDATE_REGEX.find_iter(&input) {
        let candidate = mat.as_str();
        if candidate.len() < options.min_entropy_length {
            continue;
        }
        if should_skip_entropy_candidate(&input, candidate, mat.start(), mat.end()) {
            continue;
        }
        if shannon_entropy(candidate) <= options.entropy_threshold {
            continue;
        }
        matches.push(mat.start()..mat.end());
    }

    if matches.is_empty() {
        return;
    }

    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    for range in matches {
        if range.start < cursor {
            continue;
        }
        output.push_str(&input[cursor..range.start]);
        let original = input[range.clone()].to_string();
        let placeholder = next_placeholder(SecretKind::HighEntropy, result.redaction_count + 1);
        output.push_str(&placeholder);
        result.mappings.push(RedactionMapping {
            kind: SecretKind::HighEntropy,
            placeholder,
            original,
        });
        result.redaction_count += 1;
        cursor = range.end;
    }
    output.push_str(&input[cursor..]);
    result.redacted_text = output;
}

fn should_skip_entropy_candidate(
    full_text: &str,
    candidate: &str,
    start: usize,
    end: usize,
) -> bool {
    if candidate.starts_with("http://") || candidate.starts_with("https://") {
        return true;
    }
    if UUID_REGEX.is_match(candidate) {
        return true;
    }
    if is_probable_path(candidate) {
        return true;
    }
    if full_text.as_bytes().get(start.saturating_sub(1)) == Some(&b'/') {
        return true;
    }
    if is_hash_context(full_text, candidate, start, end) {
        return true;
    }
    if is_likely_non_secret_line_context(full_text, start, end) {
        return true;
    }
    false
}

fn is_probable_path(candidate: &str) -> bool {
    (candidate.starts_with('/') || candidate.starts_with("./") || candidate.starts_with("../"))
        || (candidate.contains('/') && candidate.contains('.'))
}

fn is_hash_context(full_text: &str, candidate: &str, start: usize, end: usize) -> bool {
    let is_hex = candidate
        .chars()
        .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase());
    if !is_hex || candidate.len() < 24 {
        return false;
    }
    let prefix_start = start.saturating_sub(32);
    let suffix_end = (end + 32).min(full_text.len());
    let window = &full_text[prefix_start..suffix_end].to_ascii_lowercase();
    window.contains("sha") || window.contains("hash") || window.contains("commit")
}

fn is_likely_non_secret_line_context(full_text: &str, start: usize, end: usize) -> bool {
    let line_start = full_text[..start].rfind('\n').map_or(0, |idx| idx + 1);
    let line_end = full_text[end..]
        .find('\n')
        .map_or(full_text.len(), |idx| end + idx);
    let line = full_text[line_start..line_end].to_ascii_lowercase();
    line.contains("url=")
        || line.contains("path=")
        || line.contains("uuid=")
        || line.contains("commit_hash=")
}

fn shannon_entropy(text: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    let len = text.len() as f64;
    for byte in text.bytes() {
        *counts.entry(byte).or_insert(0usize) += 1;
    }
    counts.values().fold(0.0, |acc, count| {
        let p = *count as f64 / len;
        acc - (p * p.log2())
    })
}

fn compile_regex(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        // Panic is ok thanks to `load_regex` test.
        Err(err) => panic!("invalid regex pattern `{pattern}`: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::collections::HashSet;

    #[test]
    fn load_regex() {
        let _ = redact_secrets_structured("secret", &SecretRedactionOptions::default());
    }

    #[test]
    fn known_patterns_are_redacted() {
        let input = concat!(
            "openai=sk-abcdefghijklmnopqrstuvwxyz123456\n",
            "github=ghp_abcdefghijklmnopqrstuvwxyz1234567890ABCD\n",
            "auth: Bearer ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890\n",
        );
        let result = redact_secrets_structured(input, &SecretRedactionOptions::default());
        assert_eq!(result.redaction_count, 3);
        assert!(
            !result
                .redacted_text
                .contains("sk-abcdefghijklmnopqrstuvwxyz123456")
        );
        assert!(
            !result
                .redacted_text
                .contains("ghp_abcdefghijklmnopqrstuvwxyz1234567890ABCD")
        );
        assert!(
            !result
                .redacted_text
                .contains("ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890")
        );
        assert!(
            result
                .redacted_text
                .contains("Bearer [REDACTED:BEARER_TOKEN:3]")
        );
    }

    #[test]
    fn pem_block_is_redacted() {
        let input =
            "-----BEGIN PRIVATE KEY-----\nAAAABBBBCCCCDDDDEEEEFFFF\n-----END PRIVATE KEY-----";
        let result = redact_secrets_structured(input, &SecretRedactionOptions::default());
        assert_eq!(result.redaction_count, 1);
        assert!(result.redacted_text.contains("[REDACTED:PRIVATE_KEY:1]"));
    }

    #[test]
    fn connection_string_is_redacted() {
        let input = "dsn=postgres://user:pass@example.com:5432/db";
        let result = redact_secrets_structured(input, &SecretRedactionOptions::default());
        assert_eq!(result.redaction_count, 1);
        assert!(
            result
                .redacted_text
                .contains("[REDACTED:CONNECTION_STRING:1]")
        );
    }

    #[test]
    fn env_assignment_redacts_only_value() {
        let input = "API_KEY=\"abcdefghijklmnopqrstuvwxyz123456\"";
        let result = redact_secrets_structured(input, &SecretRedactionOptions::default());
        assert_eq!(result.redaction_count, 1);
        assert_eq!(
            result.redacted_text,
            "API_KEY=\"[REDACTED:ENV_VALUE:1]\"".to_string()
        );
    }

    #[test]
    fn entropy_pass_redacts_high_entropy_candidate() {
        let input = "payload=QmFzZTY0Q2FuZGlkYXRlU3RyaW5nMTIzNDU2Nzg5MDEyMzQ1";
        let options = SecretRedactionOptions {
            redact_high_entropy: true,
            entropy_threshold: 3.8,
            min_entropy_length: 24,
        };
        let result = redact_secrets_structured(input, &options);
        assert!(
            result
                .mappings
                .iter()
                .any(|entry| entry.kind == SecretKind::HighEntropy)
        );
    }

    #[test]
    fn entropy_pass_skips_false_positives() {
        let input = concat!(
            "url=https://example.com/path/to/resource/abcd1234efgh5678ijkl9012mnop3456\n",
            "uuid=550e8400-e29b-41d4-a716-446655440000\n",
            "path=/Users/dev/project/src/main.rs\n",
            "commit_hash=abcdef0123456789abcdef0123456789abcdef01\n",
        );
        let options = SecretRedactionOptions {
            redact_high_entropy: true,
            entropy_threshold: 3.8,
            min_entropy_length: 24,
        };
        let result = redact_secrets_structured(input, &options);
        let kinds: HashSet<SecretKind> = result.mappings.iter().map(|entry| entry.kind).collect();
        assert!(!kinds.contains(&SecretKind::HighEntropy));
    }

    #[test]
    fn rehydrate_round_trip_restores_original_text() {
        let input = concat!(
            "API_KEY=abcdefghijklmnopqrstuvwxyz123456\n",
            "token=sk-ant-abcdefghijklmnopqrstuvwxyz123456789\n",
        );
        let result = redact_secrets_structured(input, &SecretRedactionOptions::default());
        let restored = result.rehydrate_text(&result.redacted_text);
        assert_eq!(restored, input);
    }

    #[test]
    fn legacy_redact_api_uses_generic_placeholder() {
        let redacted = redact_secrets("token=sk-abcdefghijklmnopqrstuvwxyz123456".to_string());
        assert!(redacted.contains("[REDACTED_SECRET]"));
        assert!(!redacted.contains("[REDACTED:"));
    }
}
