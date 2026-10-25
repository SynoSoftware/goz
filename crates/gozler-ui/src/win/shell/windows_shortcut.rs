//! Save before changing Explorer, so a crash cannot lose the setting to restore.

use super::*;
use serde::{Deserialize, Serialize};
use windows::Win32::System::Registry::{REG_EXPAND_SZ, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ};

const EXPLORER: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const DISABLED: &str = "DisabledHotkeys";
const SETTINGS: &str = r"Software\Gozler";
const BACKUP: &str = "WindowsShortcutBackup";
const MAX_STRING_UNITS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Setting {
    kind: u32,
    text: String,
}

#[derive(Serialize, Deserialize)]
struct Backup {
    key: u8,
    previous: Option<Setting>,
}

fn read(
    key: &str,
    name: &str,
    _worker: &super::super::work::OffThread,
) -> io::Result<Option<Setting>> {
    let mut text = [0u16; MAX_STRING_UNITS];
    let mut bytes = size_of_val(&text) as u32;
    let mut kind = Default::default();
    // SAFETY: the names, type and fixed-size UTF-16 buffer outlive the call.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
            Some(&mut kind),
            Some(text.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    registry(status)?;
    if !bytes.is_multiple_of(2) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let length = bytes as usize / 2;
    let text = text
        .get(..length)
        .and_then(|text| text.strip_suffix(&[0]))
        .ok_or(io::ErrorKind::InvalidData)?;
    let text = String::from_utf16(text).map_err(io::Error::other)?;
    if text.contains('\0') {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(Some(Setting { kind: kind.0, text }))
}

fn write(value: Option<&Setting>, _worker: &super::super::work::OffThread) -> io::Result<()> {
    let Some(value) = value else {
        return delete_value(EXPLORER, DISABLED);
    };
    let text: Vec<_> = value.text.encode_utf16().chain([0]).collect();
    // SAFETY: the names and terminated UTF-16 buffer outlive the call.
    registry(unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(EXPLORER),
            &HSTRING::from(DISABLED),
            value.kind,
            Some(text.as_ptr().cast()),
            (text.len() * 2) as u32,
        )
    })
}

fn disabled(previous: Option<&Setting>, key: u8) -> Setting {
    let mut value = previous.cloned().unwrap_or(Setting {
        kind: REG_SZ.0,
        text: String::new(),
    });
    if !value
        .text
        .bytes()
        .any(|letter| letter.eq_ignore_ascii_case(&key))
    {
        value.text.push(char::from(key));
    }
    value
}

fn restored(current: Option<Setting>, saved: Backup) -> Option<Setting> {
    if current.as_ref() == Some(&disabled(saved.previous.as_ref(), saved.key)) {
        saved.previous
    } else {
        current.map(|mut value| {
            value
                .text
                .retain(|letter| !letter.eq_ignore_ascii_case(&char::from(saved.key)));
            value
        })
    }
}

pub(super) fn set_disabled(
    key: Option<u8>,
    worker: &super::super::work::OffThread,
) -> io::Result<()> {
    let saved = read(SETTINGS, BACKUP, worker)?
        .map(|value| serde_json::from_str::<Backup>(&value.text).map_err(io::Error::other))
        .transpose()?;
    if key.is_none() && saved.is_none() {
        return Ok(());
    }
    let mut current = read(EXPLORER, DISABLED, worker)?;
    let mut owned = false;
    if let Some(saved) = saved {
        if !saved.key.is_ascii_alphanumeric()
            || saved
                .previous
                .as_ref()
                .is_some_and(|value| ![REG_SZ.0, REG_EXPAND_SZ.0].contains(&value.kind))
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if key == Some(saved.key) {
            owned = true;
        } else {
            current = restored(current, saved);
            write(current.as_ref(), worker)?;
            delete_value(SETTINGS, BACKUP)?;
        }
    }
    if let Some(key) = key {
        let changed = disabled(current.as_ref(), key);
        if current.as_ref() != Some(&changed) {
            if !owned {
                let saved = serde_json::to_string(&Backup {
                    key,
                    previous: current,
                })
                .map_err(io::Error::other)?;
                if saved.encode_utf16().count() >= MAX_STRING_UNITS {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                set_string(SETTINGS, Some(BACKUP), &saved)?;
            }
            write(Some(&changed), worker)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Setting {
        Setting {
            kind: REG_SZ.0,
            text: value.to_owned(),
        }
    }

    #[test]
    fn windows_shortcut_restoration_preserves_prior_and_external_settings() {
        assert_eq!(disabled(Some(&text("Rs")), b'S'), text("Rs"));
        for (previous, current, expected) in [
            (None, Some(text("S")), None),
            (Some(text("")), Some(text("S")), Some(text(""))),
            (Some(text("R")), Some(text("RS")), Some(text("R"))),
            (Some(text("R")), Some(text("ES")), Some(text("E"))),
            (Some(text("R")), None, None),
        ] {
            assert_eq!(
                restored(
                    current,
                    Backup {
                        key: b'S',
                        previous
                    }
                ),
                expected
            );
        }
    }
}
