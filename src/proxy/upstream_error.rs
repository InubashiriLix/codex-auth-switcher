use hyper::{HeaderMap, StatusCode};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpstreamFailureKind {
    TokenExpired,
    InvalidCredential,
    OrganizationMismatch,
    MembershipRemoved,
    IpAllowlist,
    Forbidden,
    TemporaryRateLimit,
    QuotaBlocked,
    UnknownUnauthorized,
    UnknownRateLimit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpstreamFailure {
    pub kind: UpstreamFailureKind,
    pub error_code: Option<String>,
    pub request_id: Option<String>,
    pub retry_after_seconds: Option<u64>,
}

impl UpstreamFailure {
    pub fn event_detail(&self) -> String {
        let code = self.error_code.as_deref().unwrap_or("unknown");
        let request = self.request_id.as_deref().unwrap_or("unavailable");
        format!("class={:?}; code={code}; request_id={request}", self.kind)
    }
}

pub fn classify(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> UpstreamFailure {
    let json = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    let code = first_text(&[
        json.pointer("/error/code"),
        json.pointer("/error/type"),
        json.get("code"),
        json.get("type"),
    ])
    .map(normalize_code);
    let message = first_text(&[
        json.pointer("/error/message"),
        json.get("message"),
        json.pointer("/detail/message"),
    ])
    .unwrap_or_default()
    .to_ascii_lowercase();
    let searchable = format!("{} {message}", code.as_deref().unwrap_or_default());

    let kind = match status {
        StatusCode::UNAUTHORIZED
            if has_any(
                &searchable,
                &[
                    "token_expired",
                    "token expired",
                    "expired token",
                    "jwt expired",
                ],
            ) =>
        {
            UpstreamFailureKind::TokenExpired
        }
        StatusCode::UNAUTHORIZED
            if has_any(
                &searchable,
                &[
                    "ip_not_authorized",
                    "ip not authorized",
                    "ip allowlist",
                    "allowlisted ip",
                ],
            ) =>
        {
            UpstreamFailureKind::IpAllowlist
        }
        StatusCode::UNAUTHORIZED
            if has_any(
                &searchable,
                &[
                    "not a member",
                    "membership",
                    "removed from",
                    "organization membership",
                ],
            ) =>
        {
            UpstreamFailureKind::MembershipRemoved
        }
        StatusCode::UNAUTHORIZED
            if has_any(
                &searchable,
                &[
                    "organization mismatch",
                    "project mismatch",
                    "wrong organization",
                    "wrong project",
                ],
            ) =>
        {
            UpstreamFailureKind::OrganizationMismatch
        }
        StatusCode::UNAUTHORIZED
            if has_any(
                &searchable,
                &[
                    "invalid_api_key",
                    "invalid authentication",
                    "incorrect api key",
                    "invalid token",
                    "revoked",
                    "invalid_grant",
                ],
            ) =>
        {
            UpstreamFailureKind::InvalidCredential
        }
        StatusCode::UNAUTHORIZED => UpstreamFailureKind::UnknownUnauthorized,
        StatusCode::FORBIDDEN => UpstreamFailureKind::Forbidden,
        StatusCode::TOO_MANY_REQUESTS
            if has_any(
                &searchable,
                &[
                    "credit_balance_exhausted",
                    "organization_spend_limit_exceeded",
                    "project_spend_limit_exceeded",
                    "organization_usage_limit_exceeded",
                    "insufficient_quota",
                    "billing",
                ],
            ) =>
        {
            UpstreamFailureKind::QuotaBlocked
        }
        StatusCode::TOO_MANY_REQUESTS
            if retry_after(headers).is_some()
                || has_any(
                    &searchable,
                    &["rate_limit", "rate limit", "too many requests"],
                ) =>
        {
            UpstreamFailureKind::TemporaryRateLimit
        }
        StatusCode::TOO_MANY_REQUESTS => UpstreamFailureKind::UnknownRateLimit,
        _ => UpstreamFailureKind::Forbidden,
    };

    UpstreamFailure {
        kind,
        error_code: code,
        request_id: header_text(headers, "x-request-id"),
        retry_after_seconds: retry_after(headers),
    }
}

fn retry_after(headers: &HeaderMap) -> Option<u64> {
    header_text(headers, "retry-after")?.parse().ok()
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(512).collect())
}

fn first_text(values: &[Option<&Value>]) -> Option<String> {
    values
        .iter()
        .flatten()
        .find_map(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn normalize_code(value: String) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
        .take(128)
        .collect::<String>()
        .to_ascii_lowercase()
}

fn has_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;

    #[test]
    fn classifies_expired_token_without_treating_every_401_as_refreshable() {
        let headers = HeaderMap::new();
        let expired = classify(
            StatusCode::UNAUTHORIZED,
            &headers,
            br#"{"error":{"code":"token_expired","message":"expired"}}"#,
        );
        assert_eq!(expired.kind, UpstreamFailureKind::TokenExpired);

        let unknown = classify(
            StatusCode::UNAUTHORIZED,
            &headers,
            br#"{"error":{"message":"access denied"}}"#,
        );
        assert_eq!(unknown.kind, UpstreamFailureKind::UnknownUnauthorized);
    }

    #[test]
    fn quota_and_temporary_rate_limits_are_distinct() {
        let quota = classify(
            StatusCode::TOO_MANY_REQUESTS,
            &HeaderMap::new(),
            br#"{"error":{"code":"credit_balance_exhausted"}}"#,
        );
        assert_eq!(quota.kind, UpstreamFailureKind::QuotaBlocked);

        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("17"));
        headers.insert("x-request-id", HeaderValue::from_static("req_test"));
        let limited = classify(StatusCode::TOO_MANY_REQUESTS, &headers, b"{}");
        assert_eq!(limited.kind, UpstreamFailureKind::TemporaryRateLimit);
        assert_eq!(limited.retry_after_seconds, Some(17));
        assert_eq!(limited.request_id.as_deref(), Some("req_test"));
    }
}
