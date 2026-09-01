use crate::{
    config::DeviceIdentityConfig,
    error::{AppError, Result},
    identity::local_device_seed,
};
use hyper::{
    body::Bytes,
    header::{CONTENT_ENCODING, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde_json::Value;
use std::io::{Cursor, Read};
use std::path::Path;
use uuid::Uuid;

const INSTALLATION_ID_HEADER: &str = "x-codex-installation-id";
const TURN_METADATA_HEADER: &str = "x-codex-turn-metadata";
const MAX_DECOMPRESSED_JSON_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) struct DeviceFingerprint {
    installation_id: String,
}

impl DeviceFingerprint {
    pub(super) fn from_config(config: &DeviceIdentityConfig, codex_home: &Path) -> Option<Self> {
        config.enabled.then(|| {
            let installation_id = config
                .installation_id
                .or_else(|| read_codex_installation_id(codex_home));
            Self::from_values(installation_id, &local_device_seed())
        })
    }

    #[cfg(test)]
    fn from_config_with_seed(config: &DeviceIdentityConfig, seed: &str) -> Self {
        Self::from_values(config.installation_id, seed)
    }

    fn from_values(installation_id: Option<Uuid>, seed: &str) -> Self {
        let installation_id = installation_id.unwrap_or_else(|| {
            let material = format!("codex-switcher/device/v1/{seed}");
            Uuid::new_v5(&Uuid::NAMESPACE_URL, material.as_bytes())
        });
        Self {
            installation_id: installation_id.to_string(),
        }
    }

    pub(super) fn normalize_headers(&self, headers: &mut HeaderMap) -> Result<()> {
        headers.insert(
            INSTALLATION_ID_HEADER,
            HeaderValue::from_str(&self.installation_id)
                .expect("UUID is always a valid HTTP header value"),
        );
        if let Some(value) = headers.get(TURN_METADATA_HEADER) {
            let metadata = value.to_str().map_err(|_| {
                AppError::Message("x-codex-turn-metadata 不是有效文本，拒绝产生冲突设备身份".into())
            })?;
            let rewritten = self.rewrite_metadata_text(metadata)?;
            headers.insert(
                TURN_METADATA_HEADER,
                HeaderValue::from_str(&rewritten).map_err(|_| {
                    AppError::Message("收敛后的 x-codex-turn-metadata 无法编码为请求头".into())
                })?,
            );
        }
        Ok(())
    }

    pub(super) fn normalize_body(&self, headers: &HeaderMap, body: Bytes) -> Result<Bytes> {
        if body.is_empty() || !is_json(headers) {
            return Ok(body);
        }
        let encoding = headers
            .get(CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .unwrap_or("");
        let decoded = match encoding {
            "" | "identity" => body.to_vec(),
            "zstd" => decode_zstd_limited(&body)?,
            other => {
                return Err(AppError::Message(format!(
                    "设备身份收敛不支持 Content-Encoding: {other}"
                )));
            }
        };
        let mut value: Value = serde_json::from_slice(&decoded).map_err(|error| {
            AppError::Message(format!("无法解析 Codex JSON 请求体以收敛设备身份：{error}"))
        })?;
        self.rewrite_request_json(&mut value)?;
        let rewritten = serde_json::to_vec(&value)?;
        if encoding == "zstd" {
            Ok(Bytes::from(
                zstd::stream::encode_all(Cursor::new(rewritten), 0).map_err(|error| {
                    AppError::Message(format!("无法重新压缩 Codex zstd 请求体：{error}"))
                })?,
            ))
        } else {
            Ok(Bytes::from(rewritten))
        }
    }

    fn rewrite_request_json(&self, value: &mut Value) -> Result<()> {
        let Some(root) = value.as_object_mut() else {
            return Err(AppError::Message("Codex 请求体不是 JSON 对象".into()));
        };
        if let Some(metadata) = root.get_mut(TURN_METADATA_HEADER) {
            self.rewrite_metadata_value(metadata)?;
        }
        if let Some(client_metadata) = root.get_mut("client_metadata") {
            let object = client_metadata
                .as_object_mut()
                .ok_or_else(|| AppError::Message("Codex client_metadata 不是 JSON 对象".into()))?;
            object.insert(
                INSTALLATION_ID_HEADER.into(),
                Value::String(self.installation_id.clone()),
            );
            if let Some(metadata) = object.get_mut(TURN_METADATA_HEADER) {
                self.rewrite_metadata_value(metadata)?;
            }
        }
        Ok(())
    }

    fn rewrite_metadata_value(&self, value: &mut Value) -> Result<()> {
        let text = value
            .as_str()
            .ok_or_else(|| AppError::Message("x-codex-turn-metadata 不是 JSON 字符串".into()))?;
        *value = Value::String(self.rewrite_metadata_text(text)?);
        Ok(())
    }

    fn rewrite_metadata_text(&self, text: &str) -> Result<String> {
        let mut value: Value = serde_json::from_str(text).map_err(|error| {
            AppError::Message(format!("无法解析 x-codex-turn-metadata：{error}"))
        })?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| AppError::Message("x-codex-turn-metadata 不是 JSON 对象".into()))?;
        object.insert(
            "installation_id".into(),
            Value::String(self.installation_id.clone()),
        );
        Ok(serde_json::to_string(&value)?)
    }
}

fn read_codex_installation_id(codex_home: &Path) -> Option<Uuid> {
    let path = codex_home.join("installation_id");
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|value| Uuid::parse_str(value.trim()).ok())
}

fn decode_zstd_limited(body: &[u8]) -> Result<Vec<u8>> {
    let decoder = zstd::stream::read::Decoder::new(Cursor::new(body))
        .map_err(|error| AppError::Message(format!("无法解压 Codex zstd 请求体：{error}")))?;
    let mut decoded = Vec::new();
    decoder
        .take(MAX_DECOMPRESSED_JSON_BYTES + 1)
        .read_to_end(&mut decoded)
        .map_err(|error| AppError::Message(format!("无法解压 Codex zstd 请求体：{error}")))?;
    if decoded.len() as u64 > MAX_DECOMPRESSED_JSON_BYTES {
        return Err(AppError::Message(format!(
            "解压后的 Codex JSON 请求体不能超过 {} MiB",
            MAX_DECOMPRESSED_JSON_BYTES / (1024 * 1024)
        )));
    }
    Ok(decoded)
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').next().is_some_and(|mime| {
                let mime = mime.trim();
                mime.eq_ignore_ascii_case("application/json") || mime.ends_with("+json")
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::{CONTENT_ENCODING, USER_AGENT};

    const ORIGINATOR_HEADER: &str = "originator";

    fn profile() -> DeviceFingerprint {
        let config = DeviceIdentityConfig {
            enabled: true,
            installation_id: Some(Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap()),
            user_agent: Some("stable-agent/1".into()),
            originator: Some("stable-origin".into()),
        };
        DeviceFingerprint::from_config_with_seed(&config, "ignored")
    }

    #[test]
    fn derived_installation_id_is_stable_for_a_device_seed() {
        let config = DeviceIdentityConfig::default();
        let first = DeviceFingerprint::from_config_with_seed(&config, "machine-a");
        let second = DeviceFingerprint::from_config_with_seed(&config, "machine-a");
        let other = DeviceFingerprint::from_config_with_seed(&config, "machine-b");
        assert_eq!(first.installation_id, second.installation_id);
        assert_ne!(first.installation_id, other.installation_id);
    }

    #[test]
    fn existing_codex_installation_id_is_reused() {
        let root = std::env::temp_dir().join(format!("codex-device-id-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("installation_id"),
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee\n",
        )
        .unwrap();
        let profile = DeviceFingerprint::from_config(&DeviceIdentityConfig::default(), &root)
            .expect("identity convergence is enabled by default");
        assert_eq!(
            profile.installation_id,
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn headers_converge_without_changing_session_identity() {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("varying-agent"));
        headers.insert(
            ORIGINATOR_HEADER,
            HeaderValue::from_static("varying-origin"),
        );
        headers.insert(
            TURN_METADATA_HEADER,
            HeaderValue::from_static(
                r#"{"installation_id":"old","session_id":"session-a","thread_id":"thread-a"}"#,
            ),
        );
        profile().normalize_headers(&mut headers).unwrap();
        assert_eq!(headers[USER_AGENT], "varying-agent");
        assert_eq!(headers[ORIGINATOR_HEADER], "varying-origin");
        assert_eq!(
            headers[INSTALLATION_ID_HEADER],
            "11111111-2222-4333-8444-555555555555"
        );
        let metadata: Value =
            serde_json::from_str(headers[TURN_METADATA_HEADER].to_str().unwrap()).unwrap();
        assert_eq!(
            metadata["installation_id"],
            "11111111-2222-4333-8444-555555555555"
        );
        assert_eq!(metadata["session_id"], "session-a");
        assert_eq!(metadata["thread_id"], "thread-a");
    }

    #[test]
    fn json_and_zstd_bodies_converge_all_installation_id_projections() {
        let input = serde_json::json!({
            "prompt_cache_key": "session-a",
            "client_metadata": {
                "x-codex-installation-id": "old",
                "session_id": "session-a",
                "x-codex-turn-metadata": "{\"installation_id\":\"old\",\"session_id\":\"session-a\"}"
            }
        });
        for zstd in [false, true] {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            let raw = serde_json::to_vec(&input).unwrap();
            let body = if zstd {
                headers.insert(CONTENT_ENCODING, HeaderValue::from_static("zstd"));
                Bytes::from(zstd::stream::encode_all(Cursor::new(raw), 0).unwrap())
            } else {
                Bytes::from(raw)
            };
            let body = profile().normalize_body(&headers, body).unwrap();
            let decoded = if zstd {
                zstd::stream::decode_all(Cursor::new(body)).unwrap()
            } else {
                body.to_vec()
            };
            let output: Value = serde_json::from_slice(&decoded).unwrap();
            assert_eq!(output["prompt_cache_key"], "session-a");
            assert_eq!(output["client_metadata"]["session_id"], "session-a");
            assert_eq!(
                output["client_metadata"]["x-codex-installation-id"],
                "11111111-2222-4333-8444-555555555555"
            );
            let metadata: Value = serde_json::from_str(
                output["client_metadata"][TURN_METADATA_HEADER]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                metadata["installation_id"],
                "11111111-2222-4333-8444-555555555555"
            );
            assert_eq!(metadata["session_id"], "session-a");
        }
    }
}
