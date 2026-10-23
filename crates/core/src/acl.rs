//! Files only this user may read, on Windows (Windows plan WD8, W2): where Unix gives the
//! token, state.json and the settings backups mode 0600 and their folders 0700, Windows
//! gives them a **protected DACL** (no inherited entries) that grants full control to the
//! current user's SID only. Doctor reads the token's DACL back and warns when anyone else
//! could read it.
//!
//! The profile's own ACL already keeps other users out of `%APPDATA%`, and the token sits
//! in plain text in `settings.json` anyway (D3), so this is defence in depth, like 0600.
//! Administrators can always read it, as root can on macOS.
//!
//! The verdict is pure (tested on every OS); the calls are `cfg(windows)`.

/// How a file only its owner may read is described to the user: its mode on Unix, its
/// DACL on Windows.
pub const PRIVATE: &str = if cfg!(windows) { "only you can read it" } else { "mode 0600" };

/// SIDs that may read any file anyway: SYSTEM and the Administrators group (like root).
pub const TRUSTED_SIDS: [&str; 2] = ["S-1-5-18", "S-1-5-32-544"];

/// A name for a well-known SID that doctor may report.
pub fn sid_name(sid: &str) -> String {
    match sid {
        "S-1-1-0" => "Everyone".into(),
        "S-1-5-11" => "Authenticated Users".into(),
        "S-1-5-32-545" => "Users".into(),
        "S-1-5-18" => "SYSTEM".into(),
        "S-1-5-32-544" => "Administrators".into(),
        "S-1-5-4" => "INTERACTIVE".into(),
        other => other.to_string(),
    }
}

/// What a file's DACL says: whether it is protected, and its allow entries' SIDs (string
/// form). Deny entries only narrow access, so they are left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dacl {
    pub protected: bool,
    pub allowed: Vec<String>,
}

/// Who besides `user` (and SYSTEM and Administrators) may read a file with this DACL, by
/// name. Empty: only the user. A missing DACL (`None`) means everyone may.
pub fn others(dacl: Option<&Dacl>, user: &str) -> Vec<String> {
    let Some(d) = dacl else { return vec!["Everyone (the file has no DACL)".into()] };
    let mut out: Vec<String> = d.allowed.iter().filter(|s| s.as_str() != user && !TRUSTED_SIDS.contains(&s.as_str())).map(|s| sid_name(s)).collect();
    out.dedup();
    out
}

#[cfg(windows)]
pub use sys::{current_user_sid, make_private, read_dacl};

#[cfg(windows)]
mod sys {
    use super::Dacl;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE};
    use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        AddAccessAllowedAceEx, GetAce, GetAclInformation, GetLengthSid, GetSecurityDescriptorControl, GetTokenInformation, InitializeAcl, AclSizeInformation, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_REVISION, ACL_SIZE_INFORMATION, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// FILE_ALL_ACCESS.
    const FULL_CONTROL: u32 = 0x001F_01FF;
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

    fn wide(p: &Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    /// The current user's SID, as bytes (a TOKEN_USER buffer) and its pointer into them.
    fn user_token() -> std::io::Result<(Vec<u64>, PSID)> {
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u64; (len as usize).div_ceil(8) + 1];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as *mut _, (buf.len() * 8) as u32, &mut len) != 0;
            CloseHandle(token);
            if !ok {
                return Err(std::io::Error::last_os_error());
            }
            let sid = (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid;
            Ok((buf, sid))
        }
    }

    fn sid_string(sid: PSID) -> Option<String> {
        unsafe {
            let mut s: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(sid, &mut s) == 0 || s.is_null() {
                return None;
            }
            let mut n = 0;
            while *s.add(n) != 0 {
                n += 1;
            }
            let out = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
            LocalFree(s as _);
            Some(out)
        }
    }

    /// The current user's SID in string form (`S-1-5-21-…`).
    pub fn current_user_sid() -> Option<String> {
        let (_buf, sid) = user_token().ok()?;
        sid_string(sid)
    }

    /// Gives `path` a protected DACL with one entry: full control for the current user. A
    /// directory's entry is inherited by what is created in it.
    pub fn make_private(path: &Path, directory: bool) -> std::io::Result<()> {
        unsafe {
            let (_buf, sid) = user_token()?;
            let size = (std::mem::size_of::<ACL>() + std::mem::size_of::<ACCESS_ALLOWED_ACE>() + GetLengthSid(sid) as usize + 8) as u32;
            let mut acl_buf = vec![0u64; size as usize / 8 + 1];
            let acl = acl_buf.as_mut_ptr() as *mut ACL;
            if InitializeAcl(acl, size, ACL_REVISION) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let flags = if directory { OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE } else { 0 };
            if AddAccessAllowedAceEx(acl, ACL_REVISION, flags, FULL_CONTROL, sid) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let name = wide(path);
            let r = SetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, std::ptr::null_mut(), std::ptr::null_mut(), acl, std::ptr::null());
            if r != 0 {
                return Err(std::io::Error::from_raw_os_error(r as i32));
            }
            Ok(())
        }
    }

    /// The file's DACL: protected or not, and its allow entries' SIDs. `Ok(None)`: it has
    /// no DACL at all (everyone may read it).
    pub fn read_dacl(path: &Path) -> std::io::Result<Option<Dacl>> {
        unsafe {
            let name = wide(path);
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            let r = GetNamedSecurityInfoW(name.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, std::ptr::null_mut(), std::ptr::null_mut(), &mut dacl, std::ptr::null_mut(), &mut sd);
            if r != 0 {
                return Err(std::io::Error::from_raw_os_error(r as i32));
            }
            let result = (|| {
                if dacl.is_null() {
                    return None;
                }
                let (mut control, mut rev) = (0u16, 0u32);
                GetSecurityDescriptorControl(sd, &mut control, &mut rev);
                let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
                GetAclInformation(dacl, &mut info as *mut _ as *mut _, std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32, AclSizeInformation);
                let mut allowed = Vec::new();
                for i in 0..info.AceCount {
                    let mut ace: *mut core::ffi::c_void = std::ptr::null_mut();
                    if GetAce(dacl, i, &mut ace) == 0 {
                        continue;
                    }
                    if (*(ace as *const ACE_HEADER)).AceType != ACCESS_ALLOWED_ACE_TYPE {
                        continue;
                    }
                    let sid = &(*(ace as *const ACCESS_ALLOWED_ACE)).SidStart as *const u32 as PSID;
                    if let Some(s) = sid_string(sid) {
                        allowed.push(s);
                    }
                }
                Some(Dacl { protected: control & SE_DACL_PROTECTED != 0, allowed })
            })();
            LocalFree(sd as _);
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "S-1-5-21-1-2-3-1001";

    #[test]
    fn only_the_user_system_and_administrators_may_read_it() {
        let d = |sids: &[&str]| Dacl { protected: true, allowed: sids.iter().map(|s| s.to_string()).collect() };
        assert!(others(Some(&d(&[ME])), ME).is_empty(), "what make_private writes");
        assert!(others(Some(&d(&[ME, "S-1-5-18", "S-1-5-32-544"])), ME).is_empty(), "a profile's usual entries");
        assert_eq!(others(Some(&d(&[ME, "S-1-5-32-545", "S-1-1-0"])), ME), ["Users", "Everyone"]);
        assert_eq!(others(Some(&d(&[ME, "S-1-5-11"])), ME), ["Authenticated Users"]);
        assert_eq!(others(Some(&d(&["S-1-5-21-1-2-3-1002"])), ME), ["S-1-5-21-1-2-3-1002"], "another user, by SID");
        assert_eq!(others(None, ME), ["Everyone (the file has no DACL)"]);
    }

    /// Windows: what make_private writes reads back as protected, the user only; a token
    /// written through persist gets it too, and a private directory passes it on.
    #[cfg(windows)]
    #[test]
    fn make_private_leaves_the_user_alone_on_a_protected_dacl() {
        let dir = std::env::temp_dir().join(format!("bw-acl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::persist::create_private_dir(&dir).unwrap();
        let me = current_user_sid().expect("the user's SID");
        let d = read_dacl(&dir).unwrap().expect("a DACL");
        assert!(d.protected && d.allowed == [me.clone()], "{d:?}");
        let token = crate::token::write(&dir, "aclTestTokenNotReal_0123456789").unwrap();
        let t = read_dacl(&token).unwrap().expect("a DACL");
        assert!(t.allowed.iter().all(|s| *s == me), "{t:?}");
        assert!(others(Some(&t), &me).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
