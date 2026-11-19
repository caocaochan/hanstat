//! The Azure Speech key, never kept in plain text: encrypted for the user
//! with DPAPI on Windows (a file in the app folder only this Windows account
//! can decrypt), in the login Keychain on macOS, and in the Secret Service
//! keyring (GNOME Keyring, KWallet) on Linux.
//!
//! [`load`] and [`store`] may wait for the keyring to be unlocked (a password
//! prompt on Linux and macOS): call them off the UI thread.

/// Where the key is kept, for the settings panel.
pub fn where_kept() -> &'static str {
    if cfg!(windows) {
        "encrypted for your Windows account"
    } else if cfg!(target_os = "macos") {
        "in your Keychain"
    } else {
        "in your keyring"
    }
}

/// The key stored before, if any.
pub fn load() -> Result<Option<String>, String> {
    imp::load()
}

/// Keep `key`, replacing the one stored; an empty key removes it.
pub fn store(key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        imp::clear()
    } else {
        imp::store(key)
    }
}

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    /// Mixed into the encryption, so another program's DPAPI blob isn't
    /// mistaken for this one.
    const ENTROPY: &[u8] = b"hanstat azure speech key";

    fn path() -> PathBuf {
        hanstat::models::app_dir().join("azure-speech.key")
    }

    fn blob(b: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: b.len() as u32,
            pbData: b.as_ptr() as *mut u8,
        }
    }

    /// Copy DPAPI's output and free it.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        unsafe { LocalFree(out.pbData as HLOCAL) };
        v
    }

    /// `plain` encrypted for this Windows account.
    pub fn seal(plain: &[u8]) -> Result<Vec<u8>, String> {
        let (input, entropy) = (blob(plain), blob(ENTROPY));
        let mut out = CRYPT_INTEGER_BLOB::default();
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(format!(
                "Windows can't encrypt the key: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(unsafe { take(out) })
    }

    pub fn unseal(sealed: &[u8]) -> Result<Vec<u8>, String> {
        let (input, entropy) = (blob(sealed), blob(ENTROPY));
        let mut out = CRYPT_INTEGER_BLOB::default();
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(format!(
                "The saved key can't be decrypted (saved by another Windows account?): {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(unsafe { take(out) })
    }

    pub fn load() -> Result<Option<String>, String> {
        let data = match std::fs::read(path()) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        String::from_utf8(unseal(&data)?)
            .map(Some)
            .map_err(|_| "The saved key is damaged".to_owned())
    }

    pub fn store(key: &str) -> Result<(), String> {
        let sealed = seal(key.as_bytes())?;
        let p = path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = p.with_extension("key.tmp");
        std::fs::write(&tmp, &sealed).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }

    pub fn clear() -> Result<(), String> {
        match std::fs::remove_file(path()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    const SERVICE: &str = "hanstat: Azure Speech";
    const ACCOUNT: &str = "azure-speech-key";
    /// errSecItemNotFound.
    const NOT_FOUND: i32 = -25300;

    pub fn load() -> Result<Option<String>, String> {
        match get_generic_password(SERVICE, ACCOUNT) {
            Ok(b) => String::from_utf8(b)
                .map(Some)
                .map_err(|_| "The saved key is damaged".to_owned()),
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(format!("The Keychain refused: {e}")),
        }
    }

    pub fn store(key: &str) -> Result<(), String> {
        set_generic_password(SERVICE, ACCOUNT, key.as_bytes())
            .map_err(|e| format!("The Keychain refused: {e}"))
    }

    pub fn clear() -> Result<(), String> {
        match delete_generic_password(SERVICE, ACCOUNT) {
            Err(e) if e.code() != NOT_FOUND => Err(format!("The Keychain refused: {e}")),
            _ => Ok(()),
        }
    }
}

/// The Secret Service D-Bus API (freedesktop.org), spoken directly: the
/// secret travels over the session bus in the clear ("plain" session), as
/// with most clients; it is stored encrypted by the keyring.
#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::collections::HashMap;
    use zbus::blocking::Connection;
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Type, Value};

    #[derive(Debug, serde::Serialize, serde::Deserialize, Type)]
    struct Secret {
        session: OwnedObjectPath,
        parameters: Vec<u8>,
        value: Vec<u8>,
        content_type: String,
    }

    #[zbus::proxy(
        interface = "org.freedesktop.Secret.Service",
        default_service = "org.freedesktop.secrets",
        default_path = "/org/freedesktop/secrets",
        gen_async = false,
        blocking_name = "Service"
    )]
    trait Service {
        fn open_session(
            &self,
            algorithm: &str,
            input: &Value<'_>,
        ) -> zbus::Result<(OwnedValue, OwnedObjectPath)>;
        fn search_items(
            &self,
            attributes: HashMap<&str, &str>,
        ) -> zbus::Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)>;
        fn unlock(
            &self,
            objects: &[ObjectPath<'_>],
        ) -> zbus::Result<(Vec<OwnedObjectPath>, OwnedObjectPath)>;
        fn read_alias(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
    }

    #[zbus::proxy(
        interface = "org.freedesktop.Secret.Collection",
        default_service = "org.freedesktop.secrets",
        gen_async = false,
        blocking_name = "Collection"
    )]
    trait Collection {
        fn create_item(
            &self,
            properties: HashMap<&str, Value<'_>>,
            secret: &Secret,
            replace: bool,
        ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;
    }

    #[zbus::proxy(
        interface = "org.freedesktop.Secret.Item",
        default_service = "org.freedesktop.secrets",
        gen_async = false,
        blocking_name = "Item"
    )]
    trait Item {
        fn get_secret(&self, session: &ObjectPath<'_>) -> zbus::Result<Secret>;
        fn delete(&self) -> zbus::Result<OwnedObjectPath>;
    }

    #[zbus::proxy(
        interface = "org.freedesktop.Secret.Prompt",
        default_service = "org.freedesktop.secrets",
        gen_async = false,
        blocking_name = "Prompt"
    )]
    trait Prompt {
        fn prompt(&self, window_id: &str) -> zbus::Result<()>;
        #[zbus(signal)]
        fn completed(&self, dismissed: bool, result: Value<'_>) -> zbus::Result<()>;
    }

    fn attributes() -> HashMap<&'static str, &'static str> {
        HashMap::from([("application", "hanstat"), ("purpose", "azure-speech-key")])
    }

    fn err(e: zbus::Error) -> String {
        format!("The keyring (Secret Service) refused: {e}")
    }

    struct Session<'c> {
        conn: &'c Connection,
        service: Service<'c>,
        path: OwnedObjectPath,
    }

    impl<'c> Session<'c> {
        fn open(conn: &'c Connection) -> Result<Session<'c>, String> {
            let service = Service::new(conn)
                .map_err(|e| format!("No keyring (Secret Service) to keep the key in: {e}"))?;
            let (_, path) = service
                .open_session("plain", &Value::from(""))
                .map_err(|e| format!("No keyring (Secret Service) to keep the key in: {e}"))?;
            Ok(Session {
                conn,
                service,
                path,
            })
        }

        /// Run a prompt (unlocking the keyring) and wait for the user.
        fn prompt(&self, prompt: OwnedObjectPath) -> Result<(), String> {
            if prompt.as_str() == "/" {
                return Ok(());
            }
            let p = Prompt::builder(self.conn)
                .path(prompt)
                .map_err(err)?
                .build()
                .map_err(err)?;
            let mut done = p.receive_completed().map_err(err)?;
            p.prompt("").map_err(err)?;
            let signal = done
                .next()
                .ok_or_else(|| "The keyring's prompt went away".to_owned())?;
            let args = signal.args().map_err(err)?;
            if args.dismissed {
                Err("The keyring stayed locked".to_owned())
            } else {
                Ok(())
            }
        }

        fn unlock(&self, path: &OwnedObjectPath) -> Result<(), String> {
            let (_, prompt) = self.service.unlock(&[path.as_ref()]).map_err(err)?;
            self.prompt(prompt)
        }

        /// The stored item, unlocked.
        fn item(&self) -> Result<Option<OwnedObjectPath>, String> {
            let (unlocked, locked) = self.service.search_items(attributes()).map_err(err)?;
            if let Some(i) = unlocked.into_iter().next() {
                return Ok(Some(i));
            }
            let Some(i) = locked.into_iter().next() else {
                return Ok(None);
            };
            self.unlock(&i)?;
            Ok(Some(i))
        }

        fn item_proxy(&self, path: OwnedObjectPath) -> Result<Item<'c>, String> {
            Item::builder(self.conn)
                .path(path)
                .map_err(err)?
                .build()
                .map_err(err)
        }
    }

    fn connect() -> Result<Connection, String> {
        Connection::session().map_err(|e| format!("No session bus for the keyring: {e}"))
    }

    pub fn load() -> Result<Option<String>, String> {
        let conn = connect()?;
        let s = Session::open(&conn)?;
        let Some(path) = s.item()? else {
            return Ok(None);
        };
        let secret = s
            .item_proxy(path)?
            .get_secret(&s.path.as_ref())
            .map_err(err)?;
        String::from_utf8(secret.value)
            .map(Some)
            .map_err(|_| "The saved key is damaged".to_owned())
    }

    pub fn store(key: &str) -> Result<(), String> {
        let conn = connect()?;
        let s = Session::open(&conn)?;
        let collection = s.service.read_alias("default").map_err(err)?;
        if collection.as_str() == "/" {
            return Err("The keyring has no default collection to keep the key in".to_owned());
        }
        s.unlock(&collection)?;
        let c = Collection::builder(&conn)
            .path(collection)
            .map_err(err)?
            .build()
            .map_err(err)?;
        let props = HashMap::from([
            (
                "org.freedesktop.Secret.Item.Label",
                Value::from("hanstat: Azure Speech key"),
            ),
            (
                "org.freedesktop.Secret.Item.Attributes",
                Value::from(attributes()),
            ),
        ]);
        let secret = Secret {
            session: s.path.clone(),
            parameters: Vec::new(),
            value: key.as_bytes().to_vec(),
            content_type: "text/plain".to_owned(),
        };
        let (_, prompt) = c.create_item(props, &secret, true).map_err(err)?;
        s.prompt(prompt)
    }

    pub fn clear() -> Result<(), String> {
        let conn = connect()?;
        let s = Session::open(&conn)?;
        if let Some(path) = s.item()? {
            let prompt = s.item_proxy(path)?.delete().map_err(err)?;
            s.prompt(prompt)?;
        }
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    /// In memory only: the saved key file is never touched.
    #[test]
    fn dpapi_round_trip() {
        let key = "0123456789abcdef0123456789abcdef";
        let sealed = super::imp::seal(key.as_bytes()).unwrap();
        assert!(
            !sealed.windows(key.len()).any(|w| w == key.as_bytes()),
            "not in the clear"
        );
        assert_eq!(super::imp::unseal(&sealed).unwrap(), key.as_bytes());
        let mut bad = sealed.clone();
        let n = bad.len();
        bad[n - 1] ^= 1;
        assert!(super::imp::unseal(&bad).is_err(), "tampering is caught");
    }
}
