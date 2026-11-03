//! Windows half of the payout key-file check. The file's DACL may grant read
//! or write only to the current user, SYSTEM and Administrators. Unix checks
//! mode bits instead (`miner_guard::check_key_mode`).

use std::ffi::c_void;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::null_mut;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
    DACL_SECURITY_INFORMATION, GetAce, GetAclInformation, GetTokenInformation,
    PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR};

const FILE_READ_DATA: u32 = 0x1;
const FILE_WRITE_DATA: u32 = 0x2;
const FILE_APPEND_DATA: u32 = 0x4;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const GENERIC_READ: u32 = 0x8000_0000;
/// Access bits that let an account read or write the key's contents.
const OTHERS_ACCESS_MASK: u32 = FILE_READ_DATA
    | FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | GENERIC_READ
    | GENERIC_WRITE
    | GENERIC_ALL;
const ACCESS_ALLOWED: u8 = 0x0;
const ACCESS_DENIED: u8 = 0x1;
const SYSTEM_SID: &str = "S-1-5-18";
const ADMINISTRATORS_SID: &str = "S-1-5-32-544";

/// True when an account other than the current user, SYSTEM or Administrators
/// can read or write `path`. A missing DACL gives everyone full access, so it
/// counts as open.
///
/// # Errors
///
/// The token, the file's security descriptor or an ACE cannot be read. The
/// caller treats that as a refusal.
pub fn others_can_access(path: &Path) -> Result<bool, String> {
    let me = current_user_sid()?;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut dacl: *mut ACL = null_mut();
    let mut sd = PSECURITY_DESCRIPTOR(null_mut());
    // SAFETY: `wide` is NUL-terminated and outlives the call; the out-pointers are locals.
    let status = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl as *mut _),
            None,
            &mut sd,
        )
    };
    let _sd = SecurityDescriptor(sd);
    status.ok().map_err(|e| e.to_string())?;
    if dacl.is_null() {
        return Ok(true);
    }
    let mut info = ACL_SIZE_INFORMATION::default();
    let info_len = u32::try_from(size_of::<ACL_SIZE_INFORMATION>()).map_err(|e| e.to_string())?;
    // SAFETY: `dacl` points into `_sd`, which lives to the end of this function; `info` is the
    // structure the AclSizeInformation class writes.
    unsafe { GetAclInformation(dacl, (&raw mut info).cast(), info_len, AclSizeInformation) }
        .map_err(|e| e.to_string())?;
    for index in 0..info.AceCount {
        let mut ace = null_mut::<c_void>();
        // SAFETY: `index` is below AceCount, so the ACE exists and `ace` points into the DACL.
        unsafe { GetAce(dacl, index, &mut ace) }.map_err(|e| e.to_string())?;
        if ace_opens_to_others(ace, &me)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// One ACE: does it grant the read or write bits to an account outside the allow list?
fn ace_opens_to_others(ace: *mut c_void, me: &str) -> Result<bool, String> {
    // SAFETY: GetAce returned a pointer to a complete ACE, whose header comes first.
    let header = unsafe { &*ace.cast::<ACE_HEADER>() };
    if header.AceType == ACCESS_DENIED {
        return Ok(false);
    }
    // SAFETY: Mask follows the header in every allow-type ACE, so reading it is in bounds.
    let mask = unsafe { (*ace.cast::<ACCESS_ALLOWED_ACE>()).Mask };
    if mask & OTHERS_ACCESS_MASK == 0 {
        return Ok(false);
    }
    if header.AceType != ACCESS_ALLOWED {
        // Object and callback ACEs carry fields this check does not parse; refuse them.
        return Ok(true);
    }
    // SAFETY: the SID of an ACCESS_ALLOWED ACE starts at SidStart, inside the ACE.
    let sid = PSID(
        unsafe {
            ace.cast::<u8>()
                .add(offset_of!(ACCESS_ALLOWED_ACE, SidStart))
        }
        .cast(),
    );
    let text = sid_text(sid)?;
    Ok(text != me && text != SYSTEM_SID && text != ADMINISTRATORS_SID)
}

/// The current process user's SID in string form (`S-1-5-21-...`).
fn current_user_sid() -> Result<String, String> {
    let mut raw = HANDLE::default();
    // SAFETY: the pseudo-handle needs no closing; `raw` receives a token that `Token` closes.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) }
        .map_err(|e| e.to_string())?;
    let token = Token(raw);
    let mut needed = 0u32;
    // SAFETY: a null buffer asks only for the size; the call fails and writes `needed`.
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut needed) };
    // u64 words keep the buffer aligned for TOKEN_USER.
    let mut buf = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `buf` holds `needed` bytes, aligned for TOKEN_USER.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    }
    .map_err(|e| e.to_string())?;
    // SAFETY: a successful TokenUser query wrote a TOKEN_USER at the start of `buf`.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    sid_text(sid)
}

/// A SID as its string form. The system string is freed before returning.
fn sid_text(sid: PSID) -> Result<String, String> {
    let mut raw = PWSTR(null_mut());
    // SAFETY: `sid` points at a valid SID; on success `raw` is a system-allocated string.
    unsafe { ConvertSidToStringSidW(sid, &mut raw) }.map_err(|e| e.to_string())?;
    // SAFETY: `raw` is NUL-terminated on success, and is freed on the next line either way.
    let text = unsafe { raw.to_string() };
    let _ = unsafe { LocalFree(Some(HLOCAL(raw.0.cast()))) };
    text.map_err(|e| e.to_string())
}

/// Closes the token handle on drop.
struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcessToken and is closed exactly once, here.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Frees the security descriptor GetNamedSecurityInfoW allocated, which the DACL points into.
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: the pointer is the LocalAlloc'd block from GetNamedSecurityInfoW (or null).
        let _ = unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// A key file in the temp folder whose DACL is set by icacls `args`.
    fn key_with_acl(test: &str, args: &[&str]) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("maya-hub-acl-{test}-{}.key", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"secret").expect("write the key file");
        let status = Command::new("icacls")
            .arg(&path)
            .args(args)
            .status()
            .expect("run icacls");
        assert!(status.success(), "icacls {args:?} failed");
        path
    }

    #[test]
    fn owner_only_key_file_is_accepted() {
        let me = current_user_sid().expect("current user SID");
        let grant = format!("*{me}:F");
        let path = key_with_acl("owner", &["/inheritance:r", "/grant:r", &grant]);
        let open = others_can_access(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(open, Ok(false));
    }

    #[test]
    fn system_and_administrators_may_hold_the_key() {
        let me = current_user_sid().expect("current user SID");
        let grant = format!("*{me}:F");
        let path = key_with_acl(
            "trusted",
            &[
                "/inheritance:r",
                "/grant:r",
                &grant,
                "/grant:r",
                "*S-1-5-18:F",
                "/grant:r",
                "*S-1-5-32-544:F",
            ],
        );
        let open = others_can_access(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(open, Ok(false));
    }

    #[test]
    fn everyone_read_ace_is_refused() {
        let me = current_user_sid().expect("current user SID");
        let grant = format!("*{me}:F");
        let path = key_with_acl(
            "everyone",
            &["/inheritance:r", "/grant:r", &grant, "/grant", "*S-1-1-0:R"],
        );
        let open = others_can_access(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(open, Ok(true));
    }
}
