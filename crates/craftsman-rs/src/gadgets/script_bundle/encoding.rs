//! Complete bounded DATA-view and actual distinct code-envelope sizing.
//! Nothing is dispatched; the image, task data/work, consent and host are absent.

use super::{
    AdmissionError, CandidateBundle, ENCODED_BYTES, Result, RuntimeProfile, SuppliedFile, text,
};
use std::io::{self, Write};

struct Count(usize);
impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|size| *size <= ENCODED_BYTES)
            .ok_or_else(|| io::Error::other("script data encoding bound"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(super) fn measure(candidate: &CandidateBundle<'_>, runtime: RuntimeProfile) -> Result<usize> {
    let mut count = Count(0);
    encode_data(candidate, runtime, &mut count).map_err(|_| AdmissionError::Bound)?;
    Ok(count.0)
}
pub(super) fn measure_guest_code(
    candidate: &CandidateBundle<'_>,
    runtime: RuntimeProfile,
) -> Result<usize> {
    let mut count = Count(0);
    encode_guest_code(candidate, runtime, &mut count).map_err(|_| AdmissionError::Bound)?;
    Ok(count.0)
}
fn string(out: &mut impl Write, value: &[u8]) -> io::Result<()> {
    let value = text(value).map_err(|_| io::Error::other("invalid script data text"))?;
    serde_json::to_writer(out, value).map_err(io::Error::other)
}
fn source(out: &mut impl Write, file: &SuppliedFile<'_>) -> io::Result<()> {
    out.write_all(b"{\"name\":")?;
    string(out, file.name)?;
    out.write_all(b",\"source\":")?;
    string(out, file.bytes)?;
    out.write_all(b"}")
}
fn binary(out: &mut impl Write, file: &SuppliedFile<'_>) -> io::Result<()> {
    out.write_all(b"{\"name\":")?;
    string(out, file.name)?;
    out.write_all(b",\"base64\":\"")?;
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    // Fixed stack chunk, no whole base64 allocation before encoding admission.
    for bytes in file.bytes.chunks(3) {
        let a = bytes[0];
        let b = bytes.get(1).copied().unwrap_or(0);
        let c = bytes.get(2).copied().unwrap_or(0);
        out.write_all(&[
            ALPHABET[(a >> 2) as usize],
            ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize],
            if bytes.len() > 1 {
                ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize]
            } else {
                b'='
            },
            if bytes.len() > 2 {
                ALPHABET[(c & 63) as usize]
            } else {
                b'='
            },
        ])?;
    }
    out.write_all(b"\"}")
}
fn array(out: &mut impl Write, files: &[SuppliedFile<'_>], binary_data: bool) -> io::Result<()> {
    out.write_all(b"[")?;
    for (index, file) in files.iter().enumerate() {
        if index != 0 {
            out.write_all(b",")?;
        }
        if binary_data {
            binary(out, file)?;
        } else {
            source(out, file)?;
        }
    }
    out.write_all(b"]")
}
pub(super) fn encode_data(
    candidate: &CandidateBundle<'_>,
    runtime: RuntimeProfile,
    out: &mut impl Write,
) -> io::Result<()> {
    out.write_all(b"{\"runtime\":")?;
    string(out, runtime.name().as_bytes())?;
    out.write_all(b",\"metadata\":")?;
    string(out, candidate.metadata)?;
    out.write_all(b",\"entry\":")?;
    source(out, &candidate.entry)?;
    out.write_all(b",\"modules\":")?;
    array(out, candidate.modules, false)?;
    out.write_all(b",\"assets\":")?;
    array(out, candidate.assets, true)?;
    out.write_all(b",\"artifacts\":")?;
    array(out, candidate.artifacts, true)?;
    out.write_all(b",\"image_provided\":")?;
    out.write_all(if candidate.image.is_some() {
        b"true"
    } else {
        b"false"
    })?;
    out.write_all(b"}")
}
pub(super) fn encode_guest_code(
    candidate: &CandidateBundle<'_>,
    runtime: RuntimeProfile,
    out: &mut impl Write,
) -> io::Result<()> {
    if runtime == RuntimeProfile::Cpython {
        // The real python v4 guest uses version1/code/modules/cwd; no entry
        // field, package directories or JS work/input codec is invented here.
        out.write_all(b"{\"version\":1,\"code\":")?;
        string(out, candidate.entry.bytes)?;
        out.write_all(b",\"modules\":{")?;
    } else {
        // No guest version field; TS transformation belongs to this WASM image.
        out.write_all(b"{\"entry\":")?;
        string(out, candidate.entry.name)?;
        out.write_all(b",\"modules\":{")?;
        string(out, candidate.entry.name)?;
        out.write_all(b":")?;
        string(out, candidate.entry.bytes)?;
    }
    for (index, module) in candidate.modules.iter().enumerate() {
        if runtime != RuntimeProfile::Cpython || index != 0 {
            out.write_all(b",")?;
        }
        string(out, module.name)?;
        out.write_all(b":")?;
        string(out, module.bytes)?;
    }
    out.write_all(if runtime == RuntimeProfile::Cpython {
        b"},\"cwd\":\"/work\"}"
    } else {
        b"},\"work\":{},\"input\":null}"
    })
}
