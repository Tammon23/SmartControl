use serde_json::Value;
use std::path::Path;
const MAGIC: &[u8] = b"SCDPAPI1";

pub fn load(path: &Path) -> Result<Value, String> {
    let data = std::fs::read(path).map_err(|_| "Cannot read local sign-in")?;
    let plain = if data.starts_with(MAGIC) {
        protect(&data[MAGIC.len()..], false)?
    } else {
        data
    };
    serde_json::from_slice(&plain).map_err(|_| "Invalid local sign-in".into())
}
pub fn save(path: &Path, value: &Value) -> Result<(), String> {
    let plain = serde_json::to_vec(value).map_err(|_| "Cannot encode sign-in")?;
    let mut data = MAGIC.to_vec();
    data.extend(protect(&plain, true)?);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| "Cannot create settings folder")?;
    }
    let temporary = path.with_extension("pending");
    std::fs::write(&temporary, data).map_err(|_| "Cannot save encrypted sign-in")?;
    std::fs::rename(temporary, path).map_err(|_| "Cannot replace encrypted sign-in".into())
}
#[cfg(windows)]
fn protect(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{Foundation::LocalFree, Security::Cryptography::*};
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output: CRYPT_INTEGER_BLOB = std::mem::zeroed();
        let ok = if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err("Windows could not unlock sign-in for this user".into());
        }
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData as _);
        Ok(bytes)
    }
}
#[cfg(not(windows))]
fn protect(_: &[u8], _: bool) -> Result<Vec<u8>, String> {
    Err("Windows encryption is required".into())
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn encrypted_round_trip_and_tamper_rejection() {
        let path =
            std::env::temp_dir().join(format!("smartcontrol-auth-{}.bin", std::process::id()));
        let value = serde_json::json!({"access_token":"fake-access-value", "refresh_token":"fake-refresh-value"});
        super::save(&path, &value).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(17).any(|b| b == b"fake-access-value"));
        assert_eq!(super::load(&path).unwrap(), value);
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        assert!(super::load(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
