//! Optional "start with Windows" registration for the installed launcher.
//!
//! The entry points at `StarkRune.exe --background`, so sign-in opens the
//! tray + island without a main window. A development checkout has no
//! launcher and reports an honest error instead of writing a broken command.

#[cfg(windows)]
fn command() -> Result<String, String> {
    let root = std::env::var("STARK_INSTALL_ROOT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "Start with Windows is available in the installed app.".to_string())?;
    let launcher = root.join("StarkRune.exe");
    if !launcher.is_file() {
        return Err(format!("{} is missing.", launcher.display()));
    }
    Ok(format!("\"{}\" --background", launcher.display()))
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(windows)]
pub fn enabled() -> bool {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, KEY_QUERY_VALUE,
    };
    let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let name = wide("StarkRune");
    let mut key = std::ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    if opened != 0 {
        return false;
    }
    let mut size = 0u32;
    let found = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    } == 0;
    unsafe { RegCloseKey(key) };
    found
}

#[cfg(windows)]
pub fn set_enabled(on: bool) -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY_CURRENT_USER,
        KEY_SET_VALUE, REG_SZ,
    };
    let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let name = wide("StarkRune");
    let mut key = std::ptr::null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut key,
        )
    };
    if opened != 0 {
        return Err(format!(
            "Could not open the Windows startup key ({opened})."
        ));
    }
    let result = if on {
        let value: Vec<u16> = command()?.encode_utf16().chain(Some(0)).collect();
        unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteValueW(key, name.as_ptr()) }
    };
    unsafe { RegCloseKey(key) };
    // ERROR_FILE_NOT_FOUND while disabling is success: the value is already absent.
    if result != 0 && !(!on && result == 2) {
        Err(format!("Windows startup registration failed ({result})."))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub fn enabled() -> bool {
    false
}

#[cfg(not(windows))]
pub fn set_enabled(_on: bool) -> Result<(), String> {
    Err("Start with Windows is only available on Windows.".to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_dev_checkout_does_not_write_a_startup_command() {
        #[cfg(windows)]
        {
            std::env::remove_var("STARK_INSTALL_ROOT");
            assert!(super::command().is_err());
        }
    }
}
