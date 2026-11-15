use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wasmparser::{Parser, Payload};

pub const WASM_SIGNATURE_SECTION: &str = "wasm_signature";
pub const VINE_METADATA_SECTION: &str = "vine.metadata";
pub const PUMPKIN_METADATA_SECTION: &str = "pumpkin.metadata";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WasmSignatureEnvelope {
    pub version: u8,
    pub algorithm: String,
    pub public_key_hex: String,
    pub signature_hex: String,
}

#[derive(Debug, Clone)]
pub struct SignatureVerification {
    pub is_signed: bool,
    pub is_valid: bool,
    pub public_key_hex: Option<String>,
    pub error: Option<String>,
}

/// Computes the lowercase SHA-256 hex digest of `bytes` (used for `.cwasm` component cache keys).
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn is_signature_or_meta_section(name: &str) -> bool {
    matches!(
        name,
        WASM_SIGNATURE_SECTION
            | "pumpkin.signature"
            | VINE_METADATA_SECTION
            | PUMPKIN_METADATA_SECTION
    )
}

/// Strips trailing marketplace/signature custom sections so Wasmtime receives clean component bytes.
#[must_use]
pub fn strip_signature_sections(wasm_bytes: &[u8]) -> Vec<u8> {
    let mut last_valid_end = wasm_bytes.len();
    let mut depth = 0usize;

    for payload in Parser::new(0).parse_all(wasm_bytes) {
        match payload {
            Ok(Payload::Version { ref range, .. }) if depth == 0 => {
                last_valid_end = range.end as usize;
            }
            Ok(ref p @ (Payload::ModuleSection { .. } | Payload::ComponentSection { .. })) => {
                if depth == 0
                    && let Some((_, range)) = p.as_section()
                {
                    last_valid_end = range.end as usize;
                }
                depth += 1;
            }
            Ok(Payload::End(_)) => {
                depth = depth.saturating_sub(1);
            }
            Ok(Payload::CustomSection(cs)) if depth == 0 => {
                if !is_signature_or_meta_section(cs.name()) {
                    last_valid_end = cs.range().end as usize;
                }
            }
            Ok(p) if depth == 0 => {
                if let Some((_, range)) = p.as_section() {
                    last_valid_end = range.end as usize;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    wasm_bytes[..last_valid_end].to_vec()
}

fn read_leb128(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        if shift >= 64 {
            return None;
        }
        result |= u64::from(byte & 0x7f) << shift;
        if (byte & 0x80) == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
    }
    None
}

/// Verifies an Ed25519 `wasm_signature` custom section against `trusted_public_keys`.
///
/// If `trusted_public_keys` is non-empty, the envelope's `public_key_hex` must match one of the
/// configured trusted keys (case-insensitive). If `trusted_public_keys` is empty, the signature
/// is verified cryptographically against the envelope's embedded `public_key_hex`.
#[must_use]
pub fn verify_wasm_signature(
    wasm_bytes: &[u8],
    trusted_public_keys: &[String],
) -> SignatureVerification {
    let clean_wasm = strip_signature_sections(wasm_bytes);
    let mut metadata_raw: Option<Vec<u8>> = None;
    let mut signature_raw: Option<Vec<u8>> = None;
    let mut depth = 0usize;

    for payload in Parser::new(0).parse_all(wasm_bytes) {
        match payload {
            Ok(Payload::ModuleSection { .. } | Payload::ComponentSection { .. }) => depth += 1,
            Ok(Payload::End(_)) => depth = depth.saturating_sub(1),
            Ok(Payload::CustomSection(cs)) if depth == 0 => {
                if cs.name() == VINE_METADATA_SECTION || cs.name() == PUMPKIN_METADATA_SECTION {
                    metadata_raw = Some(cs.data().to_vec());
                } else if cs.name() == WASM_SIGNATURE_SECTION || cs.name() == "pumpkin.signature" {
                    signature_raw = Some(cs.data().to_vec());
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    // Fallback for trailing sections appended without a leading 0x00 section ID byte.
    if (metadata_raw.is_none() || signature_raw.is_none()) && clean_wasm.len() < wasm_bytes.len() {
        let trailing = &wasm_bytes[clean_wasm.len()..];
        let mut cursor = 0;
        while cursor < trailing.len() {
            if trailing[cursor] == 0 {
                cursor += 1;
            }
            let Some((section_len, len_bytes)) = read_leb128(&trailing[cursor..]) else {
                break;
            };
            cursor += len_bytes;
            let Some(section_end) = cursor.checked_add(section_len as usize) else {
                break;
            };
            if section_end > trailing.len() {
                break;
            }
            let section_data = &trailing[cursor..section_end];
            if let Some((name_len, name_len_bytes)) = read_leb128(section_data)
                && let Some(name_end) = name_len_bytes.checked_add(name_len as usize)
                && name_end <= section_data.len()
                && let Ok(name) = std::str::from_utf8(&section_data[name_len_bytes..name_end])
            {
                let payload_data = &section_data[name_end..];
                if name == VINE_METADATA_SECTION || name == PUMPKIN_METADATA_SECTION {
                    metadata_raw = Some(payload_data.to_vec());
                } else if name == WASM_SIGNATURE_SECTION || name == "pumpkin.signature" {
                    signature_raw = Some(payload_data.to_vec());
                }
            }
            cursor = section_end;
        }
    }

    let Some(sig_bytes) = signature_raw else {
        return SignatureVerification {
            is_signed: false,
            is_valid: false,
            public_key_hex: None,
            error: Some("Plugin does not contain a wasm_signature custom section.".into()),
        };
    };

    let envelope: WasmSignatureEnvelope = match serde_json::from_slice(&sig_bytes) {
        Ok(env) => env,
        Err(e) => {
            return SignatureVerification {
                is_signed: true,
                is_valid: false,
                public_key_hex: None,
                error: Some(format!("Invalid signature envelope JSON: {e}")),
            };
        }
    };

    let pub_hex = envelope.public_key_hex.trim().to_ascii_lowercase();
    if !trusted_public_keys.is_empty()
        && !trusted_public_keys
            .iter()
            .any(|k| k.trim().eq_ignore_ascii_case(&pub_hex))
    {
        return SignatureVerification {
            is_signed: true,
            is_valid: false,
            public_key_hex: Some(pub_hex),
            error: Some("Signing public key is not in plugins.trusted_public_keys.".into()),
        };
    }

    let Ok(pub_bytes) = hex::decode(&pub_hex) else {
        return SignatureVerification {
            is_signed: true,
            is_valid: false,
            public_key_hex: Some(pub_hex),
            error: Some("Invalid Ed25519 public key hex in signature envelope.".into()),
        };
    };

    let Ok(verifying_key) = VerifyingKey::try_from(pub_bytes.as_slice()) else {
        return SignatureVerification {
            is_signed: true,
            is_valid: false,
            public_key_hex: Some(pub_hex),
            error: Some("Malformed 32-byte Ed25519 public key.".into()),
        };
    };

    let Ok(raw_sig) = hex::decode(envelope.signature_hex.trim()) else {
        return SignatureVerification {
            is_signed: true,
            is_valid: false,
            public_key_hex: Some(pub_hex),
            error: Some("Invalid signature hex in signature envelope.".into()),
        };
    };

    let Ok(signature) = Signature::from_slice(&raw_sig) else {
        return SignatureVerification {
            is_signed: true,
            is_valid: false,
            public_key_hex: Some(pub_hex),
            error: Some("Invalid 64-byte Ed25519 signature.".into()),
        };
    };

    let mut payload =
        Vec::with_capacity(clean_wasm.len() + metadata_raw.as_ref().map_or(0, std::vec::Vec::len));
    payload.extend_from_slice(&clean_wasm);
    if let Some(meta) = &metadata_raw {
        payload.extend_from_slice(meta);
    }

    match verifying_key.verify(&payload, &signature) {
        Ok(()) => SignatureVerification {
            is_signed: true,
            is_valid: true,
            public_key_hex: Some(pub_hex),
            error: None,
        },
        Err(e) => SignatureVerification {
            is_signed: true,
            is_valid: false,
            public_key_hex: Some(pub_hex),
            error: Some(format!("Ed25519 signature verification failed: {e}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use wasm_encoder::{ComponentBuilder, ComponentSection, CustomSection};

    fn build_signed_component(signing_key: &SigningKey) -> Vec<u8> {
        let clean = ComponentBuilder::default().finish();
        let meta_bytes = br#"{"plugin":"demo"}"#;
        let mut sign_payload = clean.clone();
        sign_payload.extend_from_slice(meta_bytes);
        let sig = signing_key.sign(&sign_payload);

        let envelope = WasmSignatureEnvelope {
            version: 1,
            algorithm: "Ed25519".to_string(),
            public_key_hex: hex::encode(signing_key.verifying_key().to_bytes()),
            signature_hex: hex::encode(sig.to_bytes()),
        };
        let sig_json = serde_json::to_vec(&envelope).unwrap();

        let mut out = clean;
        CustomSection {
            name: VINE_METADATA_SECTION.into(),
            data: meta_bytes.as_slice().into(),
        }
        .append_to_component(&mut out);
        CustomSection {
            name: WASM_SIGNATURE_SECTION.into(),
            data: sig_json.as_slice().into(),
        }
        .append_to_component(&mut out);
        out
    }

    #[test]
    fn verifies_signed_component_and_detects_tampering() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let pub_hex = hex::encode(signing_key.verifying_key().to_bytes());
        let signed = build_signed_component(&signing_key);

        let ok = verify_wasm_signature(&signed, std::slice::from_ref(&pub_hex));
        assert!(ok.is_signed);
        assert!(ok.is_valid);
        assert_eq!(ok.public_key_hex.as_deref(), Some(pub_hex.as_str()));

        // Untrusted key list rejects
        let wrong_key = "00".repeat(32);
        let untrusted = verify_wasm_signature(&signed, &[wrong_key]);
        assert!(untrusted.is_signed);
        assert!(!untrusted.is_valid);

        // Tampering metadata bytes invalidates signature
        let mut tampered = signed.clone();
        let pos = tampered
            .windows(4)
            .position(|w| w == b"demo")
            .expect("metadata present");
        tampered[pos] = b'X';
        let bad = verify_wasm_signature(&tampered, std::slice::from_ref(&pub_hex));
        assert!(bad.is_signed);
        assert!(!bad.is_valid);

        // Unsigned binary reports !is_signed
        let unsigned = ComponentBuilder::default().finish();
        let res = verify_wasm_signature(&unsigned, &[]);
        assert!(!res.is_signed);
        assert!(!res.is_valid);
    }
}
