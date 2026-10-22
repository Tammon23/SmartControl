use std::{
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{Foundation::*, System::Registry::*};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "SmartControl";
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn read_at(path: &str, name: &str) -> Result<Option<String>, String> {
    unsafe {
        let mut raw = null_mut();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide(path).as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut raw,
        );
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err("Cannot read the startup setting".into());
        }
        let key = Key(raw);
        let mut kind = 0;
        let mut size = 0;
        let name = wide(name);
        let status = RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null(),
            &mut kind,
            null_mut(),
            &mut size,
        );
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS || kind != REG_SZ || size > 65536 || size % 2 != 0 {
            return Err("Cannot read the startup setting".into());
        }
        let mut data = vec![0u16; size as usize / 2 + 1];
        if RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null(),
            &mut kind,
            data.as_mut_ptr() as *mut u8,
            &mut size,
        ) != ERROR_SUCCESS
        {
            return Err("Cannot read the startup setting".into());
        }
        let end = data.iter().position(|c| *c == 0).unwrap_or(data.len());
        Ok(Some(String::from_utf16_lossy(&data[..end])))
    }
}
fn write_at(path: &str, name: &str, command: Option<&str>) -> Result<(), String> {
    unsafe {
        let mut raw = null_mut();
        let path = wide(path);
        let status = if command.is_some() {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                null(),
                &mut raw,
                null_mut(),
            )
        } else {
            RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_SET_VALUE, &mut raw)
        };
        if command.is_none() && status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if status != ERROR_SUCCESS {
            return Err("Cannot change the startup setting".into());
        }
        let key = Key(raw);
        let name = wide(name);
        let status = if let Some(command) = command {
            let data = wide(command);
            RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                (data.len() * 2) as u32,
            )
        } else {
            RegDeleteValueW(key.0, name.as_ptr())
        };
        if status == ERROR_SUCCESS || (command.is_none() && status == ERROR_FILE_NOT_FOUND) {
            Ok(())
        } else {
            Err("Cannot change the startup setting".into())
        }
    }
}
fn command(exe: &Path, config: &Path) -> Result<String, String> {
    if !exe.is_absolute() || !config.is_absolute() {
        return Err("Startup needs absolute application and configuration paths".into());
    }
    let command = format!("\"{}\" \"{}\"", exe.display(), config.display());
    if command.encode_utf16().count() > 260 {
        return Err("The application path is too long for Windows startup. Move SmartControl to a shorter folder and try again.".into());
    }
    Ok(command)
}
pub fn enabled() -> Result<bool, String> {
    Ok(read_at(RUN_KEY, VALUE)?.is_some_and(|value| !value.is_empty()))
}
pub fn set_enabled(enable: bool, config: &Path) -> Result<(), String> {
    if enable {
        let exe = std::env::current_exe().map_err(|_| "Cannot locate SmartControl")?;
        let config =
            std::path::absolute(config).map_err(|_| "Cannot locate the bulb configuration")?;
        write_at(RUN_KEY, VALUE, Some(&command(&exe, &config)?))
    } else {
        write_at(RUN_KEY, VALUE, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enable_disable_preserves_other_registry_values() {
        let path = format!(
            r"Software\SmartControl\Test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        assert_eq!(read_at(&path, VALUE).unwrap(), None);
        write_at(&path, "Other", Some("preserve this")).unwrap();
        write_at(
            &path,
            VALUE,
            Some(r#""C:\Apps\SmartControl.exe" "C:\Apps\client.json""#),
        )
        .unwrap();
        assert!(read_at(&path, VALUE).unwrap().is_some());
        write_at(&path, VALUE, None).unwrap();
        write_at(&path, VALUE, None).unwrap();
        assert_eq!(read_at(&path, VALUE).unwrap(), None);
        assert_eq!(
            read_at(&path, "Other").unwrap().as_deref(),
            Some("preserve this")
        );
        write_at(&path, "Other", None).unwrap();
        assert_eq!(
            unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, wide(&path).as_ptr()) },
            ERROR_SUCCESS
        );
    }
    #[test]
    fn startup_command_preserves_spaces_unicode_and_config_argument() {
        let command = command(
            Path::new(r"C:\My Apps\SmartControl.exe"),
            Path::new(r"C:\Users\Élodie\Bulb Settings\client.json"),
        )
        .unwrap();
        unsafe {
            let mut count = 0;
            let args = windows_sys::Win32::UI::Shell::CommandLineToArgvW(
                wide(&command).as_ptr(),
                &mut count,
            );
            assert!(!args.is_null());
            assert_eq!(count, 2);
            for (i, expected) in [
                r"C:\My Apps\SmartControl.exe",
                r"C:\Users\Élodie\Bulb Settings\client.json",
            ]
            .iter()
            .enumerate()
            {
                let value = *args.add(i);
                let mut length = 0;
                while *value.add(length) != 0 {
                    length += 1;
                }
                assert_eq!(
                    String::from_utf16_lossy(std::slice::from_raw_parts(value, length)),
                    *expected
                );
            }
            LocalFree(args as _);
        }
    }
}
