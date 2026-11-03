//! Windows access control for the state directory. It holds the node's tunnel
//! token, and the default %ProgramData% ACL lets every local user read
//! folders there (and create them).

use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use windows_sys::core::PWSTR;
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE, WIN32_ERROR};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW,
    SetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetSecurityDescriptorDacl, GetTokenInformation, IsWellKnownSid, TokenUser, WinBuiltinAdministratorsSid,
    WinLocalSystemSid, ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Give `dir` a DACL that grants SYSTEM, Administrators and the account the
/// node runs as full control, and nobody else anything. Inheritance from the
/// parent is cut, and the files inside inherit this DACL.
///
/// A directory someone else owns is refused rather than fixed: its owner can
/// change the DACL back at any time, and may have planted a config in it.
pub fn restrict_dir(dir: &Path) -> io::Result<()> {
    let path: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let user = ProcessUser::get()?;
    if !owned_by(&path, user.sid())? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "it is owned by another account; delete it so the node can create it anew",
        ));
    }
    let sddl = format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{})", sid_string(user.sid())?);
    let sddl: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    // SAFETY: the strings are NUL-terminated, the security descriptor lives
    // until the DACL taken from it has been applied, and every out pointer
    // points to a local of the expected type.
    unsafe {
        let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let _sd = LocalBox(sd);
        let (mut present, mut defaulted, mut dacl) = (0, 0, ptr::null_mut::<ACL>());
        if GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) == 0 {
            return Err(io::Error::last_os_error());
        }
        check(SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null(),
        ))
    }
}

/// Whether `path` (NUL-terminated) is owned by SYSTEM, Administrators or `user`.
fn owned_by(path: &[u16], user: PSID) -> io::Result<bool> {
    // SAFETY: `owner` points into `sd`, which is freed only after the checks.
    unsafe {
        let mut owner: PSID = ptr::null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
        check(GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut sd,
        ))?;
        let _sd = LocalBox(sd);
        Ok(IsWellKnownSid(owner, WinLocalSystemSid) != 0
            || IsWellKnownSid(owner, WinBuiltinAdministratorsSid) != 0
            || EqualSid(owner, user) != 0)
    }
}

/// The user of this process's token.
struct ProcessUser(Vec<u64>);

impl ProcessUser {
    fn get() -> io::Result<Self> {
        // SAFETY: the buffer is as large as the first call said it must be,
        // and u64s keep the TOKEN_USER at its start aligned.
        unsafe {
            let mut token: HANDLE = ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut len = 0;
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            let result = if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(ProcessUser(buf)) };
            CloseHandle(token);
            result
        }
    }

    fn sid(&self) -> PSID {
        // SAFETY: `get` filled the buffer with a TOKEN_USER, whose SID points
        // into the same buffer.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}

/// `S-1-5-...` form of `sid`, for SDDL.
fn sid_string(sid: PSID) -> io::Result<String> {
    // SAFETY: on success the system hands back a NUL-terminated string, which
    // is read before it is freed.
    unsafe {
        let mut s: PWSTR = ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut s) == 0 {
            return Err(io::Error::last_os_error());
        }
        let _s = LocalBox(s.cast());
        let len = (0..).take_while(|&i| *s.add(i) != 0).count();
        Ok(String::from_utf16_lossy(std::slice::from_raw_parts(s, len)))
    }
}

fn check(err: WIN32_ERROR) -> io::Result<()> {
    if err == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(err as i32))
    }
}

/// Memory the system allocated with LocalAlloc, freed on drop.
struct LocalBox(*mut c_void);

impl Drop for LocalBox {
    fn drop(&mut self) {
        // SAFETY: the pointer came from the system for us to LocalFree, and
        // LocalFree accepts null.
        unsafe { LocalFree(self.0) };
    }
}
