//! ring AEAD/HKDF/HMAC only; no custom cipher or persisted plaintext key.

use super::{Category, FORMAT_VERSION, HostNamespace, Mapping, PrivacyError, Result};
use ring::{aead, hkdf, hmac};
use std::fmt;
use zeroize::Zeroizing;

pub struct HostKey {
    bytes: Zeroizing<[u8; 32]>,
}

impl HostKey {
    /// Explicit supplied host boundary, not a normal-session or keystore grant.
    /// The caller must obtain these bytes separately from model API credentials.
    pub fn supplied_by_host(bytes: Zeroizing<[u8; 32]>) -> Self {
        Self { bytes }
    }
}

impl fmt::Debug for HostKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HostKey([private])")
    }
}

pub(super) struct Keys {
    encryption: Zeroizing<[u8; 32]>,
    equality: Zeroizing<[u8; 32]>,
    namespace: Zeroizing<[u8; 32]>,
    generation: Zeroizing<[u8; 32]>,
}

impl Keys {
    pub(super) fn derive(host: HostKey) -> Result<Self> {
        let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, b"craftsman/private-privacy-vault/v1");
        let prk = salt.extract(host.bytes.as_ref());
        let derive = |label: &'static [u8]| -> Result<Zeroizing<[u8; 32]>> {
            let mut output = Zeroizing::new([0; 32]);
            prk.expand(&[label], hkdf::HKDF_SHA256)
                .map_err(|_| PrivacyError::Key)?
                .fill(output.as_mut())
                .map_err(|_| PrivacyError::Key)?;
            Ok(output)
        };
        Ok(Self {
            encryption: derive(b"aead")?,
            equality: derive(b"equality-index")?,
            namespace: derive(b"namespace")?,
            generation: derive(b"alias-generation")?,
        })
    }

    pub(super) fn scope(&self, id: &[u8; 16], namespace: &HostNamespace) -> [u8; 32] {
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.namespace.as_ref());
        let mut ctx = hmac::Context::with_key(&key);
        ctx.update(b"scope\0");
        ctx.update(&FORMAT_VERSION.to_be_bytes());
        ctx.update(id);
        for part in &namespace.parts {
            ctx.update(&(part.len() as u32).to_be_bytes());
            ctx.update(part.as_bytes());
        }
        ctx.sign().as_ref().try_into().expect("HMAC-SHA256 length")
    }

    pub(super) fn identity(&self, scope: &[u8; 32], kind: Category, value: &str) -> [u8; 32] {
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.equality.as_ref());
        let mut ctx = hmac::Context::with_key(&key);
        ctx.update(b"original\0");
        ctx.update(scope);
        ctx.update(&FORMAT_VERSION.to_be_bytes());
        ctx.update(&kind.code().to_be_bytes());
        ctx.update(&(value.len() as u32).to_be_bytes());
        ctx.update(value.as_bytes());
        ctx.sign().as_ref().try_into().expect("HMAC-SHA256 length")
    }

    /// Global within this vault, preventing reuse across its namespaces too.
    pub(super) fn alias_tag(&self, value: &str) -> [u8; 32] {
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.equality.as_ref());
        let mut ctx = hmac::Context::with_key(&key);
        ctx.update(b"alias\0");
        ctx.update(&FORMAT_VERSION.to_be_bytes());
        ctx.update(&(value.len() as u32).to_be_bytes());
        ctx.update(value.as_bytes());
        ctx.sign().as_ref().try_into().expect("HMAC-SHA256 length")
    }

    pub(super) fn alias_entropy(
        &self,
        scope: &[u8; 32],
        identity: &[u8; 32],
        counter: u32,
    ) -> [u8; 32] {
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.generation.as_ref());
        let mut ctx = hmac::Context::with_key(&key);
        ctx.update(b"pseudonym\0");
        ctx.update(&FORMAT_VERSION.to_be_bytes());
        ctx.update(scope);
        ctx.update(identity);
        ctx.update(&counter.to_be_bytes());
        ctx.sign().as_ref().try_into().expect("HMAC-SHA256 length")
    }

    fn aead(&self) -> Result<aead::LessSafeKey> {
        aead::UnboundKey::new(&aead::CHACHA20_POLY1305, self.encryption.as_ref())
            .map(aead::LessSafeKey::new)
            .map_err(|_| PrivacyError::Key)
    }

    pub(super) fn seal(&self, aad: &[u8], mut plain: Zeroizing<Vec<u8>>) -> Result<Sealed> {
        let mut nonce = [0; 12];
        getrandom::fill(&mut nonce).map_err(|_| PrivacyError::Key)?;
        self.aead()?
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(aad),
                &mut *plain,
            )
            .map_err(|_| PrivacyError::Integrity)?;
        // Only ciphertext is copied out of the zeroized staging buffer.
        Ok(Sealed {
            nonce,
            bytes: plain.to_vec(),
        })
    }

    pub(super) fn open(&self, aad: &[u8], sealed: &Sealed) -> Result<Zeroizing<Vec<u8>>> {
        if sealed.bytes.len() < 16 || sealed.bytes.len() > 1024 {
            return Err(PrivacyError::Integrity);
        }
        let mut bytes = Zeroizing::new(sealed.bytes.clone());
        let plain_len = self
            .aead()?
            .open_in_place(
                aead::Nonce::assume_unique_for_key(sealed.nonce),
                aead::Aad::from(aad),
                &mut bytes,
            )
            .map_err(|_| PrivacyError::Integrity)?
            .len();
        bytes.truncate(plain_len);
        Ok(bytes)
    }
}

pub(super) struct Sealed {
    pub(super) nonce: [u8; 12],
    pub(super) bytes: Vec<u8>,
}

pub(super) fn header_aad(id: &[u8; 16]) -> Vec<u8> {
    let mut aad = b"craftsman/privacy-vault/header/v1\0".to_vec();
    aad.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
    aad.extend_from_slice(id);
    aad
}

pub(super) fn mapping_aad(
    id: &[u8; 16],
    scope: &[u8; 32],
    category: Category,
    version: u32,
    identity: &[u8; 32],
    alias_tag: &[u8; 32],
) -> Vec<u8> {
    let mut aad = b"craftsman/privacy-vault/mapping/v1\0".to_vec();
    aad.extend_from_slice(id);
    aad.extend_from_slice(scope);
    aad.extend_from_slice(&category.code().to_be_bytes());
    aad.extend_from_slice(&version.to_be_bytes());
    aad.extend_from_slice(identity);
    aad.extend_from_slice(alias_tag);
    aad
}

pub(super) fn encode(mapping: &Mapping) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::with_capacity(
        16 + mapping.original.len() + mapping.alias.len(),
    ));
    out.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
    out.extend_from_slice(&mapping.counter.to_be_bytes());
    out.extend_from_slice(&(mapping.original.len() as u32).to_be_bytes());
    out.extend_from_slice(&(mapping.alias.len() as u32).to_be_bytes());
    out.extend_from_slice(mapping.original.as_bytes());
    out.extend_from_slice(mapping.alias.as_bytes());
    out
}

pub(super) fn decode(category: Category, bytes: &[u8]) -> Result<Mapping> {
    if bytes.len() < 16 {
        return Err(PrivacyError::Integrity);
    }
    let word =
        |index| u32::from_be_bytes(bytes[index..index + 4].try_into().expect("checked word"));
    let version = word(0);
    let counter = word(4);
    let original_len = word(8) as usize;
    let alias_len = word(12) as usize;
    if version != FORMAT_VERSION
        || counter >= super::format::ALIAS_ATTEMPTS
        || original_len == 0
        || original_len > 256
        || alias_len == 0
        || alias_len > 256
        || 16 + original_len + alias_len != bytes.len()
    {
        return Err(PrivacyError::Integrity);
    }
    let original =
        std::str::from_utf8(&bytes[16..16 + original_len]).map_err(|_| PrivacyError::Integrity)?;
    let alias =
        std::str::from_utf8(&bytes[16 + original_len..]).map_err(|_| PrivacyError::Integrity)?;
    super::format::validate_original(category, original).map_err(|_| PrivacyError::Integrity)?;
    Ok(Mapping {
        category,
        original: Zeroizing::new(original.to_owned()),
        alias: Zeroizing::new(alias.to_owned()),
        counter,
    })
}
