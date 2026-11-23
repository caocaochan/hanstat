//! The Azure Speech key, never kept in plain text ([`crate::secret`]):
//! encrypted for the user with DPAPI on Windows, in the login Keychain on
//! macOS, and in the Secret Service keyring on Linux.
//!
//! [`load`] and [`store`] may wait for the keyring to be unlocked (a password
//! prompt on Linux and macOS): call them off the UI thread.

use crate::secret::Slot;

/// Where builds before [`crate::secret`] kept it: these names stay.
const AZURE: Slot = Slot {
    what: "key",
    file: "azure-speech.key",
    entropy: b"hanstat azure speech key",
    service: "hanstat: Azure Speech",
    account: "azure-speech-key",
    purpose: "azure-speech-key",
    label: "hanstat: Azure Speech key",
};

/// Where the key is kept, for the settings panel.
pub fn where_kept() -> &'static str {
    crate::secret::where_kept()
}

/// The key stored before, if any.
pub fn load() -> Result<Option<String>, String> {
    AZURE.load()
}

/// Keep `key`, replacing the one stored; an empty key removes it.
pub fn store(key: &str) -> Result<(), String> {
    AZURE.store(key)
}
