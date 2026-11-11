//! API key storage. Secrets are protected with Windows DPAPI in current-user
//! scope, so the config file never contains a plaintext key.

use std::slice;

use crate::i18n::{trf, ts};
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
};
use windows::core::PCWSTR;

const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

pub fn protect(plaintext: &str) -> Result<String, String> {
    if plaintext.is_empty() {
        return Ok(String::new());
    }

    let mut input = plaintext.as_bytes().to_vec();
    let blob_in = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_mut_ptr(),
    };
    let mut blob_out = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptProtectData(
            &blob_in,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut blob_out,
        )
        .map_err(|e| trf!("DPAPI 加密失败: {e}", "DPAPI encryption failed: {e}"))?;
    }

    if blob_out.pbData.is_null() || blob_out.cbData == 0 {
        return Err(ts!(
            "DPAPI 加密没有返回数据",
            "DPAPI encryption returned no data"
        )
        .to_string());
    }

    let bytes = unsafe { slice::from_raw_parts(blob_out.pbData, blob_out.cbData as usize) };
    let hex = hex_encode(bytes);
    unsafe {
        LocalFree(Some(HLOCAL(blob_out.pbData.cast())));
    }
    Ok(hex)
}

pub fn unprotect(hex: &str) -> Result<String, String> {
    if hex.is_empty() {
        return Ok(String::new());
    }

    let mut input = hex_decode(hex)?;
    let blob_in = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_mut_ptr(),
    };
    let mut blob_out = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptUnprotectData(
            &blob_in,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut blob_out,
        )
        .map_err(|e| trf!("DPAPI 解密失败: {e}", "DPAPI decryption failed: {e}"))?;
    }

    if blob_out.pbData.is_null() {
        return Err(ts!(
            "DPAPI 解密没有返回数据",
            "DPAPI decryption returned no data"
        )
        .to_string());
    }

    let bytes = unsafe { slice::from_raw_parts(blob_out.pbData, blob_out.cbData as usize) };
    let result = String::from_utf8(bytes.to_vec())
        .map_err(|_| ts!("密钥数据不是合法文本", "Key data is not valid text").to_string());
    unsafe {
        LocalFree(Some(HLOCAL(blob_out.pbData.cast())));
    }
    result
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err(ts!(
            "保存的密钥数据长度非法",
            "Stored key data has an invalid length"
        )
        .to_string());
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.as_bytes().as_chunks::<2>().0 {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        bytes.push((hi << 4) | lo);
    }
    Ok(bytes)
}

fn hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(ts!(
            "保存的密钥数据包含非法字符",
            "Stored key data contains invalid characters"
        )
        .to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let bytes = vec![0, 1, 10, 15, 255, 128];
        assert_eq!(hex_decode(&hex_encode(&bytes)).unwrap(), bytes);
        assert!(hex_decode("zz").is_err());
        assert!(hex_decode("abc").is_err());
    }

    #[test]
    fn dpapi_roundtrip() {
        let secret = "voxkey-dpapi-roundtrip";
        let protected = protect(secret).unwrap();
        assert!(!protected.contains(secret));
        assert_eq!(unprotect(&protected).unwrap(), secret);
    }
}
