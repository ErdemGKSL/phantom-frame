use crate::CompressStrategy;
use anyhow::{anyhow, bail, Result};
use axum::http::HeaderMap;
use brotli::{CompressorWriter, Decompressor};
use flate2::{
    read::{GzDecoder, ZlibDecoder},
    write::{GzEncoder, ZlibEncoder},
    Compression,
};
use std::io::{Read, Write};
use tokio::task;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentEncoding {
    Brotli,
    Gzip,
    Deflate,
}

impl ContentEncoding {
    pub fn as_header_value(self) -> &'static str {
        match self {
            Self::Brotli => "br",
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
        }
    }

    pub fn from_header_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "br" | "brotli" => Some(Self::Brotli),
            "gzip" | "x-gzip" => Some(Self::Gzip),
            "deflate" => Some(Self::Deflate),
            _ => None,
        }
    }
}

pub fn configured_encoding(strategy: &CompressStrategy) -> Option<ContentEncoding> {
    match strategy {
        CompressStrategy::None => None,
        CompressStrategy::Brotli => Some(ContentEncoding::Brotli),
        CompressStrategy::Gzip => Some(ContentEncoding::Gzip),
        CompressStrategy::Deflate => Some(ContentEncoding::Deflate),
    }
}

pub fn compress_body(body: &[u8], encoding: ContentEncoding) -> Result<Vec<u8>> {
    match encoding {
        ContentEncoding::Brotli => {
            let mut output = Vec::new();
            {
                let mut writer = CompressorWriter::new(&mut output, 4096, 5, 22);
                writer.write_all(body)?;
                writer.flush()?;
            }
            Ok(output)
        }
        ContentEncoding::Gzip => {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(body)?;
            Ok(encoder.finish()?)
        }
        ContentEncoding::Deflate => {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(body)?;
            Ok(encoder.finish()?)
        }
    }
}

pub async fn compress_body_async(body: Vec<u8>, encoding: ContentEncoding) -> Result<Vec<u8>> {
    task::spawn_blocking(move || compress_body(&body, encoding))
        .await
        .map_err(|error| anyhow!("compression task failed: {}", error))?
}

pub fn decompress_body(body: &[u8], encoding: ContentEncoding) -> Result<Vec<u8>> {
    let mut output = Vec::new();

    match encoding {
        ContentEncoding::Brotli => {
            let mut decoder = Decompressor::new(body, 4096);
            decoder.read_to_end(&mut output)?;
        }
        ContentEncoding::Gzip => {
            let mut decoder = GzDecoder::new(body);
            decoder.read_to_end(&mut output)?;
        }
        ContentEncoding::Deflate => {
            let mut decoder = ZlibDecoder::new(body);
            decoder.read_to_end(&mut output)?;
        }
    }

    Ok(output)
}

pub async fn decompress_body_async(body: Vec<u8>, encoding: ContentEncoding) -> Result<Vec<u8>> {
    task::spawn_blocking(move || decompress_body(&body, encoding))
        .await
        .map_err(|error| anyhow!("decompression task failed: {}", error))?
}

pub fn decode_upstream_body(body: &[u8], content_encoding: Option<&str>) -> Result<Vec<u8>> {
    let Some(content_encoding) = content_encoding else {
        return Ok(body.to_vec());
    };

    let normalized = content_encoding.trim();
    if normalized.is_empty() || normalized.eq_ignore_ascii_case("identity") {
        return Ok(body.to_vec());
    }

    let encodings: Vec<&str> = normalized
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect();

    if encodings.len() != 1 {
        bail!("unsupported upstream content-encoding chain: {normalized}");
    }

    let encoding = ContentEncoding::from_header_value(encodings[0])
        .ok_or_else(|| anyhow!("unsupported upstream content-encoding: {}", encodings[0]))?;

    decompress_body(body, encoding)
}

pub async fn decode_upstream_body_async(
    body: Vec<u8>,
    content_encoding: Option<String>,
) -> Result<Vec<u8>> {
    task::spawn_blocking(move || decode_upstream_body(&body, content_encoding.as_deref()))
        .await
        .map_err(|error| anyhow!("upstream decode task failed: {}", error))?
}

pub fn client_accepts_encoding(headers: &HeaderMap, encoding: ContentEncoding) -> bool {
    let Some(value) = headers.get(axum::http::header::ACCEPT_ENCODING) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };

    encoding_quality(value, encoding.as_header_value()) > 0.0
}

pub fn identity_acceptable(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(axum::http::header::ACCEPT_ENCODING) else {
        return true;
    };
    let Ok(value) = value.to_str() else {
        return true;
    };

    let identity_quality = token_quality(value, "identity");
    if let Some(quality) = identity_quality {
        return quality > 0.0;
    }

    match token_quality(value, "*") {
        Some(quality) => quality > 0.0,
        None => true,
    }
}

fn encoding_quality(value: &str, encoding: &str) -> f32 {
    if let Some(quality) = token_quality(value, encoding) {
        return quality;
    }

    token_quality(value, "*").unwrap_or(0.0)
}

fn token_quality(value: &str, token_name: &str) -> Option<f32> {
    value.split(',').find_map(|item| {
        let mut segments = item.trim().split(';');
        let token = segments.next()?.trim();
        if !token.eq_ignore_ascii_case(token_name) {
            return None;
        }

        let quality = segments
            .find_map(|segment| {
                let mut key_value = segment.trim().splitn(2, '=');
                let key = key_value.next()?.trim();
                let raw_value = key_value.next()?.trim();
                if !key.eq_ignore_ascii_case("q") {
                    return None;
                }
                raw_value.parse::<f32>().ok()
            })
            .unwrap_or(1.0);

        Some(quality.clamp(0.0, 1.0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn test_configured_encoding() {
        assert_eq!(configured_encoding(&CompressStrategy::None), None);
        assert_eq!(
            configured_encoding(&CompressStrategy::Brotli),
            Some(ContentEncoding::Brotli)
        );
        assert_eq!(
            configured_encoding(&CompressStrategy::Gzip),
            Some(ContentEncoding::Gzip)
        );
        assert_eq!(
            configured_encoding(&CompressStrategy::Deflate),
            Some(ContentEncoding::Deflate)
        );
    }

    #[test]
    fn test_round_trip_compression() {
        let body = b"phantom-frame compression test body";

        for encoding in [
            ContentEncoding::Brotli,
            ContentEncoding::Gzip,
            ContentEncoding::Deflate,
        ] {
            let compressed = compress_body(body, encoding).unwrap();
            let decompressed = decompress_body(&compressed, encoding).unwrap();
            assert_eq!(decompressed, body);
        }
    }

    #[test]
    fn test_decode_upstream_identity() {
        let body = b"plain";
        assert_eq!(decode_upstream_body(body, None).unwrap(), body);
        assert_eq!(decode_upstream_body(body, Some("identity")).unwrap(), body);
    }

    #[test]
    fn test_client_accepts_encoding_with_q_values() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip;q=0.5, br;q=1.0"),
        );

        assert!(client_accepts_encoding(&headers, ContentEncoding::Brotli));
        assert!(client_accepts_encoding(&headers, ContentEncoding::Gzip));
        assert!(!client_accepts_encoding(&headers, ContentEncoding::Deflate));
    }

    #[test]
    fn test_identity_acceptable() {
        let mut headers = HeaderMap::new();
        assert!(identity_acceptable(&headers));

        headers.insert(
            axum::http::header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, br"),
        );
        assert!(identity_acceptable(&headers));

        headers.insert(
            axum::http::header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, identity;q=0"),
        );
        assert!(!identity_acceptable(&headers));

        headers.insert(
            axum::http::header::ACCEPT_ENCODING,
            HeaderValue::from_static("*;q=0"),
        );
        assert!(!identity_acceptable(&headers));
    }
}
