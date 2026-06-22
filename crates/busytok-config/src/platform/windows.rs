//! Windows-only helpers for reading SIDs from the current process token.
//!
//! These wrap the raw `windows-sys` FFI calls; the public API returns
//! `Option<String>` so callers can fall back gracefully if the OS denies
//! access (e.g. sandboxed processes).

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, HANDLE};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenGroups, TokenUser, PSID, TOKEN_GROUPS, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Convert the current user's SID to SDDL string form (e.g. "S-1-5-21-...").
///
/// Returns `None` if the process token cannot be opened or the SID cannot
/// be converted to a string.
pub fn current_user_sid_string() -> Option<String> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        if len == 0 {
            CloseHandle(token);
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as *mut _, len, &mut len);
        CloseHandle(token);
        if ok == 0 {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        sid_to_string(user.User.Sid)
    }
}

/// Logon SID (S-1-5-5-...) for the current session -- used for per-session
/// ACL isolation.
///
/// Returns `None` if the process token cannot be opened, no logon group is
/// present, or the SID cannot be converted to a string.
pub fn current_logon_sid_string() -> Option<String> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenGroups, std::ptr::null_mut(), 0, &mut len);
        if len == 0 {
            CloseHandle(token);
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        let ok = GetTokenInformation(
            token,
            TokenGroups,
            buf.as_mut_ptr() as *mut _,
            len,
            &mut len,
        );
        CloseHandle(token);
        if ok == 0 {
            return None;
        }
        let groups = &*(buf.as_ptr() as *const TOKEN_GROUPS);
        // SE_GROUP_LOGON_ID is documented as 0xC0000000 but typed as i32 in
        // windows-sys; mask against the bit pattern rather than the literal.
        const SE_GROUP_LOGON_ID_BIT: u32 = 0xC000_0000;
        for i in 0..groups.GroupCount {
            let sa = &*groups.Groups.as_ptr().add(i as usize);
            if (sa.Attributes & SE_GROUP_LOGON_ID_BIT) == SE_GROUP_LOGON_ID_BIT {
                return sid_to_string(sa.Sid);
            }
        }
        None
    }
}

/// Convert a raw SID pointer into its SDDL string form.
///
/// # Safety
/// `sid` must be a valid PSID returned by the Windows API.
unsafe fn sid_to_string(sid: PSID) -> Option<String> {
    let mut str_ptr: *mut u16 = std::ptr::null_mut();
    if ConvertSidToStringSidW(sid, &mut str_ptr) == 0 {
        return None;
    }
    // Defensive upper bound (256 wide chars): ConvertSidToStringSidW should
    // always be NUL-terminated, but cap the walk so a malformed buffer cannot
    // make us read arbitrary memory.
    let len = (0isize..)
        .take(256)
        .take_while(|&i| *str_ptr.offset(i) != 0)
        .count();
    let s = String::from_utf16_lossy(std::slice::from_raw_parts(str_ptr, len));
    LocalFree(str_ptr as *mut _);
    Some(s)
}

#[allow(dead_code)]
fn last_error() -> u32 {
    unsafe { GetLastError() }
}
