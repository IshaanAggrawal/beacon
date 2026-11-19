//! Signed statements a project makes about a user or another project.

use chrono::{DateTime, Utc};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The issuer stands behind this user.
    Vouch,
    /// A scoped, expiring vouch that only the issuing project itself honors.
    /// Lets a maintainer reward a good first PR without the new account
    /// immediately counting for anything elsewhere in the network.
    ProvisionalVouch,
    /// The issuer considers this user a bad actor.
    Denounce,
    /// Withdraws the issuer's earlier vouch.
    Revoke,
    /// Records that the user asked the issuer to reconsider a denouncement.
    Appeal,
    /// Lifts the issuer's earlier denouncements of this user.
    Rehabilitate,
    /// The issuer trusts another project's judgment, with `weight` in 0..=1.
    TrustPeer,
}

impl Kind {
    pub fn about_project(self) -> bool {
        self == Kind::TrustPeer
    }
}

/// How an attestation came to exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Origin {
    /// Signed by the maintainer key `key` (hex-encoded ed25519 public key).
    Signed { key: String },
    /// Converted from an unsigned source, such as a Vouch file. `actor` is the
    /// maintainer who made the original decision, when we can tell.
    Imported {
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attestation {
    pub kind: Kind,
    /// Project id of whoever makes the statement.
    pub issuer: String,
    /// `platform:handle` for users, a project id for `trust_peer`.
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub origin: Origin,
    /// Hash of the previous ledger entry; set when appended to a ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AttestationError {
    #[error("not signed")]
    Unsigned,
    #[error("the signing key is not valid hex-encoded ed25519")]
    BadKey,
    #[error("the signature is malformed")]
    MalformedSignature,
    #[error("the signature does not match the content; it was changed after signing")]
    SignatureMismatch,
    #[error("{0}")]
    Invalid(&'static str),
}

impl Attestation {
    pub fn new(kind: Kind, issuer: &str, subject: &str, created_at: DateTime<Utc>) -> Self {
        Self {
            kind,
            issuer: issuer.to_string(),
            subject: subject.to_string(),
            weight: None,
            reason: None,
            scope: None,
            expires_at: None,
            created_at,
            origin: Origin::Imported {
                source: "unsigned".into(),
                actor: None,
            },
            prev: None,
            sig: None,
        }
    }

    /// Checks the fields make sense together. Signatures are checked separately.
    pub fn validate(&self) -> Result<(), AttestationError> {
        use AttestationError::Invalid;
        if self.issuer.is_empty() || self.subject.is_empty() {
            return Err(Invalid("issuer and subject must be set"));
        }
        match (self.kind, self.weight) {
            (Kind::TrustPeer, Some(w)) if (0.0..=1.0).contains(&w) => {}
            (Kind::TrustPeer, _) => {
                return Err(Invalid("trust_peer needs a weight between 0 and 1"));
            }
            (_, Some(_)) => return Err(Invalid("only trust_peer carries a weight")),
            _ => {}
        }
        if self.kind.about_project() == self.subject.contains(':') {
            return Err(Invalid(
                "users are written platform:handle, projects are written without a colon",
            ));
        }
        if self.kind == Kind::ProvisionalVouch && self.expires_at.is_none() {
            return Err(Invalid("a provisional vouch must expire"));
        }
        if self.kind == Kind::TrustPeer && self.subject == self.issuer {
            return Err(Invalid("a project can't rate itself"));
        }
        Ok(())
    }

    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_none_or(|t| t > now)
    }

    /// The bytes that get signed: everything except the signature itself.
    fn signing_bytes(&self) -> Vec<u8> {
        let mut unsigned = self.clone();
        unsigned.sig = None;
        // Serializing a plain struct with no maps is deterministic in serde_json.
        serde_json::to_vec(&unsigned).expect("attestations always serialize")
    }

    pub fn sign(&mut self, key: &SigningKey) {
        self.origin = Origin::Signed {
            key: hex::encode(key.verifying_key().to_bytes()),
        };
        self.sig = None;
        let sig = key.sign(&self.signing_bytes());
        self.sig = Some(hex::encode(sig.to_bytes()));
    }

    /// Verifies the signature against the key named in `origin`. Whether that
    /// key belongs to the issuer is the policy's job, not this function's.
    pub fn verify_signature(&self) -> Result<(), AttestationError> {
        let (Origin::Signed { key }, Some(sig)) = (&self.origin, &self.sig) else {
            return Err(AttestationError::Unsigned);
        };
        let key = decode_verifying_key(key).ok_or(AttestationError::BadKey)?;
        let sig: [u8; 64] = hex::decode(sig)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or(AttestationError::MalformedSignature)?;
        key.verify_strict(&self.signing_bytes(), &Signature::from_bytes(&sig))
            .map_err(|_| AttestationError::SignatureMismatch)
    }

    /// Hash of the full entry, used to chain ledger entries together.
    pub fn hash(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("attestations always serialize");
        hex::encode(Sha256::digest(&bytes))
    }
}

pub fn decode_verifying_key(hex_key: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = hex::decode(hex_key).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

pub fn generate_key() -> SigningKey {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("the OS random number generator is available");
    SigningKey::from_bytes(&seed)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::TimeZone;

    pub fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    pub fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap() + chrono::Days::new(day.into())
    }

    fn denounce() -> Attestation {
        let mut a = Attestation::new(Kind::Denounce, "ghostty", "github:mallory", at(0));
        a.reason = Some("40 near-identical PRs in a day".into());
        a
    }

    #[test]
    fn signed_attestation_verifies() {
        let mut a = denounce();
        a.sign(&key(1));
        assert_eq!(a.verify_signature(), Ok(()));
    }

    #[test]
    fn editing_after_signing_is_detected() {
        let mut a = denounce();
        a.sign(&key(1));
        a.subject = "github:someone-else".into();
        assert_eq!(
            a.verify_signature(),
            Err(AttestationError::SignatureMismatch)
        );
    }

    #[test]
    fn swapping_in_another_key_is_detected() {
        let mut a = denounce();
        a.sign(&key(1));
        a.origin = Origin::Signed {
            key: hex::encode(key(2).verifying_key().to_bytes()),
        };
        assert_eq!(
            a.verify_signature(),
            Err(AttestationError::SignatureMismatch)
        );
    }

    #[test]
    fn survives_a_json_round_trip() {
        let mut a = Attestation::new(Kind::TrustPeer, "ghostty", "zig", at(3));
        a.weight = Some(0.8);
        a.sign(&key(1));
        let back: Attestation = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
        assert_eq!(back.verify_signature(), Ok(()));
    }

    #[test]
    fn validation_catches_mismatched_fields() {
        let mut peer = Attestation::new(Kind::TrustPeer, "ghostty", "zig", at(0));
        assert!(peer.validate().is_err(), "missing weight");
        peer.weight = Some(1.5);
        assert!(peer.validate().is_err(), "weight out of range");
        peer.weight = Some(0.5);
        assert!(peer.validate().is_ok());

        let user_as_peer = Attestation {
            subject: "github:zig".into(),
            ..peer.clone()
        };
        assert!(user_as_peer.validate().is_err());

        let provisional = Attestation::new(Kind::ProvisionalVouch, "ghostty", "github:a", at(0));
        assert!(provisional.validate().is_err(), "provisional needs expiry");
    }
}
