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
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR};

const FILE_READ_DATA: u32 = 0x1;
const FILE_WRITE_DATA: u32 = 0x2;
const FILE_APPEND_DATA: u32 = 0x4;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const GENERIC_READ: u32 = 0x8000_0000;
const DELETE: u32 = 0x0001_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
/// Access bits that let an account read, write, delete or re-grant the key.
/// WRITE_DAC and WRITE_OWNER let an account rewrite the DACL, so they count too.
const OTHERS_ACCESS_MASK: u32 = FILE_READ_DATA
    | FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_READ
    | GENERIC_WRITE
    | GENERIC_ALL;
/// On a folder, FILE_WRITE_DATA adds a file, FILE_APPEND_DATA adds a subfolder
/// and FILE_DELETE_CHILD removes any entry: together, what swapping the key
/// file needs. Listing the folder does not, so read bits are left out.
const FILE_DELETE_CHILD: u32 = 0x40;
const FOLDER_WRITE_MASK: u32 = FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_DELETE_CHILD
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_WRITE
    | GENERIC_ALL;
const ACCESS_ALLOWED: u8 = 0x0;
const ACCESS_DENIED: u8 = 0x1;
const SYSTEM_SID: &str = "S-1-5-18";
const ADMINISTRATORS_SID: &str = "S-1-5-32-544";

/// True when an account other than the current user, SYSTEM or Administrators
/// can read or write `path`, or owns it. A missing DACL gives everyone full
/// access, so it counts as open.
///
/// # Errors
///
/// The token, the file's security descriptor or an ACE cannot be read. The
/// caller treats that as a refusal.
pub fn others_can_access(path: &Path) -> Result<bool, String> {
    others_hold(path, OTHERS_ACCESS_MASK)
}

/// True when an account other than the current user, SYSTEM or Administrators
/// can add, remove or rename entries in the folder `path`, or owns it: anyone
/// who could swap the key file between the Hub's check and the miner's open.
///
/// # Errors
///
/// As [`others_can_access`].
pub fn others_can_write_into(path: &Path) -> Result<bool, String> {
    others_hold(path, FOLDER_WRITE_MASK)
}

/// Whether an untrusted account owns `path` or is granted any bit of `mask`.
fn others_hold(path: &Path, mask: u32) -> Result<bool, String> {
    let me = current_user_sid()?;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut dacl: *mut ACL = null_mut();
    let mut owner = PSID(null_mut());
    let mut sd = PSECURITY_DESCRIPTOR(null_mut());
    // SAFETY: `wide` is NUL-terminated and outlives the call; the out-pointers are locals.
    let status = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
            Some(&mut owner as *mut _),
            None,
            Some(&mut dacl as *mut _),
            None,
            &mut sd,
        )
    };
    let _sd = SecurityDescriptor(sd);
    status.ok().map_err(|e| e.to_string())?;
    // The owner always holds WRITE_DAC implicitly, and the DACL does not show it,
    // so a file someone else owns is refused whatever its ACEs say.
    if owner.0.is_null() || !account_is_trusted(&sid_text(owner)?, &me) {
        return Ok(true);
    }
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
        if ace_opens_to_others(ace, &me, mask)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// One ACE: does it grant any bit of `mask` to an account outside the allow list?
fn ace_opens_to_others(ace: *mut c_void, me: &str, others_mask: u32) -> Result<bool, String> {
    // SAFETY: GetAce returned a pointer to a complete ACE, whose header comes first.
    let header = unsafe { &*ace.cast::<ACE_HEADER>() };
    if header.AceType == ACCESS_DENIED {
        return Ok(false);
    }
    // SAFETY: Mask follows the header in every allow-type ACE, so reading it is in bounds.
    let mask = unsafe { (*ace.cast::<ACCESS_ALLOWED_ACE>()).Mask };
    if mask & others_mask == 0 {
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
    Ok(!account_is_trusted(&text, me))
}

/// The accounts allowed to hold the key: the current user, SYSTEM and Administrators.
fn account_is_trusted(text: &str, me: &str) -> bool {
    text == me || text == SYSTEM_SID || text == ADMINISTRATORS_SID
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

    #[test]
    fn users_read_ace_is_refused() {
        let me = current_user_sid().expect("current user SID");
        let grant = format!("*{me}:F");
        let path = key_with_acl(
            "users",
            &[
                "/inheritance:r",
                "/grant:r",
                &grant,
                "/grant",
                "*S-1-5-32-545:R",
            ],
        );
        let open = others_can_access(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(open, Ok(true));
    }

    #[test]
    fn everyone_write_dac_grant_is_refused() {
        let me = current_user_sid().expect("current user SID");
        let grant = format!("*{me}:F");
        let path = key_with_acl(
            "write-dac",
            &[
                "/inheritance:r",
                "/grant:r",
                &grant,
                "/grant",
                "*S-1-1-0:(WDAC)",
            ],
        );
        let open = others_can_access(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(open, Ok(true));
    }

    /// A folder in the temp folder whose DACL is set by icacls `args`.
    fn folder_with_acl(test: &str, args: &[&str]) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("maya-hub-acl-dir-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the folder");
        let status = Command::new("icacls")
            .arg(&path)
            .args(args)
            .status()
            .expect("run icacls");
        assert!(status.success(), "icacls {args:?} failed");
        path
    }

    #[test]
    fn a_folder_others_may_only_list_is_not_writable_by_them() {
        let me = current_user_sid().expect("current user SID");
        let grant = format!("*{me}:F");
        let dir = folder_with_acl(
            "list",
            &[
                "/inheritance:r",
                "/grant:r",
                &grant,
                "/grant",
                "*S-1-5-11:RX",
            ],
        );
        let open = others_can_write_into(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(open, Ok(false));
    }

    #[test]
    fn a_folder_others_may_add_files_to_is_refused() {
        let me = current_user_sid().expect("current user SID");
        let grant = format!("*{me}:F");
        // (WD) on a folder is "create files": enough to plant a key after deleting one.
        let dir = folder_with_acl(
            "add",
            &[
                "/inheritance:r",
                "/grant:r",
                &grant,
                "/grant",
                "*S-1-5-11:(WD)",
            ],
        );
        let open = others_can_write_into(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(open, Ok(true));
    }

    #[test]
    fn only_the_allow_listed_accounts_are_trusted() {
        let me = "S-1-5-21-1-2-3-1001";
        assert!(account_is_trusted(me, me));
        assert!(account_is_trusted(SYSTEM_SID, me));
        assert!(account_is_trusted(ADMINISTRATORS_SID, me));
        assert!(!account_is_trusted("S-1-5-21-1-2-3-1002", me));
        assert!(!account_is_trusted("S-1-1-0", me));
        assert!(!account_is_trusted("S-1-5-32-545", me));
    }
}
