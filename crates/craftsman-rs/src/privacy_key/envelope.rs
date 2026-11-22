//! Fixed v1 password wrapping. This is not a password-strength or erasure claim.
use super::{KeyError, Result};
use ring::{aead, pbkdf2};
use std::sync::{Mutex, TryLockError};
use std::{fmt, num::NonZeroU32};
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

// Process-wide private KDF admission: no second owner can start another KDF,
// and no blocking queue is introduced. This is not OS scheduling/billing data.
static KDF_ADMISSION: Mutex<()> = Mutex::new(());
#[cfg(test)]
static TEST_SERIAL: Mutex<()> = Mutex::new(());
#[cfg(test)]
pub(super) fn test_serial() -> std::sync::MutexGuard<'static, ()> {
    TEST_SERIAL.lock().expect("owned KDF test admission")
}

pub const ENVELOPE_BYTES: usize = 112;
pub const ITERATIONS: u32 = 600_000;
const MAGIC: &[u8; 8] = b"CPVKENV1";
const DOMAIN: &[u8] = b"craftsman/privacy-key-envelope/v1\0";
const HEADER_BYTES: usize = 64;

/// Consumes an owned buffer; raw and NFC byte lengths are independently bounded.
pub struct Passphrase {
    normalized: Zeroizing<Vec<u8>>,
}
impl fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Passphrase([private])")
    }
}
impl Passphrase {
    pub fn confirm(self, confirmation: Self) -> Result<Self> {
        if self.normalized != confirmation.normalized {
            return Err(KeyError::Passphrase);
        }
        Ok(self)
    }
    pub fn from_owned(raw: Zeroizing<Vec<u8>>) -> Result<Self> {
        if !(12..=1024).contains(&raw.len()) {
            return Err(KeyError::Passphrase);
        }
        let text = std::str::from_utf8(&raw).map_err(|_| KeyError::Passphrase)?;
        // NFC can expand an input. Reserve its entire admitted byte ceiling
        // before storing any secret, so growth cannot free an uncleared buffer.
        let mut normalized = Zeroizing::new(Vec::with_capacity(1024));
        for character in text.nfc() {
            let mut encoded = Zeroizing::new([0; 4]);
            let bytes = character.encode_utf8(encoded.as_mut()).as_bytes();
            if normalized
                .len()
                .checked_add(bytes.len())
                .is_none_or(|n| n > 1024)
            {
                return Err(KeyError::Passphrase);
            }
            normalized.extend_from_slice(bytes);
        }
        if normalized.len() < 12 {
            return Err(KeyError::Passphrase);
        }
        Ok(Self { normalized })
    }
}

pub(super) struct Envelope(Zeroizing<[u8; ENVELOPE_BYTES]>);
impl fmt::Debug for Envelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeyEnvelope([private])")
    }
}

fn wrapping_key(passphrase: &Passphrase, salt: &[u8]) -> Result<aead::LessSafeKey> {
    let _admission = KDF_ADMISSION.try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => KeyError::Busy,
        TryLockError::Poisoned(_) => KeyError::Closed,
    })?;
    let mut key = Zeroizing::new([0; 32]);
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(ITERATIONS).ok_or(KeyError::Invalid)?,
        salt,
        &passphrase.normalized,
        key.as_mut(),
    );
    let key = aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key.as_ref())
        .map_err(|_| KeyError::Invalid)?;
    Ok(aead::LessSafeKey::new(key))
}

fn aad(header: &[u8]) -> Vec<u8> {
    // Header data is public. Every byte, including nonce, is authenticated.
    let mut bytes = Vec::with_capacity(DOMAIN.len() + HEADER_BYTES);
    bytes.extend_from_slice(DOMAIN);
    bytes.extend_from_slice(header);
    bytes
}

impl Envelope {
    pub(super) fn create(passphrase: &Passphrase) -> Result<(Self, Zeroizing<[u8; 32]>)> {
        let mut bytes = Zeroizing::new([0; ENVELOPE_BYTES]);
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..12].copy_from_slice(&1u32.to_be_bytes());
        bytes[12..16].copy_from_slice(&1u32.to_be_bytes());
        bytes[16..20].copy_from_slice(&ITERATIONS.to_be_bytes());
        getrandom::fill(&mut bytes[20..64]).map_err(|_| KeyError::Random)?;
        let mut vault_key = Zeroizing::new([0; 32]);
        getrandom::fill(vault_key.as_mut()).map_err(|_| KeyError::Random)?;
        let nonce: [u8; 12] = bytes[52..64].try_into().map_err(|_| KeyError::Invalid)?;
        let key = wrapping_key(passphrase, &bytes[20..52])?;
        let additional = aad(&bytes[..HEADER_BYTES]);
        bytes[64..96].copy_from_slice(vault_key.as_ref());
        let tag = key
            .seal_in_place_separate_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(additional),
                &mut bytes[64..96],
            )
            .map_err(|_| KeyError::Invalid)?;
        bytes[96..].copy_from_slice(tag.as_ref());
        Ok((Self(bytes), vault_key))
    }

    pub(super) fn parse(bytes: Zeroizing<Vec<u8>>) -> Result<Self> {
        if bytes.len() != ENVELOPE_BYTES
            || bytes[..8] != *MAGIC
            || bytes[8..12] != 1u32.to_be_bytes()
            || bytes[12..16] != 1u32.to_be_bytes()
            || bytes[16..20] != ITERATIONS.to_be_bytes()
        {
            return Err(KeyError::Invalid);
        }
        let mut owned = Zeroizing::new([0; ENVELOPE_BYTES]);
        owned.copy_from_slice(&bytes);
        Ok(Self(owned))
    }

    pub(super) fn unlock(self, passphrase: &Passphrase) -> Result<Zeroizing<[u8; 32]>> {
        let key = wrapping_key(passphrase, &self.0[20..52])?;
        let nonce: [u8; 12] = self.0[52..64].try_into().map_err(|_| KeyError::Invalid)?;
        let additional = aad(&self.0[..HEADER_BYTES]);
        let mut ciphertext = Zeroizing::new(self.0[64..].to_vec());
        let opened = key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(additional),
                &mut ciphertext,
            )
            .map_err(|_| KeyError::Unlock)?;
        if opened.len() != 32 {
            return Err(KeyError::Invalid);
        }
        let mut key = Zeroizing::new([0; 32]);
        key.copy_from_slice(opened);
        Ok(key)
    }

    pub(super) fn as_bytes(&self) -> &[u8; ENVELOPE_BYTES] {
        &self.0
    }
}
